use std::collections::HashSet;
use tree_sitter::{Node, Tree};

use super::unquote;
use crate::state::{ClientCall, FStringSegment, FileFacts, Method, range_from_node};
use tower_lsp_server::ls_types::{Position, Range};

pub fn extract(
    src: &[u8],
    tree: &Tree,
    facts: &mut FileFacts,
    is_test_file: bool,
    client_fixtures: &[String],
    enc: crate::offset::Encoding,
) {
    if !is_test_file {
        return;
    }
    let root = tree.root_node();

    // Pass 1: build the set of names bound to test-client instances in this file,
    // plus the fixture parameter names from config (REQ-TLINK-01).
    let mut client_names: HashSet<String> = client_fixtures.iter().cloned().collect();
    collect_client_bindings(src, root, &mut client_names);

    if client_names.is_empty() {
        return;
    }

    // Pass 2: find <obj>.<verb>(<path>, …) calls where obj is a known client.
    collect_http_calls(src, root, &client_names, facts, enc);
}

// ── Pass 1: discover test-client variable names ───────────────────────────────

fn collect_client_bindings(src: &[u8], node: Node<'_>, names: &mut HashSet<String>) {
    if node.kind() == "assignment" {
        if let (Some(lhs), Some(rhs)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) && lhs.kind() == "identifier"
            && is_test_client_ctor(src, rhs)
        {
            names.insert(node_text(src, lhs).to_owned());
        }
    } else if node.kind() == "typed_parameter" {
        collect_typed_param_client(src, node, names);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_client_bindings(src, child, names);
    }
}

/// Detect `param_name: SomeClientType` in function signatures and add `param_name` to names.
fn collect_typed_param_client(src: &[u8], node: Node<'_>, names: &mut HashSet<String>) {
    let mut cursor = node.walk();
    let mut param_name: Option<&str> = None;
    let mut seen_colon = false;
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" && param_name.is_none() {
            param_name = Some(node_text(src, child));
        } else if child.kind() == ":" {
            seen_colon = true;
        } else if seen_colon {
            // The annotation may be wrapped in a 'type' node (tree-sitter Python wraps it)
            let inner = if child.kind() == "type" {
                child.child(0).unwrap_or(child)
            } else {
                child
            };
            if is_client_type_node(src, inner)
                && let Some(name) = param_name
            {
                names.insert(name.to_owned());
            }
            break;
        }
    }
}

/// Returns true if `node` is a type annotation referring to a known HTTP client class:
/// `httpx.AsyncClient`, `httpx.Client`, `AsyncClient`, `TestClient`.
fn is_client_type_node(src: &[u8], node: Node<'_>) -> bool {
    match node.kind() {
        "identifier" => {
            let name = node_text(src, node);
            matches!(name, "TestClient" | "AsyncClient") || name.ends_with("TestClient")
        }
        "attribute" => {
            let obj = node
                .child_by_field_name("object")
                .map(|n| node_text(src, n))
                .unwrap_or("");
            let attr = node
                .child_by_field_name("attribute")
                .map(|n| node_text(src, n))
                .unwrap_or("");
            obj == "httpx" && matches!(attr, "Client" | "AsyncClient")
        }
        _ => false,
    }
}

fn is_test_client_ctor(src: &[u8], node: Node<'_>) -> bool {
    if node.kind() != "call" {
        return false;
    }
    let func = match node.child_by_field_name("function") {
        Some(f) => f,
        None => return false,
    };
    match func.kind() {
        "identifier" => node_text(src, func) == "TestClient",
        "attribute" => {
            let obj_name = func
                .child_by_field_name("object")
                .map(|o| node_text(src, o))
                .unwrap_or("");
            let attr_name = func
                .child_by_field_name("attribute")
                .map(|a| node_text(src, a))
                .unwrap_or("");
            obj_name == "httpx" && (attr_name == "Client" || attr_name == "AsyncClient")
        }
        _ => false,
    }
}

// ── Pass 2: collect HTTP calls on known client names ─────────────────────────

fn collect_http_calls(
    src: &[u8],
    node: Node<'_>,
    client_names: &HashSet<String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    if node.kind() == "call"
        && let Some(call) = extract_http_call(src, node, client_names, enc)
    {
        facts.client_calls.push(call);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_http_calls(src, child, client_names, facts, enc);
    }
}

fn extract_http_call(
    src: &[u8],
    node: Node<'_>,
    client_names: &HashSet<String>,
    enc: crate::offset::Encoding,
) -> Option<ClientCall> {
    let func = node.child_by_field_name("function")?;
    if func.kind() != "attribute" {
        return None;
    }
    let obj = func.child_by_field_name("object")?;
    if obj.kind() != "identifier" {
        return None;
    }
    let obj_name = node_text(src, obj);
    if !client_names.contains(obj_name) {
        return None;
    }
    let verb = func
        .child_by_field_name("attribute")
        .map(|a| node_text(src, a))
        .unwrap_or("");
    let method = verb_to_method(verb)?;

    let args = node.child_by_field_name("arguments")?;
    let (path, path_range, is_prefix, path_depth, fstring_segments) =
        first_arg_path(src, args, enc)?;

    Some(ClientCall {
        fixture_name: obj_name.to_owned(),
        method,
        path,
        is_prefix,
        path_depth,
        fstring_segments,
        range: range_from_node(node, src, enc),
        path_range,
    })
}

fn verb_to_method(verb: &str) -> Option<Method> {
    match verb {
        "get" => Some(Method::Get),
        "post" => Some(Method::Post),
        "put" => Some(Method::Put),
        "delete" => Some(Method::Delete),
        "patch" => Some(Method::Patch),
        "options" => Some(Method::Options),
        "head" => Some(Method::Head),
        "websocket_connect" => Some(Method::WebSocket),
        _ => None,
    }
}

// ── Shared helpers ────────────────────────────────────────────────────────────

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

/// Extract the path (and is_prefix flag) from the first positional argument of a client call.
///
/// Returns `(path, range, is_prefix)`:
/// - Plain strings: exact path, `is_prefix=false`.
/// - F-strings with interpolations: static prefix before `{`, `is_prefix=true`.
/// - `"…{}…".format(…)`: prefix before `{}`, `is_prefix=true`.
/// - `"…%s…" % …`: prefix before `%s`/`%d`/etc., `is_prefix=true`.
/// - `"a" + "b"`: concatenated exact path, `is_prefix=false`.
/// - `"a" + variable`: prefix = "a", `is_prefix=true`.
///
/// Returns `(path, range, is_prefix, path_depth, fstring_segments)`.
/// `path_depth` is the total segment count when `is_prefix` is true.
/// `fstring_segments` is populated for f-strings to enable segment-by-segment matching.
fn first_arg_path(
    src: &[u8],
    args: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<(
    String,
    Range,
    bool,
    Option<usize>,
    Option<Vec<FStringSegment>>,
)> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            "string" => return extract_path_from_string(src, child, enc),
            "call" => {
                let r = extract_path_from_format_call(src, child, enc)?;
                return Some((r.0, r.1, r.2, r.3, None));
            }
            "binary_operator" => {
                let r = extract_path_from_binary_op(src, child, enc)?;
                return Some((r.0, r.1, r.2, r.3, None));
            }
            _ => continue,
        }
    }
    None
}

fn extract_path_from_string(
    src: &[u8],
    node: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<(
    String,
    Range,
    bool,
    Option<usize>,
    Option<Vec<FStringSegment>>,
)> {
    // Detect f-string by presence of an `interpolation` child node.
    let has_interpolation = {
        let mut c = node.walk();
        node.children(&mut c).any(|n| n.kind() == "interpolation")
    };
    if has_interpolation {
        let prefix = string_content_before_interpolation(src, node);
        if prefix.is_empty() {
            return None;
        }
        let depth = count_all_static_slashes_in_fstring(src, node) + 1;
        let segments = extract_fstring_segments(src, node);
        // path_range covers only the static prefix before the first interpolation.
        // This prevents goto/hover on interpolation expressions (e.g. `{uuid.uuid4()}`)
        // from hijacking normal goto-definition.
        let prefix_range = fstring_prefix_range(src, node, enc);
        Some((prefix, prefix_range, true, Some(depth), Some(segments)))
    } else {
        let raw = node_text(src, node);
        let text = unquote(raw);
        let content_range = string_content_range(node, raw, src, enc);
        Some((text, content_range, false, None, None))
    }
}

/// Extract ordered segments from an f-string node for segment-by-segment trie matching.
/// Alternates between `string_content` (Literal) and `interpolation` (Wildcard) children.
fn extract_fstring_segments(src: &[u8], node: Node<'_>) -> Vec<FStringSegment> {
    let mut segments = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "string_content" => {
                let text = node_text(src, child).to_owned();
                if !text.is_empty() {
                    segments.push(FStringSegment::Literal(text));
                }
            }
            "interpolation" => {
                segments.push(FStringSegment::Wildcard);
            }
            _ => {}
        }
    }
    segments
}

/// Collect string_content text up to the first `interpolation` child.
fn string_content_before_interpolation(src: &[u8], node: Node<'_>) -> String {
    let mut buf = String::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "string_content" => buf.push_str(node_text(src, child)),
            "interpolation" => break,
            _ => {}
        }
    }
    buf
}

/// Count slashes across ALL `string_content` children of an f-string node.
/// Used to determine the total path segment depth including segments after interpolations.
fn count_all_static_slashes_in_fstring(src: &[u8], node: Node<'_>) -> usize {
    let mut count = 0usize;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            count += node_text(src, child).chars().filter(|&c| c == '/').count();
        }
    }
    count
}

/// Compute the range covering the f-string from its start to the end of the first
/// `string_content` node (the static prefix before the first interpolation).
/// This is used as `path_range` so that goto/hover does NOT fire on `{interpolation}` nodes.
fn fstring_prefix_range(src: &[u8], node: Node<'_>, enc: crate::offset::Encoding) -> Range {
    let string_start = range_from_node(node, src, enc).start;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            let end = range_from_node(child, src, enc).end;
            return Range {
                start: string_start,
                end,
            };
        }
        if child.kind() == "interpolation" {
            break;
        }
    }
    // Fallback: zero-width range so goto does not fire on any interpolation content.
    Range {
        start: string_start,
        end: string_start,
    }
}

/// `"/path/{}".format(x)` → extract prefix before `{}` from the string object.
fn extract_path_from_format_call(
    src: &[u8],
    node: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<(String, Range, bool, Option<usize>)> {
    let func = node.child_by_field_name("function")?;
    if func.kind() != "attribute" {
        return None;
    }
    let attr_name = func
        .child_by_field_name("attribute")
        .map(|n| node_text(src, n))?;
    if attr_name != "format" {
        return None;
    }
    let obj = func.child_by_field_name("object")?;
    if obj.kind() != "string" {
        return None;
    }
    let content = string_flat_content(src, obj);
    if content.is_empty() {
        return None;
    }
    let range = range_from_node(node, src, enc);
    match content.find("{}") {
        None => Some((content, range, false, None)),
        Some(0) => None,
        Some(idx) => {
            let depth = content.chars().filter(|&c| c == '/').count() + 1;
            Some((content[..idx].to_owned(), range, true, Some(depth)))
        }
    }
}

/// `"/path/%s" % x` → extract prefix before the % placeholder from the left string.
fn extract_path_from_binary_op(
    src: &[u8],
    node: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<(String, Range, bool, Option<usize>)> {
    let op = {
        let mut c = node.walk();
        node.children(&mut c).find(|n| !n.is_named())
    };
    let op_text = op.map(|n| node_text(src, n)).unwrap_or("");

    match op_text {
        "%" => {
            // left operand must be a string template
            let left = node.child_by_field_name("left")?;
            if left.kind() != "string" {
                return None;
            }
            let content = string_flat_content(src, left);
            // Find first % placeholder (%s, %d, %r, %f, etc.)
            let prefix = take_before_percent_placeholder(&content);
            if prefix.is_empty() {
                return None;
            }
            let depth = content.chars().filter(|&c| c == '/').count() + 1;
            Some((prefix, range_from_node(node, src, enc), true, Some(depth)))
        }
        "+" => {
            // Collect leading string literals; stop at first non-string operand.
            concat_string_operands(src, node, enc)
        }
        _ => None,
    }
}

/// For `"a" + "b" + …`: collect all literal pieces. If all are strings, return exact match.
/// If the chain ends with a non-string, return the prefix so far as `is_prefix=true`.
fn concat_string_operands(
    src: &[u8],
    node: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<(String, Range, bool, Option<usize>)> {
    let range = range_from_node(node, src, enc);
    let mut buf = String::new();
    let mut all_literal = true;
    collect_plus_parts(src, node, &mut buf, &mut all_literal);
    if buf.is_empty() {
        None
    } else {
        // depth is None for + concat: can't infer total depth without knowing what the variable adds
        Some((buf, range, !all_literal, None))
    }
}

fn collect_plus_parts(src: &[u8], node: Node<'_>, buf: &mut String, all_literal: &mut bool) {
    // Recursively walk left-associative + chains: ((a + b) + c) etc.
    if node.kind() == "binary_operator" {
        let op = {
            let mut c = node.walk();
            node.children(&mut c).find(|n| !n.is_named())
        };
        if op.map(|n| node_text(src, n)) == Some("+") {
            if let Some(left) = node.child_by_field_name("left") {
                collect_plus_parts(src, left, buf, all_literal);
            }
            if let Some(right) = node.child_by_field_name("right") {
                collect_plus_parts(src, right, buf, all_literal);
            }
            return;
        }
    }
    if node.kind() == "string" {
        buf.push_str(&string_flat_content(src, node));
    } else {
        *all_literal = false;
    }
}

/// Get the flat text content of a plain string's `string_content` children.
fn string_flat_content(src: &[u8], node: Node<'_>) -> String {
    let mut buf = String::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_content" {
            buf.push_str(node_text(src, child));
        }
    }
    buf
}

fn take_before_percent_placeholder(s: &str) -> String {
    // Find %[flags][width][.prec]type  —  simplest: look for % followed by non-% char
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'%'
            && bytes
                .get(i + 1)
                .copied()
                .map(|c| c != b'%')
                .unwrap_or(false)
        {
            return s[..i].to_owned();
        }
    }
    s.to_owned()
}

/// Compute the range of the string content (excluding quotes and prefix chars).
fn string_content_range(
    node: Node<'_>,
    raw: &str,
    src: &[u8],
    enc: crate::offset::Encoding,
) -> Range {
    let base = range_from_node(node, src, enc);
    let no_prefix = raw.trim_start_matches(['r', 'b', 'R', 'B']);
    let prefix_extra = (raw.len() - no_prefix.len()) as u32;
    let (open_len, close_len): (u32, u32) =
        if no_prefix.starts_with("\"\"\"") || no_prefix.starts_with("'''") {
            (3, 3)
        } else {
            (1, 1)
        };
    Range {
        start: Position::new(
            base.start.line,
            base.start.character + prefix_extra + open_len,
        ),
        end: Position::new(base.end.line, base.end.character - close_len),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Uri;

    fn parse(src: &[u8]) -> tree_sitter::Tree {
        crate::parsing::parse_file(src)
    }

    fn extract_test(src: &str, fixtures: &[&str]) -> FileFacts {
        let bytes = src.as_bytes();
        let tree = parse(bytes);
        let uri: Uri = "file:///test_api.py".parse().unwrap();
        let mut facts = FileFacts::new(uri);
        let fixture_strs: Vec<String> = fixtures.iter().map(|s| s.to_string()).collect();
        extract(
            bytes,
            &tree,
            &mut facts,
            true,
            &fixture_strs,
            crate::offset::Encoding::Utf8,
        );
        facts
    }

    fn extract_non_test(src: &str, fixtures: &[&str]) -> FileFacts {
        let bytes = src.as_bytes();
        let tree = parse(bytes);
        let uri: Uri = "file:///api.py".parse().unwrap();
        let mut facts = FileFacts::new(uri);
        let fixture_strs: Vec<String> = fixtures.iter().map(|s| s.to_string()).collect();
        extract(
            bytes,
            &tree,
            &mut facts,
            false,
            &fixture_strs,
            crate::offset::Encoding::Utf8,
        );
        facts
    }

    #[test]
    fn no_output_in_non_test_file() {
        let facts = extract_non_test("client.get('/users')", &["client"]);
        assert!(facts.client_calls.is_empty());
    }

    #[test]
    fn fixture_name_in_config_recognized() {
        let facts = extract_test("client.get('/users')", &["client"]);
        assert_eq!(facts.client_calls.len(), 1);
        assert_eq!(facts.client_calls[0].method, Method::Get);
        assert_eq!(facts.client_calls[0].path, "/users");
        assert_eq!(facts.client_calls[0].fixture_name, "client");
    }

    #[test]
    fn test_client_assignment_discovered() {
        let src = "client = TestClient(app)\nresp = client.post('/items')";
        let facts = extract_test(src, &[]);
        assert_eq!(facts.client_calls.len(), 1);
        assert_eq!(facts.client_calls[0].method, Method::Post);
        assert_eq!(facts.client_calls[0].path, "/items");
    }

    #[test]
    fn httpx_client_assignment_discovered() {
        let src = "c = httpx.Client(base_url='http://test')\nresp = c.get('/ping')";
        let facts = extract_test(src, &[]);
        assert_eq!(facts.client_calls.len(), 1);
        assert_eq!(facts.client_calls[0].method, Method::Get);
        assert_eq!(facts.client_calls[0].path, "/ping");
    }

    #[test]
    fn httpx_async_client_assignment_discovered() {
        let src = "ac = httpx.AsyncClient()\nresp = ac.delete('/item/1')";
        let facts = extract_test(src, &[]);
        assert_eq!(facts.client_calls.len(), 1);
        assert_eq!(facts.client_calls[0].method, Method::Delete);
        assert_eq!(facts.client_calls[0].path, "/item/1");
    }

    #[test]
    fn all_http_verbs_extracted() {
        let src = "\
client = TestClient(app)
client.get('/a')
client.post('/b')
client.put('/c')
client.patch('/d')
client.delete('/e')
client.options('/f')
client.head('/g')
client.websocket_connect('/ws')
";
        let facts = extract_test(src, &[]);
        assert_eq!(facts.client_calls.len(), 8);
        let methods: Vec<_> = facts.client_calls.iter().map(|c| &c.method).collect();
        assert!(methods.contains(&&Method::Get));
        assert!(methods.contains(&&Method::Post));
        assert!(methods.contains(&&Method::Put));
        assert!(methods.contains(&&Method::Patch));
        assert!(methods.contains(&&Method::Delete));
        assert!(methods.contains(&&Method::Options));
        assert!(methods.contains(&&Method::Head));
        assert!(methods.contains(&&Method::WebSocket));
    }

    #[test]
    fn fstring_extracts_static_prefix() {
        let src = "client.post(f'/v1/contracts/private/{contract_id}/upload-signed')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "f-string must produce a client call with static prefix"
        );
        assert_eq!(facts.client_calls[0].path, "/v1/contracts/private/");
        assert!(
            facts.client_calls[0].is_prefix,
            "f-string with interpolation must be prefix-only"
        );
    }

    #[test]
    fn fstring_no_interpolation_is_exact() {
        let src = "client.get(f'/ws')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "f-string with no interpolation is an exact path"
        );
        assert_eq!(facts.client_calls[0].path, "/ws");
        assert!(!facts.client_calls[0].is_prefix);
    }

    #[test]
    fn rf_fstring_extracts_static_prefix() {
        let src = "client.get(rf'/users/{user_id}')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "rf-string with interpolation must extract prefix"
        );
        assert_eq!(facts.client_calls[0].path, "/users/");
        assert!(facts.client_calls[0].is_prefix);
    }

    #[test]
    fn format_call_extracts_prefix() {
        let src = r#"client.post("/v1/items/{}".format(item_id))"#;
        let facts = extract_test(src, &["client"]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            ".format() string must produce prefix call"
        );
        assert_eq!(facts.client_calls[0].path, "/v1/items/");
        assert!(facts.client_calls[0].is_prefix);
    }

    #[test]
    fn percent_format_extracts_prefix() {
        let src = r#"client.get("/v1/items/%s" % item_id)"#;
        let facts = extract_test(src, &["client"]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "%-format string must produce prefix call"
        );
        assert_eq!(facts.client_calls[0].path, "/v1/items/");
        assert!(facts.client_calls[0].is_prefix);
    }

    #[test]
    fn plus_concat_all_strings_is_exact() {
        let src = r#"client.get("/v1/" + "items")"#;
        let facts = extract_test(src, &["client"]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "concat of two literals is exact"
        );
        assert_eq!(facts.client_calls[0].path, "/v1/items");
        assert!(!facts.client_calls[0].is_prefix);
    }

    #[test]
    fn plus_concat_with_variable_is_prefix() {
        let src = r#"client.get("/v1/items/" + item_id)"#;
        let facts = extract_test(src, &["client"]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "concat with variable must produce prefix call"
        );
        assert_eq!(facts.client_calls[0].path, "/v1/items/");
        assert!(facts.client_calls[0].is_prefix);
    }

    #[test]
    fn await_async_client_call_extracted() {
        // `await ac.get("/path")` — the call node is still a child of await_expression
        let src = "async def test_it(ac):\n    resp = await ac.get('/items')";
        let facts = extract_test(src, &["ac"]);
        assert_eq!(facts.client_calls.len(), 1);
        assert_eq!(facts.client_calls[0].method, Method::Get);
        assert_eq!(facts.client_calls[0].path, "/items");
    }

    #[test]
    fn unknown_object_not_extracted() {
        let src = "requests.get('/users')";
        let facts = extract_test(src, &[]);
        assert!(facts.client_calls.is_empty());
    }

    #[test]
    fn keyword_arg_path_not_extracted() {
        // Path passed as keyword arg, not positional — not currently supported
        let src = "client.get(url='/users')";
        let facts = extract_test(src, &["client"]);
        assert!(facts.client_calls.is_empty());
    }

    #[test]
    fn multiple_clients_both_tracked() {
        let src = "\
c1 = TestClient(app)
c2 = httpx.Client()
c1.get('/a')
c2.post('/b')
";
        let facts = extract_test(src, &[]);
        assert_eq!(facts.client_calls.len(), 2);
    }

    #[test]
    fn fixture_name_and_assignment_combined() {
        // 'client' is from config fixtures; 'other' from local assignment
        let src = "\
other = TestClient(app)
client.get('/a')
other.post('/b')
";
        let facts = extract_test(src, &["client"]);
        assert_eq!(facts.client_calls.len(), 2);
    }

    #[test]
    fn range_covers_call_expression() {
        use tower_lsp_server::ls_types::Position;
        let src = "client.get('/users')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(facts.client_calls.len(), 1);
        let range = facts.client_calls[0].range;
        assert_eq!(range.start, Position::new(0, 0));
        assert!(range.end.character > range.start.character);
    }

    #[test]
    fn websocket_connect_method_is_websocket() {
        let src = "client.websocket_connect('/ws')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(facts.client_calls.len(), 1);
        assert_eq!(facts.client_calls[0].method, Method::WebSocket);
        assert_eq!(facts.client_calls[0].path, "/ws");
    }

    #[test]
    fn default_client_fixtures_async_client() {
        // async_client is in the default client_fixtures list
        let src = "async_client.post('/items')";
        let facts = extract_test(src, &["client", "async_client"]);
        assert_eq!(facts.client_calls.len(), 1);
        assert_eq!(facts.client_calls[0].method, Method::Post);
        assert_eq!(facts.client_calls[0].fixture_name, "async_client");
    }

    #[test]
    fn client_detected_via_httpx_attribute_annotation() {
        let src = "async def test_create(internal_client: httpx.AsyncClient):\n    resp = await internal_client.post('/v1/items')";
        let facts = extract_test(src, &[]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "typed parameter with httpx.AsyncClient must be detected"
        );
        assert_eq!(facts.client_calls[0].fixture_name, "internal_client");
        assert_eq!(facts.client_calls[0].method, Method::Post);
        assert_eq!(facts.client_calls[0].path, "/v1/items");
    }

    #[test]
    fn client_detected_via_bare_async_client_annotation() {
        let src = "async def test_it(ac: AsyncClient):\n    ac.get('/health')";
        let facts = extract_test(src, &[]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "typed parameter with bare AsyncClient must be detected"
        );
        assert_eq!(facts.client_calls[0].fixture_name, "ac");
        assert_eq!(facts.client_calls[0].method, Method::Get);
    }

    #[test]
    fn percent_format_no_static_prefix_is_skipped() {
        // "%s" % x has no static prefix — should not produce a call (would match all routes)
        let src = r#"client.get("%s" % item_id)"#;
        let facts = extract_test(src, &["client"]);
        assert!(
            facts.client_calls.is_empty(),
            "%-format with no static prefix must be skipped"
        );
    }

    #[test]
    fn format_call_no_static_prefix_is_skipped() {
        // "{}".format(x) has no static prefix — should not produce a call (would match all routes)
        let src = r#"client.get("{}".format(item_id))"#;
        let facts = extract_test(src, &["client"]);
        assert!(
            facts.client_calls.is_empty(),
            ".format() with no static prefix must be skipped"
        );
    }

    #[test]
    fn custom_client_subclass_recognized_via_type_annotation() {
        // AsyncTestClient is a project subclass of httpx.AsyncClient.
        // Names ending in "Client" must be treated as test-client types.
        let src = "async def test_it(ac: AsyncTestClient):\n    resp = await ac.get('/health')\n";
        let facts = extract_test(src, &[]);
        assert_eq!(
            facts.client_calls.len(),
            1,
            "AsyncTestClient annotation must detect client"
        );
        assert_eq!(facts.client_calls[0].path, "/health");
        assert_eq!(facts.client_calls[0].fixture_name, "ac");
    }

    #[test]
    fn fstring_multi_segment_records_path_depth() {
        // f"/v1/projects/{project.id}/installers/{installer.id}"
        // has 5 slashes across all static parts → depth = 6
        let src = "client.delete(f'/v1/projects/{project.id}/installers/{installer.id}')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(facts.client_calls.len(), 1);
        assert!(facts.client_calls[0].is_prefix);
        assert_eq!(facts.client_calls[0].path, "/v1/projects/");
        assert_eq!(
            facts.client_calls[0].path_depth,
            Some(6),
            "depth must count slashes across all static content (5 slashes → 6 segments)"
        );
    }

    #[test]
    fn fstring_two_wildcards_records_segments() {
        // f"/api/books/{book_id}/authors/{author_id}"
        let src = "client.get(f'/api/books/{book_id}/authors/{author_id}')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(facts.client_calls.len(), 1);
        let call = &facts.client_calls[0];
        assert!(call.is_prefix);
        let segs = call
            .fstring_segments
            .as_ref()
            .expect("fstring_segments must be set for f-strings");
        assert_eq!(
            segs,
            &[
                crate::state::FStringSegment::Literal("/api/books/".to_owned()),
                crate::state::FStringSegment::Wildcard,
                crate::state::FStringSegment::Literal("/authors/".to_owned()),
                crate::state::FStringSegment::Wildcard,
            ]
        );
    }

    #[test]
    fn plain_string_has_no_fstring_segments() {
        let src = "client.get('/api/books/1')";
        let facts = extract_test(src, &["client"]);
        assert_eq!(facts.client_calls.len(), 1);
        assert!(facts.client_calls[0].fstring_segments.is_none());
    }

    #[test]
    fn path_range_covers_content_without_quotes() {
        use tower_lsp_server::ls_types::Position;
        // Source: `client.get("/api/items")` — the path `/api/items` is chars 12..22
        let src = b"client.get(\"/api/items\")";
        let facts = extract_test(std::str::from_utf8(src).unwrap(), &["client"]);
        assert_eq!(facts.client_calls.len(), 1);
        let path_range = facts.client_calls[0].path_range;
        // Opening quote is at column 11, content starts at column 12
        assert_eq!(path_range.start, Position::new(0, 12));
        // Content "/api/items" is 10 chars; closing quote at column 22
        assert_eq!(path_range.end, Position::new(0, 22));
    }
}
