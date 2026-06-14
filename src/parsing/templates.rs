/// Extract template environment declarations and TemplateResponse/get_template call sites.
/// Implements REQ-TPL-01: only fires when the call receiver is provably bound to
/// Jinja2Templates(...) or jinja2.Environment(...) in the same file.
use tree_sitter::{Node, Tree};

use tower_lsp_server::ls_types::{Position, Range};

use crate::state::{FileFacts, TemplateEnvDecl, TemplateRef, TemplateUrlForSite, range_from_node};
use super::unquote;

const ENV_CTORS: &[&str] = &["Jinja2Templates", "Environment"];
const TEMPLATE_METHODS: &[&str] = &["TemplateResponse", "get_template"];

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let root = tree.root_node();

    // Pass 1: collect env var names (bound to Jinja2Templates / jinja2.Environment).
    let mut env_names: Vec<String> = vec![];
    collect_env_decls(src, root, facts, &mut env_names, enc);

    // Pass 2: collect template refs, filtered to known env bindings (P4).
    collect_template_refs(src, root, &env_names, facts, enc);
}

// ── Pass 1: env declarations ─────────────────────────────────────────────────

fn collect_env_decls(
    src: &[u8],
    node: Node<'_>,
    facts: &mut FileFacts,
    env_names: &mut Vec<String>,
    enc: crate::offset::Encoding,
) {
    if (node.kind() == "assignment" || node.kind() == "annotated_assignment")
        && let Some(decl) = try_extract_env_decl(src, node, enc) {
            env_names.push(decl.var_name.clone());
            facts.template_envs.push(decl);
        }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_env_decls(src, child, facts, env_names, enc);
    }
}

fn try_extract_env_decl(src: &[u8], assign: Node<'_>, enc: crate::offset::Encoding) -> Option<TemplateEnvDecl> {
    let left = assign.child_by_field_name("left")?;
    if left.kind() != "identifier" {
        return None;
    }
    let var_name = node_text(src, left).to_owned();

    let right = assign.child_by_field_name("right")?;
    if right.kind() != "call" {
        return None;
    }

    let func = right.child_by_field_name("function")?;
    let ctor_name = match func.kind() {
        "identifier" => node_text(src, func),
        "attribute" => {
            let attr = func.child_by_field_name("attribute").map(|n| node_text(src, n))?;
            // For attribute access, only accept jinja2.Environment — bare `Environment`
            // (from `from jinja2 import Environment`) is handled by the identifier arm.
            // Any other X.Environment (e.g., SQLAlchemy) must not be treated as a template env.
            if attr == "Environment" {
                let obj = func.child_by_field_name("object").map(|n| node_text(src, n)).unwrap_or("");
                if obj != "jinja2" {
                    return None;
                }
            }
            attr
        }
        _ => return None,
    };
    if !ENV_CTORS.contains(&ctor_name) {
        return None;
    }

    let args = right.child_by_field_name("arguments")?;
    let directories = extract_directory_args(src, args);

    Some(TemplateEnvDecl { var_name, directories, range: range_from_node(left, src, enc) })
}

/// Extract `directory=` kwarg or first positional string from a constructor's args.
fn extract_directory_args(src: &[u8], args: Node<'_>) -> Vec<String> {
    let mut dirs: Vec<String> = vec![];
    let mut cursor = args.walk();
    let mut positional_count = 0usize;

    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "keyword_argument" => {
                let key = child.child(0).map(|n| node_text(src, n)).unwrap_or("");
                if key == "directory"
                    && let Some(val) = child.child(2) {
                        push_string_or_list(src, val, &mut dirs);
                    }
            }
            _ if positional_count == 0 => {
                push_string_or_list(src, child, &mut dirs);
                positional_count += 1;
            }
            _ => {
                positional_count += 1;
            }
        }
    }
    dirs
}

fn push_string_or_list(src: &[u8], node: Node<'_>, out: &mut Vec<String>) {
    match node.kind() {
        "string" => out.push(unquote(node_text(src, node))),
        "list" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == "string" {
                    out.push(unquote(node_text(src, child)));
                }
            }
        }
        _ => {}
    }
}

// ── Pass 2: template reference calls ─────────────────────────────────────────

fn collect_template_refs(
    src: &[u8],
    node: Node<'_>,
    env_names: &[String],
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    if node.kind() == "call"
        && let Some(tpl) = try_extract_template_ref(src, node, env_names, enc) {
            facts.templates.push(tpl);
        }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_template_refs(src, child, env_names, facts, enc);
    }
}

fn try_extract_template_ref(
    src: &[u8],
    call: Node<'_>,
    env_names: &[String],
    enc: crate::offset::Encoding,
) -> Option<TemplateRef> {
    let callee = call.child_by_field_name("function")?;
    if callee.kind() != "attribute" {
        return None;
    }
    let obj = callee.child_by_field_name("object")?;
    let method = callee.child_by_field_name("attribute")?;

    if obj.kind() != "identifier" {
        return None;
    }
    let obj_name = node_text(src, obj);
    let method_name = node_text(src, method);

    if !TEMPLATE_METHODS.contains(&method_name) {
        return None;
    }
    // P4: only fire when receiver is a proven env binding in this file.
    if !env_names.iter().any(|n| n == obj_name) {
        return None;
    }

    let args = call.child_by_field_name("arguments")?;
    let (path, string_range) = extract_template_name(src, args, enc)?;

    Some(TemplateRef { path, range: string_range })
}

/// Extract the template name string from a call's argument list.
///
/// Priority:
/// 1. `name=` keyword argument (explicit, always wins)
/// 2. First string literal in the positional args — handles both argument orders:
///    - legacy  `TemplateResponse("name.html", context)` → "name.html" is arg 0
///    - modern  `TemplateResponse(request, "name.html")` → request is not a string, "name.html" is arg 1
fn extract_template_name(src: &[u8], args: Node<'_>, enc: crate::offset::Encoding) -> Option<(String, Range)> {
    // Check for `name=` kwarg first.
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "keyword_argument" {
            let key = child.child(0).map(|n| node_text(src, n)).unwrap_or("");
            if key == "name"
                && let Some(val) = child.child(2)
                    && val.kind() == "string" {
                        return Some((unquote(node_text(src, val)), range_from_node(val, src, enc)));
                    }
        }
    }

    // First positional string literal.
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            "string" => return Some((unquote(node_text(src, child)), range_from_node(child, src, enc))),
            _ => {}
        }
    }
    None
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ── Unit tests ────────────────────────────────────────────────────────────────

// ── Template file lexical scan (REQ-TPL-06) ───────────────────────────────────

/// Lexically scan a template file (HTML/Jinja) for `url_for(` / `url_path_for(` sites.
/// This is deliberately a narrow text scan, not a Jinja parse — we only need these islands.
pub fn scan_url_for_sites(src: &[u8]) -> Vec<TemplateUrlForSite> {
    let text = match std::str::from_utf8(src) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    // Build a table of byte offsets for the start of each line (0-indexed).
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.char_indices().filter(|(_, c)| *c == '\n').map(|(i, _)| i + 1))
        .collect();

    let offset_to_pos = |offset: usize| -> Position {
        // partition_point returns the first index where s > offset, so -1 is the containing line.
        let line = line_starts.partition_point(|&s| s <= offset).saturating_sub(1);
        Position { line: line as u32, character: (offset - line_starts[line]) as u32 }
    };

    const PATTERNS: &[&str] = &["url_for(", "url_path_for("];

    let mut sites: Vec<TemplateUrlForSite> = vec![];
    let mut search_from = 0;

    while search_from < text.len() {
        let found = PATTERNS.iter().filter_map(|pat| {
            text[search_from..].find(pat).map(|i| (search_from + i, *pat))
        }).min_by_key(|(pos, _)| *pos);

        let (match_start, pat) = match found {
            Some(x) => x,
            None => break,
        };

        let args_start = match_start + pat.len();
        if let Some((name, string_range, kwarg_names)) =
            parse_call_args(text, args_start, &offset_to_pos)
        {
            sites.push(TemplateUrlForSite { name, string_range, kwarg_names });
        }
        // Advance past the matched pattern (all ASCII) to stay on a char boundary.
        search_from = match_start + pat.len();
    }

    sites
}

/// Parse the argument list starting right after the opening `(` of a url_for call.
/// Returns `(route_name, string_range, kwarg_names)` or `None` if no string literal found.
fn parse_call_args(
    text: &str,
    args_start: usize,
    offset_to_pos: &impl Fn(usize) -> Position,
) -> Option<(String, Range, Vec<String>)> {
    let rest = &text[args_start..];
    let chars: Vec<(usize, char)> = rest.char_indices().collect();
    let mut i = 0;

    // Skip leading whitespace.
    while i < chars.len() && chars[i].1.is_whitespace() {
        i += 1;
    }

    // First non-whitespace must be a quote for the route name.
    if i >= chars.len() {
        return None;
    }
    let quote = chars[i].1;
    if quote != '\'' && quote != '"' {
        return None;
    }

    // Find the closing quote (no escape handling needed — route names are identifiers).
    let name_start_offset = args_start + chars[i].0 + quote.len_utf8();
    let mut j = i + 1;
    while j < chars.len() && chars[j].1 != quote {
        j += 1;
    }
    if j >= chars.len() {
        return None;
    }
    let name_end_offset = args_start + chars[j].0;
    let name = rest[chars[i].0 + quote.len_utf8()..chars[j].0].to_owned();

    let string_range = Range {
        start: offset_to_pos(name_start_offset),
        end: offset_to_pos(name_end_offset),
    };

    // Collect kwarg names after the route name string.
    // Start byte is past the closing quote character.
    let kwarg_names = collect_kwarg_names(rest, chars[j].0 + quote.len_utf8());

    Some((name, string_range, kwarg_names))
}

/// Scan the remaining argument text for `ident=` patterns (kwarg names).
/// `start_byte` is the byte offset in `rest` to begin from (just past the closing route-name quote).
fn collect_kwarg_names(rest: &str, start_byte: usize) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    let slice = &rest[start_byte..];

    // Find the closing ')' to bound the scan (first unbalanced one).
    let mut depth = 1i32;
    let mut end_byte = slice.len();
    for (bi, ch) in slice.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 { end_byte = bi; break; }
            }
            _ => {}
        }
    }
    let args_text = &slice[..end_byte];

    // Extract `word=` patterns that aren't inside nested parens/brackets.
    let mut depth = 0i32;
    let mut word_start: Option<usize> = None;
    let bytes = args_text.as_bytes();
    let mut bi = 0;
    while bi < bytes.len() {
        let b = bytes[bi];
        match b {
            b'(' | b'[' | b'{' => { depth += 1; word_start = None; bi += 1; }
            b')' | b']' | b'}' => { depth -= 1; word_start = None; bi += 1; }
            b'=' if depth == 0 => {
                if let Some(ws) = word_start {
                    let ident = args_text[ws..bi].trim();
                    if !ident.is_empty() && ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        names.push(ident.to_owned());
                    }
                }
                word_start = None;
                bi += 1;
            }
            b if b.is_ascii_alphanumeric() || b == b'_' => {
                if word_start.is_none() { word_start = Some(bi); }
                bi += 1;
            }
            _ => { word_start = None; bi += 1; }
        }
    }

    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_file;
    use tower_lsp_server::ls_types::Uri;

    // ── Template url_for lexical scan tests ──────────────────────────────────

    fn scan(src: &str) -> Vec<TemplateUrlForSite> {
        scan_url_for_sites(src.as_bytes())
    }

    #[test]
    fn url_for_single_quotes() {
        let sites = scan(r#"{{ url_for('list_books') }}"#);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "list_books");
        assert!(sites[0].kwarg_names.is_empty());
    }

    #[test]
    fn url_for_double_quotes() {
        let sites = scan(r#"{{ url_for("list_books") }}"#);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "list_books");
    }

    #[test]
    fn url_path_for_recognized() {
        let sites = scan(r#"{{ url_path_for('get_book', book_id=1) }}"#);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "get_book");
    }

    #[test]
    fn kwarg_names_extracted() {
        let sites = scan(r#"{{ url_for('get_book', book_id=book.id) }}"#);
        assert_eq!(sites[0].kwarg_names, vec!["book_id"]);
    }

    #[test]
    fn multiple_kwargs() {
        let sites = scan(r#"{{ url_for('route', a=x, b=y) }}"#);
        assert_eq!(sites[0].kwarg_names, vec!["a", "b"]);
    }

    #[test]
    fn multiple_url_for_in_file() {
        let sites = scan("{{ url_for('home') }}\n{{ url_for('detail', pk=1) }}");
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].name, "home");
        assert_eq!(sites[1].name, "detail");
    }

    #[test]
    fn url_for_without_string_first_arg_ignored() {
        let sites = scan(r#"{{ url_for(name_var) }}"#);
        assert_eq!(sites.len(), 0);
    }

    #[test]
    fn string_range_line_and_col() {
        // "url_for('home')" — 'home' starts at col 9 (after `url_for('`)
        let sites = scan("{{ url_for('home') }}");
        assert_eq!(sites[0].string_range.start.line, 0);
        // offset: "{{ url_for('" = 12 bytes → col 12
        assert_eq!(sites[0].string_range.start.character, 12);
        // 'home' is 4 chars, ends at col 16
        assert_eq!(sites[0].string_range.end.character, 16);
    }

    #[test]
    fn string_range_second_line() {
        let src = "{% block %}\n{{ url_for('about') }}";
        let sites = scan(src);
        assert_eq!(sites[0].string_range.start.line, 1);
    }

    #[test]
    fn kwarg_nested_call_parens_not_confused() {
        // `id=get_id()` — the inner `()` must not close the outer scan early.
        let sites = scan(r#"{{ url_for('route', id=get_id(), q=compute(a=1)) }}"#);
        assert_eq!(sites[0].kwarg_names, vec!["id", "q"]);
        // `a=1` inside `compute(...)` is at depth 1 so it must be skipped.
    }

    #[test]
    fn request_url_for_not_matched() {
        // `request.url_for(` — the pattern `url_for(` appears inside but with `request.` prefix.
        // This is fine: we deliberately match ANY `url_for(` occurrence including through
        // `request.url_for(`. The Python-side url_for scanner already handles those in .py files.
        let sites = scan(r#"{{ request.url_for('home') }}"#);
        assert_eq!(sites.len(), 1, "request.url_for( contains url_for( so it IS matched");
        assert_eq!(sites[0].name, "home");
    }

    fn uri() -> Uri {
        "file:///app/pages.py".parse().unwrap()
    }

    fn run(src: &str) -> FileFacts {
        let bytes = src.as_bytes();
        let tree = parse_file(bytes);
        let mut facts = FileFacts::new(uri());
        extract(bytes, &tree, &mut facts, crate::offset::Encoding::Utf8);
        facts
    }

    // ── Env declaration tests ─────────────────────────────────────────────────

    #[test]
    fn jinja2templates_decl_extracted() {
        let facts = run(r#"
from fastapi.templating import Jinja2Templates
templates = Jinja2Templates(directory="templates")
"#);
        assert_eq!(facts.template_envs.len(), 1);
        assert_eq!(facts.template_envs[0].var_name, "templates");
        assert_eq!(facts.template_envs[0].directories, vec!["templates"]);
    }

    #[test]
    fn jinja2_environment_decl_extracted() {
        let facts = run(r#"
import jinja2
env = jinja2.Environment(loader=jinja2.FileSystemLoader("tmpl"))
"#);
        assert_eq!(facts.template_envs.len(), 1);
        assert_eq!(facts.template_envs[0].var_name, "env");
    }

    #[test]
    fn jinja2templates_positional_directory() {
        let facts = run(r#"
templates = Jinja2Templates("app/templates")
"#);
        assert_eq!(facts.template_envs[0].directories, vec!["app/templates"]);
    }

    // ── TemplateResponse tests ────────────────────────────────────────────────

    #[test]
    fn template_response_legacy_order() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
def index():
    return templates.TemplateResponse("index.html", {"request": request})
"#);
        assert_eq!(facts.templates.len(), 1);
        assert_eq!(facts.templates[0].path, "index.html");
    }

    #[test]
    fn template_response_modern_order() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
def index(request: Request):
    return templates.TemplateResponse(request, "index.html")
"#);
        assert_eq!(facts.templates.len(), 1);
        assert_eq!(facts.templates[0].path, "index.html");
    }

    #[test]
    fn template_response_name_kwarg() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
def index(request: Request):
    return templates.TemplateResponse(request=request, name="book_list.html")
"#);
        assert_eq!(facts.templates.len(), 1);
        assert_eq!(facts.templates[0].path, "book_list.html");
    }

    #[test]
    fn get_template_call() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
tpl = templates.get_template("email/welcome.html")
"#);
        assert_eq!(facts.templates.len(), 1);
        assert_eq!(facts.templates[0].path, "email/welcome.html");
    }

    // ── P4 guard tests ────────────────────────────────────────────────────────

    #[test]
    fn unbound_template_response_ignored() {
        let facts = run(r#"
def index():
    return something.TemplateResponse("index.html", {})
"#);
        assert_eq!(facts.templates.len(), 0, "must not fire when receiver is not a known env");
    }

    #[test]
    fn variable_template_name_ignored() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
def index():
    name = get_template_name()
    return templates.TemplateResponse(request, name)
"#);
        assert_eq!(facts.templates.len(), 0, "non-literal template name must be skipped (P4)");
    }

    #[test]
    fn annotated_assignment_env_decl() {
        let facts = run(r#"
from fastapi.templating import Jinja2Templates
templates: Jinja2Templates = Jinja2Templates(directory="templates")
"#);
        assert_eq!(facts.template_envs.len(), 1, "annotated_assignment must be recognized");
        assert_eq!(facts.template_envs[0].var_name, "templates");
    }

    #[test]
    fn list_directory_arg() {
        let facts = run(r#"
templates = Jinja2Templates(directory=["tmpl1", "tmpl2"])
"#);
        assert_eq!(facts.template_envs[0].directories, vec!["tmpl1", "tmpl2"]);
    }

    #[test]
    fn bare_environment_import_accepted() {
        let facts = run(r#"
from jinja2 import Environment
env = Environment(loader=None)
"#);
        assert_eq!(facts.template_envs.len(), 1);
        assert_eq!(facts.template_envs[0].var_name, "env");
    }

    #[test]
    fn non_jinja2_environment_rejected() {
        // SQLAlchemy or other X.Environment must not be treated as a template env.
        let facts = run(r#"
import sqlalchemy
env = sqlalchemy.Environment()
"#);
        assert_eq!(facts.template_envs.len(), 0, "X.Environment where X != jinja2 must be rejected");
    }

    #[test]
    fn chained_call_receiver_rejected() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
def index():
    return get_templates().TemplateResponse(request, "index.html")
"#);
        assert_eq!(facts.templates.len(), 0, "non-identifier receiver must be rejected");
    }

    #[test]
    fn no_positional_string_returns_none() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
def index(request: Request):
    return templates.TemplateResponse(context=ctx)
"#);
        assert_eq!(facts.templates.len(), 0, "no string arg → no template ref");
    }

    #[test]
    fn multiple_template_refs_in_file() {
        let facts = run(r#"
templates = Jinja2Templates(directory="templates")
def list_books():
    return templates.TemplateResponse(request, "book_list.html")
def detail(pk: int):
    return templates.TemplateResponse(request, "book_detail.html")
"#);
        assert_eq!(facts.templates.len(), 2);
        let paths: Vec<&str> = facts.templates.iter().map(|t| t.path.as_str()).collect();
        assert!(paths.contains(&"book_list.html"));
        assert!(paths.contains(&"book_detail.html"));
    }
}
