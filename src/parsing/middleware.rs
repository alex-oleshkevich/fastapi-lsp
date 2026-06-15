#[cfg(test)]
use tower_lsp_server::ls_types::Uri;
use tree_sitter::{Node, Tree};

use crate::state::{FileFacts, MiddlewareCall, MwClassDecl, MwKwarg, MwSource, range_from_node};

/// Stock Starlette/FastAPI middleware kwargs with detail strings (REQ-MW-03).
/// Each entry: (class_name, &[(kwarg_name, type_and_default)])
/// Versioned against Starlette 0.41 docs.
pub static STOCK_MIDDLEWARE: &[(&str, &[(&str, &str)])] = &[
    (
        "CORSMiddleware",
        &[
            ("allow_origins", "list[str] = []"),
            ("allow_methods", r#"list[str] = ["GET"]"#),
            ("allow_headers", "list[str] = []"),
            ("allow_credentials", "bool = False"),
            ("allow_origin_regex", "str | None = None"),
            ("expose_headers", "list[str] = []"),
            ("max_age", "int = 600"),
        ],
    ),
    (
        "TrustedHostMiddleware",
        &[
            ("allowed_hosts", r#"list[str] = ["*"]"#),
            ("www_redirect", "bool = True"),
        ],
    ),
    (
        "GZipMiddleware",
        &[("minimum_size", "int = 500"), ("compresslevel", "int = 9")],
    ),
    (
        "SessionMiddleware",
        &[
            ("secret_key", "str"),
            ("session_cookie", r#"str = "session""#),
            ("max_age", "int = 14 * 24 * 60 * 60"),
            ("path", r#"str = "/""#),
            ("same_site", r#"str = "lax""#),
            ("https_only", "bool = False"),
            ("domain", "str | None = None"),
        ],
    ),
    ("HTTPSRedirectMiddleware", &[]),
];

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let root = tree.root_node();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        walk(src, child, facts, enc);
    }
}

fn walk(src: &[u8], node: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    match node.kind() {
        "class_definition" => {
            extract_mw_class(src, node, facts, enc);
            // Still recurse in case there are nested classes (unusual but valid)
            recurse(src, node, facts, enc);
        }
        "decorated_definition" => {
            extract_mw_decorator(src, node, facts, enc);
            // Don't recurse into decorated function bodies
        }
        "expression_statement" => {
            if let Some(inner) = node.child(0)
                && inner.kind() == "call"
            {
                extract_mw_call(src, inner, facts, enc);
            }
            recurse(src, node, facts, enc);
        }
        "assignment" => {
            extract_from_app_constructor(src, node, facts, enc);
            recurse(src, node, facts, enc);
        }
        _ => recurse(src, node, facts, enc),
    }
}

fn recurse(src: &[u8], node: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(src, child, facts, enc);
    }
}

// ── `app.add_middleware(ClassName, ...)` ─────────────────────────────────────

fn extract_mw_call(
    src: &[u8],
    call: Node<'_>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let callee = match call.child_by_field_name("function") {
        Some(n) => n,
        None => return,
    };
    if callee.kind() != "attribute" {
        return;
    }
    let attr = callee
        .child_by_field_name("attribute")
        .map(|n| node_text(src, n))
        .unwrap_or("");
    if attr != "add_middleware" {
        return;
    }
    let app_name = callee
        .child_by_field_name("object")
        .map(|n| node_text(src, n).to_owned())
        .unwrap_or_default();

    let args = match call.child_by_field_name("arguments") {
        Some(a) => a,
        None => return,
    };

    // First positional arg is the class
    let class_name = first_positional_text(src, args);
    if class_name.is_empty() {
        return;
    }

    let present_kwargs = extract_existing_kwargs(src, args);
    let kwargs_start = first_positional_end(args);
    facts.middlewares.push(MiddlewareCall {
        app_name,
        source: MwSource::Class(class_name),
        range: range_from_node(call, src, enc),
        kwargs_start,
        present_kwargs,
    });
}

// ── `@app.middleware("http")` decorator ──────────────────────────────────────

fn extract_mw_decorator(
    src: &[u8],
    node: Node<'_>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let mut func_node = None;
    let mut decorators = vec![];

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "decorator" => decorators.push(child),
            "function_definition" | "async_function_definition" => func_node = Some(child),
            _ => {}
        }
    }

    let func = match func_node {
        Some(f) => f,
        None => return,
    };
    let fn_name = match func.child_by_field_name("name") {
        Some(n) => node_text(src, n).to_owned(),
        None => return,
    };

    for decorator in decorators {
        // @<obj>.middleware("<type>") — skip the '@' token, find the expression node
        let expr = {
            let mut cursor = decorator.walk();
            let mut found = None;
            for child in decorator.children(&mut cursor) {
                if child.kind() != "@" {
                    found = Some(child);
                    break;
                }
            }
            match found {
                Some(e) => e,
                None => continue,
            }
        };
        if expr.kind() != "call" {
            continue;
        }
        let callee = match expr.child_by_field_name("function") {
            Some(n) => n,
            None => continue,
        };
        if callee.kind() != "attribute" {
            continue;
        }
        let attr = callee
            .child_by_field_name("attribute")
            .map(|n| node_text(src, n))
            .unwrap_or("");
        if attr != "middleware" {
            continue;
        }
        let app_name = callee
            .child_by_field_name("object")
            .map(|n| node_text(src, n).to_owned())
            .unwrap_or_default();

        facts.middlewares.push(MiddlewareCall {
            app_name,
            source: MwSource::DecoratorFn(fn_name.clone()),
            range: range_from_node(expr, src, enc),
            kwargs_start: None,
            present_kwargs: vec![],
        });
    }
}

// ── Workspace class: `__init__(self, app, ...)` ───────────────────────────────

fn extract_mw_class(
    src: &[u8],
    node: Node<'_>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let class_name = match node.child_by_field_name("name") {
        Some(n) => node_text(src, n).to_owned(),
        None => return,
    };

    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };

    // Look for `def __init__` inside the class body
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if matches!(
            child.kind(),
            "function_definition" | "async_function_definition" | "decorated_definition"
        ) {
            let func = if child.kind() == "decorated_definition" {
                // Find the inner function/async_function inside a decorated def
                let mut c = child.walk();
                let mut inner = None;
                for gc in child.children(&mut c) {
                    if matches!(
                        gc.kind(),
                        "function_definition" | "async_function_definition"
                    ) {
                        inner = Some(gc);
                        break;
                    }
                }
                match inner {
                    Some(f) => f,
                    None => continue,
                }
            } else {
                child
            };

            let name = match func.child_by_field_name("name") {
                Some(n) => node_text(src, n),
                None => continue,
            };

            if name != "__init__" {
                continue;
            }

            let params = match func.child_by_field_name("parameters") {
                Some(p) => p,
                None => continue,
            };

            let kwargs = extract_init_params(src, params);
            if kwargs.is_empty() {
                // No `app` param — not a middleware class
                continue;
            }

            facts.mw_classes.push(MwClassDecl {
                class_name,
                kwargs,
                range: range_from_node(node, src, enc),
            });
            return;
        }
    }
}

/// Extract kwargs from `__init__(self, app, kw1: T = v, ...)` as `MwKwarg`.
/// Returns empty vec if the first non-self param is not `app`.
fn extract_init_params(src: &[u8], params: Node<'_>) -> Vec<MwKwarg> {
    let mut kwargs = vec![];
    let mut cursor = params.walk();
    let mut positional_idx = 0usize;
    let mut has_app = false;

    for child in params.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "identifier" => {
                let name = node_text(src, child);
                if positional_idx == 0 && name == "self" {
                    positional_idx += 1;
                    continue;
                }
                if positional_idx == 1 {
                    if name == "app" {
                        has_app = true;
                    } else {
                        return vec![];
                    }
                    positional_idx += 1;
                    continue;
                }
                kwargs.push(MwKwarg {
                    name: name.to_owned(),
                    detail: None,
                });
                positional_idx += 1;
            }
            "default_parameter"
            | "typed_parameter"
            | "typed_default_parameter"
            | "keyword_only_parameter" => {
                let param_name = match child.child(0) {
                    Some(n) if n.kind() == "identifier" => node_text(src, n),
                    _ => continue,
                };
                if positional_idx == 1 {
                    if param_name == "app" {
                        has_app = true;
                    } else {
                        return vec![];
                    }
                    positional_idx += 1;
                    continue;
                }
                let detail = param_detail(src, child);
                kwargs.push(MwKwarg {
                    name: param_name.to_owned(),
                    detail,
                });
                positional_idx += 1;
            }
            "dictionary_splat_pattern"
            | "list_splat_pattern"
            | "keyword_separator"
            | "positional_separator"
            | "list_splat_argument"
            | "dictionary_splat_argument" => {}
            _ => {
                positional_idx += 1;
            }
        }
    }

    if has_app { kwargs } else { vec![] }
}

/// Build a detail string like `str = "value"` or `list[str]` from a parameter node.
fn param_detail(src: &[u8], param: Node<'_>) -> Option<String> {
    let annotation = param
        .child_by_field_name("type")
        .map(|n| node_text(src, n).to_owned());
    let default = param
        .child_by_field_name("value")
        .map(|n| node_text(src, n).to_owned());
    match (annotation, default) {
        (Some(a), Some(d)) => Some(format!("{a} = {d}")),
        (Some(a), None) => Some(a),
        (None, Some(d)) => Some(format!("= {d}")),
        (None, None) => None,
    }
}

/// Collect existing kwarg names from the argument list of a call.
fn extract_existing_kwargs(src: &[u8], args: Node<'_>) -> Vec<String> {
    let mut names = vec![];
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "keyword_argument" {
            // keyword_argument has no named 'name' field in tree-sitter-python; child(0) is the key.
            if let Some(key) = child.child(0)
                && key.kind() == "identifier"
            {
                names.push(node_text(src, key).to_owned());
            }
        }
    }
    names
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn first_positional_text(src: &[u8], args: Node<'_>) -> String {
    first_positional_node(args)
        .map(|n| node_text(src, n).to_owned())
        .unwrap_or_default()
}

fn first_positional_node(args: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            _ => return Some(child),
        }
    }
    None
}

/// Returns the position just past the first positional argument (i.e., where kwargs begin).
fn first_positional_end(args: Node<'_>) -> Option<tower_lsp_server::ls_types::Position> {
    let node = first_positional_node(args)?;
    let end = node.end_position();
    Some(tower_lsp_server::ls_types::Position::new(
        end.row as u32,
        end.column as u32,
    ))
}

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ── `app = FastAPI(middleware=[Middleware(ClassName, ...)])` ──────────────────

fn extract_from_app_constructor(
    src: &[u8],
    node: Node<'_>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let lhs = match node.child_by_field_name("left") {
        Some(n) if n.kind() == "identifier" => n,
        _ => return,
    };
    let app_name = node_text(src, lhs).to_owned();

    let rhs = match node.child_by_field_name("right") {
        Some(n) if n.kind() == "call" => n,
        _ => return,
    };
    let args = match rhs.child_by_field_name("arguments") {
        Some(a) => a,
        None => return,
    };

    // Find the `middleware=[...]` keyword argument value
    let middleware_list = {
        let mut found = None;
        let mut cursor = args.walk();
        for kwarg in args.children(&mut cursor) {
            if kwarg.kind() != "keyword_argument" {
                continue;
            }
            let is_mw = kwarg
                .child(0)
                .map(|k| k.kind() == "identifier" && node_text(src, k) == "middleware")
                .unwrap_or(false);
            if !is_mw {
                continue;
            }
            let mut vc = kwarg.walk();
            for child in kwarg.children(&mut vc) {
                if child.kind() == "list" {
                    found = Some(child);
                    break;
                }
            }
            break;
        }
        match found {
            Some(l) => l,
            None => return,
        }
    };

    let mut list_cursor = middleware_list.walk();
    for item in middleware_list.children(&mut list_cursor) {
        if item.kind() != "call" {
            continue;
        }
        let is_middleware_call = item
            .child_by_field_name("function")
            .map(|f| f.kind() == "identifier" && node_text(src, f) == "Middleware")
            .unwrap_or(false);
        if !is_middleware_call {
            continue;
        }
        let item_args = match item.child_by_field_name("arguments") {
            Some(a) => a,
            None => continue,
        };
        let class_name = first_positional_text(src, item_args);
        if class_name.is_empty() {
            continue;
        }
        let present_kwargs = extract_existing_kwargs(src, item_args);
        let kwargs_start = first_positional_end(item_args);
        facts.middlewares.push(MiddlewareCall {
            app_name: app_name.clone(),
            source: MwSource::Class(class_name),
            range: range_from_node(item, src, enc),
            kwargs_start,
            present_kwargs,
        });
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_file;

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
    fn add_middleware_class() {
        let facts = run(r#"
from starlette.middleware.cors import CORSMiddleware
app = FastAPI()
app.add_middleware(CORSMiddleware, allow_origins=["*"])
"#);
        assert_eq!(facts.middlewares.len(), 1);
        assert!(
            matches!(&facts.middlewares[0].source, MwSource::Class(n) if n == "CORSMiddleware")
        );
    }

    #[test]
    fn decorator_middleware() {
        let facts = run(r#"
app = FastAPI()

@app.middleware("http")
async def add_timing(request, call_next):
    return await call_next(request)
"#);
        assert_eq!(facts.middlewares.len(), 1);
        assert!(
            matches!(&facts.middlewares[0].source, MwSource::DecoratorFn(n) if n == "add_timing")
        );
    }

    #[test]
    fn workspace_mw_class_indexed() {
        let facts = run(r#"
class TimingMiddleware:
    def __init__(self, app, header_name: str = "X-Time"):
        self.app = app
        self.header_name = header_name
"#);
        assert_eq!(facts.mw_classes.len(), 1);
        let cls = &facts.mw_classes[0];
        assert_eq!(cls.class_name, "TimingMiddleware");
        assert_eq!(cls.kwargs.len(), 1);
        assert_eq!(cls.kwargs[0].name, "header_name");
        assert_eq!(cls.kwargs[0].detail.as_deref(), Some(r#"str = "X-Time""#));
    }

    #[test]
    fn class_without_app_param_ignored() {
        let facts = run(r#"
class NotMiddleware:
    def __init__(self, name: str):
        self.name = name
"#);
        assert_eq!(facts.mw_classes.len(), 0);
    }

    #[test]
    fn stock_table_has_cors() {
        let cors = STOCK_MIDDLEWARE
            .iter()
            .find(|(n, _)| *n == "CORSMiddleware");
        assert!(cors.is_some());
        let (_, kwargs) = cors.unwrap();
        assert!(kwargs.iter().any(|(name, _)| *name == "allow_origins"));
    }

    #[test]
    fn constructor_middleware_kwarg() {
        let facts = run(r#"
from starlette.middleware import Middleware
from starlette.middleware.cors import CORSMiddleware
app = FastAPI(middleware=[
    Middleware(CORSMiddleware, allow_origins=["*"]),
    Middleware(GZipMiddleware, minimum_size=500),
])
"#);
        assert_eq!(facts.middlewares.len(), 2);
        assert!(
            matches!(&facts.middlewares[0].source, MwSource::Class(n) if n == "CORSMiddleware")
        );
        assert!(
            matches!(&facts.middlewares[1].source, MwSource::Class(n) if n == "GZipMiddleware")
        );
        assert_eq!(facts.middlewares[0].app_name, "app");
        assert!(
            facts.middlewares[0]
                .present_kwargs
                .contains(&"allow_origins".to_owned())
        );
    }

    #[test]
    fn multiple_registrations() {
        let facts = run(r#"
app.add_middleware(GZipMiddleware, minimum_size=1000)
app.add_middleware(TrustedHostMiddleware, allowed_hosts=["*"])
"#);
        assert_eq!(facts.middlewares.len(), 2);
    }

    #[test]
    fn present_kwargs_captured() {
        let facts = run(r#"
app.add_middleware(CORSMiddleware, allow_origins=["*"], allow_methods=["GET"])
"#);
        assert_eq!(facts.middlewares.len(), 1);
        let mw = &facts.middlewares[0];
        assert!(mw.present_kwargs.contains(&"allow_origins".to_owned()));
        assert!(mw.present_kwargs.contains(&"allow_methods".to_owned()));
        assert!(!mw.present_kwargs.contains(&"allow_headers".to_owned()));
    }

    #[test]
    fn kwargs_start_set_past_class_arg() {
        let facts = run("app.add_middleware(CORSMiddleware, allow_origins=[])");
        let mw = &facts.middlewares[0];
        let ks = mw.kwargs_start.expect("kwargs_start should be set");
        // CORSMiddleware ends at column 33 (0-indexed), kwargs start should be >= that
        assert_eq!(ks.line, 0);
        assert!(ks.character >= 18); // past "app.add_middleware(" (19 chars) and class name start
    }

    #[test]
    fn workspace_kwarg_with_annotation_and_default() {
        let facts = run(r#"
class TimingMiddleware:
    def __init__(self, app, header_name: str = "X-Time", log_slow: bool = False):
        pass
"#);
        assert_eq!(facts.mw_classes.len(), 1);
        let cls = &facts.mw_classes[0];
        assert_eq!(cls.kwargs.len(), 2);
        assert_eq!(cls.kwargs[0].name, "header_name");
        assert_eq!(cls.kwargs[1].name, "log_slow");
        assert!(cls.kwargs[0].detail.is_some());
        assert!(cls.kwargs[1].detail.is_some());
    }
}
