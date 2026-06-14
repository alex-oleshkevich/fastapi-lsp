/// Extract `request.url_for("name")` and `obj.url_path_for("name")` call sites.
/// These are the primary consumer of the route_names index (REQ-ROUTE-11).
use tree_sitter::{Node, Tree};

use tower_lsp_server::ls_types::{Position, Range};
use crate::state::{FileFacts, UrlForSite, range_from_node};
use super::unquote;

const URL_FOR_METHODS: &[&str] = &["url_for", "url_path_for"];

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    walk(src, tree.root_node(), facts, enc);
}

fn walk(src: &[u8], node: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    if node.kind() == "call" {
        extract_call(src, node, facts, enc);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(src, child, facts, enc);
    }
}

fn extract_call(src: &[u8], call: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let callee = match call.child_by_field_name("function") {
        Some(n) => n,
        None => return,
    };
    if callee.kind() != "attribute" {
        return;
    }

    let method_name = callee
        .child_by_field_name("attribute")
        .map(|n| node_text(src, n))
        .unwrap_or("");

    if !URL_FOR_METHODS.contains(&method_name) {
        return;
    }

    let args = match call.child_by_field_name("arguments") {
        Some(a) => a,
        None => return,
    };

    // First positional arg is the route name (with content range for completions)
    let (route_name, name_range) = match first_string_arg_with_range(src, args, enc) {
        Some(pair) => pair,
        None => return,
    };

    // Collect keyword argument names (path parameters for URL building)
    let (kwarg_names, has_splat_kwargs) = collect_kwarg_names(src, args);

    facts.url_for_sites.push(UrlForSite {
        name: route_name,
        kwarg_names,
        has_splat_kwargs,
        range: range_from_node(call, src, enc),
        name_range: Some(name_range),
    });
}

fn first_string_arg_with_range(
    src: &[u8],
    args: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<(String, Range)> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            "string" => {
                let raw = node_text(src, child);
                let content_range = string_content_range(child, raw, src, enc);
                return Some((unquote(raw), content_range));
            }
            _ => {}
        }
    }
    None
}

fn string_content_range(node: Node<'_>, raw: &str, src: &[u8], enc: crate::offset::Encoding) -> Range {
    let base = range_from_node(node, src, enc);
    let no_prefix = raw.trim_start_matches(['r', 'b', 'R', 'B', 'f', 'F']);
    let prefix_extra = (raw.len() - no_prefix.len()) as u32;
    let quote_len: u32 = if no_prefix.starts_with("\"\"\"") || no_prefix.starts_with("'''") { 3 } else { 1 };
    Range {
        start: Position::new(base.start.line, base.start.character + prefix_extra + quote_len),
        end: Position::new(base.end.line, base.end.character - quote_len),
    }
}

fn collect_kwarg_names(src: &[u8], args: Node<'_>) -> (Vec<String>, bool) {
    let mut names = vec![];
    let mut has_splat = false;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "dictionary_splat_argument" {
            has_splat = true;
        } else if child.kind() == "keyword_argument"
            && let Some(key) = child.child(0)
                && key.kind() == "identifier" {
                    names.push(node_text(src, key).to_owned());
                }
    }
    (names, has_splat)
}

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_file;
    use tower_lsp_server::ls_types::Uri;

    fn uri() -> Uri {
        "file:///app/main.py".parse().unwrap()
    }

    fn run(src: &str) -> FileFacts {
        let bytes = src.as_bytes();
        let tree = parse_file(bytes);
        let mut facts = FileFacts::new(uri());
        extract(bytes, &tree, &mut facts, crate::offset::Encoding::Utf8);
        facts
    }

    #[test]
    fn request_url_for() {
        let facts = run(r#"
@app.get("/")
async def index(request: Request):
    url = request.url_for("list_books")
"#);
        assert_eq!(facts.url_for_sites.len(), 1);
        assert_eq!(facts.url_for_sites[0].name, "list_books");
    }

    #[test]
    fn url_path_for_with_kwargs() {
        let facts = run(r#"
url = app.url_path_for("get_book", book_id=42)
"#);
        assert_eq!(facts.url_for_sites.len(), 1);
        assert_eq!(facts.url_for_sites[0].name, "get_book");
        assert_eq!(facts.url_for_sites[0].kwarg_names, vec!["book_id"]);
    }

    #[test]
    fn non_url_for_calls_ignored() {
        let facts = run(r#"
result = some_other.method("arg")
url = client.get("/api")
"#);
        assert_eq!(facts.url_for_sites.len(), 0);
    }

    #[test]
    fn url_for_in_template_context() {
        let facts = run(r#"
templates.TemplateResponse("index.html", {"request": request,
    "home_url": request.url_for("home")})
"#);
        assert_eq!(facts.url_for_sites.len(), 1);
        assert_eq!(facts.url_for_sites[0].name, "home");
    }
}
