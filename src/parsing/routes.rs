use std::collections::HashMap;

use tower_lsp_server::ls_types::{Range, Uri};
use tree_sitter::{Node, Tree};

use super::unquote;
use crate::state::{
    AppDecl, FileFacts, IncludeCall, Method, PrefixValue, RouteFact, RouterDecl, range_from_node,
};

const ROUTE_METHODS: &[&str] = &[
    "get",
    "post",
    "put",
    "delete",
    "patch",
    "options",
    "head",
    "trace",
    "websocket",
    "api_route",
];

const APP_CTORS: &[&str] = &["FastAPI", "Starlette"];
const ROUTER_CTORS: &[&str] = &["APIRouter"];

pub fn extract(src: &[u8], tree: &Tree, uri: &Uri, enc: crate::offset::Encoding) -> FileFacts {
    let mut facts = FileFacts::new(uri.clone());
    let root = tree.root_node();

    // First pass: collect module-level string constants for prefix resolution
    let constants = collect_module_constants(src, root);

    // Second pass: extract facts
    walk(src, root, &constants, &mut facts, enc);
    facts
}

// ── String constant collection (all scopes) ──────────────────────────────────

/// Collect all `NAME = "string"` / `NAME: type = "string"` assignments in the
/// entire tree. This covers both module-level constants and function-local ones
/// (e.g., `PREFIX = "/v2"` inside a factory), enabling prefix resolution at
/// any nesting depth (REQ-ROUTE-12). When the same name is assigned different
/// values at different scopes, last-write wins — an acceptable approximation
/// for the common single-value-per-name case.
fn collect_module_constants(src: &[u8], root: Node<'_>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    collect_constants_recursive(src, root, &mut map);
    map
}

fn collect_constants_recursive(src: &[u8], node: Node<'_>, map: &mut HashMap<String, String>) {
    match node.kind() {
        "assignment" => {
            maybe_collect_constant(src, node, map);
            // Still recurse — an assignment's RHS could contain lambdas etc.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_constants_recursive(src, child, map);
            }
        }
        "expression_statement" => {
            if let Some(inner) = node.child(0)
                && inner.kind() == "assignment"
            {
                maybe_collect_constant(src, inner, map);
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_constants_recursive(src, child, map);
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                collect_constants_recursive(src, child, map);
            }
        }
    }
}

fn maybe_collect_constant(src: &[u8], assign: Node<'_>, map: &mut HashMap<String, String>) {
    // Plain:      PREFIX = "/api"           → left=identifier, right=string
    // Annotated:  PREFIX: str = "/api"      → left=identifier, type=..., right=string
    let lhs = match assign.child_by_field_name("left") {
        Some(n) if n.kind() == "identifier" => n,
        _ => return,
    };
    let rhs = match assign.child_by_field_name("right") {
        Some(n) => n,
        None => return,
    };
    if let Some(s) = string_value(src, rhs) {
        map.insert(node_text(src, lhs).to_owned(), s);
    }
}

// ── Main tree walker ──────────────────────────────────────────────────────────

fn walk(
    src: &[u8],
    node: Node<'_>,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    match node.kind() {
        "decorated_definition" => {
            extract_decorated_definition(src, node, consts, facts, enc);
            // Don't recurse into it — the function body may contain nested defs
        }
        "assignment" => {
            extract_assignment(src, node, consts, facts, enc);
            recurse(src, node, consts, facts, enc);
        }
        "expression_statement" => {
            if let Some(inner) = node.child(0) {
                extract_expression(src, inner, consts, facts, enc);
            }
            recurse(src, node, consts, facts, enc);
        }
        _ => {
            recurse(src, node, consts, facts, enc);
        }
    }
}

fn recurse(
    src: &[u8],
    node: Node<'_>,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(src, child, consts, facts, enc);
    }
}

// ── Decorated function: route decorators ─────────────────────────────────────

fn extract_decorated_definition(
    src: &[u8],
    node: Node<'_>,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    // Find the function_definition child
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

    let handler_name = match func.child_by_field_name("name") {
        Some(n) => node_text(src, n).to_owned(),
        None => return,
    };
    let handler_range = range_from_node(func, src, enc);
    let (handler_params, handler_param_ranges, params_insert_pos, handler_has_splat_args) =
        extract_handler_params(src, func, enc);
    let return_annotation = extract_return_annotation(src, func);

    for decorator in decorators {
        // decorator → '@' expr  (the expr is the second child)
        let expr = match decorator.child(1) {
            Some(e) => e,
            None => continue,
        };

        if let Some(mut route) = try_route_decorator(
            src,
            expr,
            consts,
            &handler_name,
            handler_range,
            handler_params.clone(),
            handler_param_ranges.clone(),
            params_insert_pos,
            handler_has_splat_args,
            enc,
        ) {
            route.return_annotation = return_annotation.clone();
            facts.routes.push(route);
        }
    }
}

/// Extract the bare identifier from the `-> T` return annotation of a function definition.
/// Returns None for subscripts, attributes, and Python builtin type names.
fn extract_return_annotation(src: &[u8], func_node: Node<'_>) -> Option<String> {
    let return_type = func_node.child_by_field_name("return_type")?;
    // return_type yields a "type" wrapper node; its named child is the actual expression.
    let type_node = return_type.named_child(0)?;
    if type_node.kind() != "identifier" {
        return None;
    }
    let name = node_text(src, type_node);
    // Suppress Python builtins and typing utilities that are never Pydantic models.
    const BUILTINS: &[&str] = &[
        "None",
        "str",
        "int",
        "float",
        "bool",
        "bytes",
        "bytearray",
        "list",
        "dict",
        "set",
        "tuple",
        "Any",
        "Optional",
        "List",
        "Dict",
        "Set",
        "Tuple",
        "Union",
        "Type",
        "ClassVar",
        "Final",
        "Literal",
        "Generator",
        "AsyncGenerator",
        "Iterator",
        "AsyncIterator",
        "Callable",
        "Awaitable",
        "Coroutine",
        "JSONResponse",
        "Response",
        "HTMLResponse",
        "PlainTextResponse",
        "RedirectResponse",
        "StreamingResponse",
        "FileResponse",
    ];
    if BUILTINS.contains(&name) {
        return None;
    }
    Some(name.to_owned())
}

/// Extract parameter names and splat-arg presence from a function_definition node.
/// Skips `self` and `cls`. Returns (names, has_splat).
/// Returns (names, ranges, params_insert_pos, has_splat_args).
/// `ranges` is aligned with `names` — `ranges[i]` is the source range of `names[i]`.
/// `params_insert_pos` is the position just before the closing `)`, for inserting new params.
fn extract_handler_params(
    src: &[u8],
    func_node: Node<'_>,
    enc: crate::offset::Encoding,
) -> (
    Vec<String>,
    Vec<Range>,
    Option<tower_lsp_server::ls_types::Position>,
    bool,
) {
    let params_node = match func_node.child_by_field_name("parameters") {
        Some(p) => p,
        None => return (vec![], vec![], None, false),
    };

    let mut names = vec![];
    let mut ranges = vec![];
    let mut has_splat = false;

    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(src, child);
                if name != "self" && name != "cls" {
                    names.push(name.to_owned());
                    ranges.push(range_from_node(child, src, enc));
                }
            }
            "typed_parameter" | "default_parameter" | "typed_default_parameter" => {
                if let Some(first) = child.child(0)
                    && first.kind() == "identifier"
                {
                    let name = node_text(src, first);
                    if name != "self" && name != "cls" {
                        names.push(name.to_owned());
                        ranges.push(range_from_node(first, src, enc));
                    }
                }
            }
            "list_splat_pattern" | "dictionary_splat_pattern" => {
                has_splat = true;
            }
            _ => {}
        }
    }

    // Position just before the closing `)` — insert point for new parameters.
    let end = params_node.end_position();
    let insert_pos = if end.column > 0 {
        Some(tower_lsp_server::ls_types::Position::new(
            end.row as u32,
            (end.column - 1) as u32,
        ))
    } else {
        None
    };

    (names, ranges, insert_pos, has_splat)
}

#[allow(clippy::too_many_arguments)]
fn try_route_decorator(
    src: &[u8],
    expr: Node<'_>,
    consts: &HashMap<String, String>,
    handler_name: &str,
    handler_range: Range,
    handler_params: Vec<String>,
    handler_param_ranges: Vec<Range>,
    params_insert_pos: Option<tower_lsp_server::ls_types::Position>,
    handler_has_splat_args: bool,
    enc: crate::offset::Encoding,
) -> Option<RouteFact> {
    // @obj.method(path, ...) → expr is a call
    if expr.kind() != "call" {
        return None;
    }

    let callee = expr.child_by_field_name("function")?;
    if callee.kind() != "attribute" {
        return None;
    }

    let obj_node = callee.child_by_field_name("object")?;
    let attr_node = callee.child_by_field_name("attribute")?;

    let obj_name = node_text(src, obj_node);
    let method_name = node_text(src, attr_node);

    // @obj.api_route(path, methods=[...]) is a special case
    if method_name == "api_route" {
        let args = expr.child_by_field_name("arguments")?;
        let (path, path_range, path_quote_width) = extract_path_arg(src, args, consts, enc)?;
        let methods = extract_methods_kwarg(src, args).unwrap_or_else(|| vec![Method::Get]);
        let (response_model, response_model_range) =
            extract_kwarg_string_with_range(src, args, "response_model", enc);
        let dependencies = extract_dependencies_kwarg(src, args);
        let route_name = extract_kwarg_string(src, args, "name");
        return Some(RouteFact {
            handler_name: handler_name.to_owned(),
            handler_range,
            object_name: obj_name.to_owned(),
            methods,
            path,
            path_range,
            path_quote_width,
            response_model,
            response_model_range,
            return_annotation: None,
            status_code: extract_kwarg_u16(src, args, "status_code"),
            dependencies,
            route_name,
            handler_params,
            handler_param_ranges,
            params_insert_pos,
            handler_has_splat_args,
            handler_params_known: true,
        });
    }

    if !ROUTE_METHODS.contains(&method_name) {
        return None;
    }

    let method = match method_name {
        "get" => Method::Get,
        "post" => Method::Post,
        "put" => Method::Put,
        "delete" => Method::Delete,
        "patch" => Method::Patch,
        "options" => Method::Options,
        "head" => Method::Head,
        "trace" => Method::Trace,
        "websocket" => Method::WebSocket,
        _ => return None,
    };

    let args = expr.child_by_field_name("arguments")?;
    let (path, path_range, path_quote_width) = extract_path_arg(src, args, consts, enc)?;
    let (response_model, response_model_range) =
        extract_kwarg_string_with_range(src, args, "response_model", enc);
    let dependencies = extract_dependencies_kwarg(src, args);
    let route_name = extract_kwarg_string(src, args, "name");

    Some(RouteFact {
        handler_name: handler_name.to_owned(),
        handler_range,
        object_name: obj_name.to_owned(),
        methods: vec![method],
        path,
        path_range,
        path_quote_width,
        response_model,
        response_model_range,
        return_annotation: None,
        status_code: extract_kwarg_u16(src, args, "status_code"),
        dependencies,
        route_name,
        handler_params,
        handler_param_ranges,
        params_insert_pos,
        handler_has_splat_args,
        handler_params_known: true,
    })
}

// ── Assignments: APIRouter, FastAPI, Starlette ────────────────────────────────

fn extract_assignment(
    src: &[u8],
    node: Node<'_>,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    // lhs = rhs
    // Accept both plain identifiers (`router = APIRouter()`) and
    // `self.attr` attribute assignments (`self.router = APIRouter()`), REQ-ROUTE-13.
    let lhs = match node.child_by_field_name("left").or_else(|| node.child(0)) {
        Some(n) => n,
        None => return,
    };
    let var_name = match lhs.kind() {
        "identifier" => node_text(src, lhs).to_owned(),
        "attribute" => {
            // Only handle `self.X` — other attribute assignments are too dynamic.
            let obj = lhs.child_by_field_name("object").map(|n| node_text(src, n)).unwrap_or("");
            let attr = lhs.child_by_field_name("attribute").map(|n| node_text(src, n)).unwrap_or("");
            if obj != "self" || attr.is_empty() {
                return;
            }
            format!("self.{attr}")
        }
        _ => return,
    };
    let rhs = match node.child_by_field_name("right").or_else(|| node.child(2)) {
        Some(n) => n,
        None => return,
    };

    if rhs.kind() != "call" {
        return;
    }

    let callee = match rhs.child_by_field_name("function") {
        Some(n) => n,
        None => return,
    };
    let callee_name = match callee.kind() {
        "identifier" => node_text(src, callee),
        "attribute" => {
            // e.g. fastapi.FastAPI()
            callee
                .child_by_field_name("attribute")
                .map(|n| node_text(src, n))
                .unwrap_or("")
        }
        _ => return,
    };

    let range = range_from_node(lhs, src, enc);

    if APP_CTORS.contains(&callee_name) {
        if let Some(app_args) = rhs.child_by_field_name("arguments") {
            extract_table_routes_kwarg(src, app_args, &var_name, consts, facts, enc);
        }
        facts.apps.push(AppDecl {
            name: var_name,
            range,
        });
    } else if ROUTER_CTORS.contains(&callee_name) {
        let args = rhs.child_by_field_name("arguments").unwrap_or(rhs);
        let prefix = extract_prefix_kwarg(src, args, consts, "prefix");
        let tags = extract_string_list_kwarg(src, args, "tags");
        facts.routers.push(RouterDecl {
            name: var_name,
            prefix,
            tags,
            range,
        });
    }
}

// ── Table-style routes: routes=[Route(...), WebSocketRoute(...)] ──────────────

fn extract_table_routes_kwarg(
    src: &[u8],
    app_args: Node<'_>,
    app_name: &str,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let routes_val = match find_kwarg_node(src, app_args, "routes") {
        Some(v) => v,
        None => return,
    };
    if routes_val.kind() == "list" {
        extract_from_route_list(src, routes_val, app_name, consts, facts, enc);
    }
    // Identifier references to module-level list constants are not handled here;
    // that would require a separate list-constant collection pass.
}

fn extract_from_route_list(
    src: &[u8],
    list: Node<'_>,
    app_name: &str,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let mut cursor = list.walk();
    for child in list.children(&mut cursor) {
        if child.kind() != "call" {
            continue;
        }
        let func = match child.child_by_field_name("function") {
            Some(f) => f,
            None => continue,
        };
        let func_name = match func.kind() {
            "identifier" => node_text(src, func),
            "attribute" => func
                .child_by_field_name("attribute")
                .map(|a| node_text(src, a))
                .unwrap_or(""),
            _ => continue,
        };
        let call_args = match child.child_by_field_name("arguments") {
            Some(a) => a,
            None => continue,
        };
        match func_name {
            "Route" => facts
                .routes
                .extend(extract_route_entry(src, call_args, app_name, consts, enc)),
            "WebSocketRoute" => {
                if let Some(r) =
                    extract_websocket_route_entry(src, call_args, app_name, consts, enc)
                {
                    facts.routes.push(r);
                }
            }
            "Mount" => extract_mount_entry(src, call_args, app_name, consts, facts, enc),
            _ => {}
        }
    }
}

fn extract_route_entry(
    src: &[u8],
    args: Node<'_>,
    app_name: &str,
    consts: &HashMap<String, String>,
    enc: crate::offset::Encoding,
) -> Vec<RouteFact> {
    let (path, path_range, path_quote_width) = match extract_path_arg(src, args, consts, enc) {
        Some(p) => p,
        None => (PrefixValue::Unresolved, None, None),
    };
    let (handler_name, handler_range) = match endpoint_positional(src, args, enc) {
        Some(p) => p,
        None => return vec![],
    };
    // Starlette's runtime default is None (all methods), but the spec (REQ-STAR-01)
    // calls for GET as the LSP default — intentional, not a bug.
    let methods = extract_methods_kwarg(src, args).unwrap_or_else(|| vec![Method::Get]);
    let dependencies = extract_dependencies_kwarg(src, args);
    let route_name = extract_kwarg_string(src, args, "name");
    methods
        .into_iter()
        .map(|method| RouteFact {
            handler_name: handler_name.clone(),
            handler_range,
            object_name: app_name.to_owned(),
            methods: vec![method],
            path: path.clone(),
            path_range,
            path_quote_width,
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            status_code: None,
            dependencies: dependencies.clone(),
            route_name: route_name.clone(),
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: false,
        })
        .collect()
}

fn extract_websocket_route_entry(
    src: &[u8],
    args: Node<'_>,
    app_name: &str,
    consts: &HashMap<String, String>,
    enc: crate::offset::Encoding,
) -> Option<RouteFact> {
    let (path, path_range, path_quote_width) = match extract_path_arg(src, args, consts, enc) {
        Some(p) => p,
        None => (PrefixValue::Unresolved, None, None),
    };
    let (handler_name, handler_range) = endpoint_positional(src, args, enc)?;
    let route_name = extract_kwarg_string(src, args, "name");
    Some(RouteFact {
        handler_name,
        handler_range,
        object_name: app_name.to_owned(),
        methods: vec![Method::WebSocket],
        path,
        path_range,
        path_quote_width,
        response_model: None,
        response_model_range: None,
        return_annotation: None,
        status_code: None,
        dependencies: vec![],
        route_name,
        handler_params: vec![],
        handler_param_ranges: vec![],
        params_insert_pos: None,
        handler_has_splat_args: false,
        handler_params_known: false,
    })
}

/// Extract a `Mount(...)` table entry.
///
/// Three cases:
/// - `Mount(path, routes=[...])` → flatten nested route list with prefix prepended.
/// - `Mount(path, app=<identifier>)` → cross-app include (like include_router).
/// - `Mount(path, app=<call>)` → terminal mount record (e.g. StaticFiles).
fn extract_mount_entry(
    src: &[u8],
    args: Node<'_>,
    app_name: &str,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let (prefix, _, _) = match extract_path_arg(src, args, consts, enc) {
        Some(p) => p,
        None => (PrefixValue::Unresolved, None, None),
    };
    let mount_name = extract_kwarg_string(src, args, "name");

    // `routes=` kwarg: nested route list — flatten with prefix prepended.
    // Identifier references (routes=CONST) are not followed (would need a separate
    // list-constant collection pass); those entries are silently dropped (P4).
    if let Some(routes_val) = find_kwarg_node(src, args, "routes") {
        if routes_val.kind() == "list" {
            let before = facts.routes.len();
            extract_from_route_list(src, routes_val, app_name, consts, facts, enc);
            // Prepend mount prefix to all newly added routes.
            for route in facts.routes[before..].iter_mut() {
                route.path = prepend_prefix(&prefix, &route.path);
            }
        }
        return;
    }

    // `app=` kwarg (positional arg 2 is also checked for compatibility).
    let app_val = find_kwarg_node(src, args, "app").or_else(|| {
        let mut pos = 0usize;
        let mut cursor = args.walk();
        for child in args.children(&mut cursor) {
            match child.kind() {
                "," | "(" | ")" => {}
                "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
                _ => {
                    if pos == 1 {
                        return Some(child);
                    }
                    pos += 1;
                }
            }
        }
        None
    });

    let Some(app_val) = app_val else { return };

    match app_val.kind() {
        "identifier" => {
            // Cross-app mount: record as an include so navigation features can find the
            // sub-app. Note: path resolution for routes inside the sub-app does not
            // apply the mount prefix (the linker short-circuits on AppDecl entries), so
            // those routes resolve to their local paths only.
            let target = node_text(src, app_val).to_owned();
            facts.includes.push(IncludeCall {
                target,
                prefix,
                app_name: app_name.to_owned(),
                dependencies: vec![],
                range: range_from_node(args, src, enc),
            });
        }
        "call" => {
            // Terminal mount (StaticFiles, plain ASGI app, etc.)
            let handler_name = mount_name.clone().unwrap_or_else(|| {
                node_text(
                    src,
                    app_val.child_by_field_name("function").unwrap_or(app_val),
                )
                .to_owned()
            });
            facts.routes.push(RouteFact {
                handler_name,
                handler_range: range_from_node(app_val, src, enc),
                object_name: app_name.to_owned(),
                methods: vec![Method::Mount],
                path: prefix,
                path_range: None,
                path_quote_width: None,
                response_model: None,
                response_model_range: None,
                return_annotation: None,
                status_code: None,
                dependencies: vec![],
                route_name: mount_name,
                handler_params: vec![],
                handler_param_ranges: vec![],
                params_insert_pos: None,
                handler_has_splat_args: false,
                handler_params_known: false,
            });
        }
        _ => {}
    }
}

/// Prepend `mount_prefix` to `route_path`, collapsing doubled slashes.
fn prepend_prefix(mount_prefix: &PrefixValue, route_path: &PrefixValue) -> PrefixValue {
    match (mount_prefix, route_path) {
        (PrefixValue::Literal(pre), PrefixValue::Literal(path)) => {
            let joined = if pre.ends_with('/') && path.starts_with('/') {
                format!("{}{}", pre, &path[1..])
            } else if !pre.ends_with('/') && !path.starts_with('/') && !path.is_empty() {
                format!("{}/{}", pre, path)
            } else {
                format!("{}{}", pre, path)
            };
            PrefixValue::Literal(joined)
        }
        _ => PrefixValue::Unresolved,
    }
}

/// Returns the (name, range) of the second positional argument (the endpoint callable).
fn endpoint_positional(
    src: &[u8],
    args: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<(String, Range)> {
    let mut positional = 0usize;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "," | "(" | ")" => {}
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            _ => {
                if positional == 1 {
                    let name = match child.kind() {
                        "identifier" | "attribute" => node_text(src, child).to_owned(),
                        _ => return None,
                    };
                    return Some((name, range_from_node(child, src, enc)));
                }
                positional += 1;
            }
        }
    }
    None
}

// ── Expression statements: include_router, app.mount, app.add_route ──────────

fn extract_expression(
    src: &[u8],
    node: Node<'_>,
    consts: &HashMap<String, String>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    if node.kind() != "call" {
        return;
    }

    let callee = match node.child_by_field_name("function") {
        Some(n) => n,
        None => return,
    };

    if callee.kind() != "attribute" {
        return;
    }

    let obj_node = callee.child_by_field_name("object");
    let attr_name = callee
        .child_by_field_name("attribute")
        .map(|n| node_text(src, n))
        .unwrap_or("");

    match attr_name {
        "include_router" => {
            let args = match node.child_by_field_name("arguments") {
                Some(a) => a,
                None => return,
            };
            let target = extract_positional_text(src, args, 0);
            let prefix = extract_prefix_kwarg(src, args, consts, "prefix");
            let app_name = obj_node
                .map(|n| node_text(src, n).to_owned())
                .unwrap_or_default();
            let dependencies = extract_dependencies_kwarg(src, args);
            facts.includes.push(IncludeCall {
                target,
                prefix,
                app_name,
                dependencies,
                range: range_from_node(node, src, enc),
            });
        }
        "mount" => {
            let args = match node.child_by_field_name("arguments") {
                Some(a) => a,
                None => return,
            };
            let app_name = obj_node
                .map(|n| node_text(src, n).to_owned())
                .unwrap_or_default();
            extract_mount_entry(src, args, &app_name, consts, facts, enc);
        }
        "add_route" => {
            let args = match node.child_by_field_name("arguments") {
                Some(a) => a,
                None => return,
            };
            let app_name = obj_node
                .map(|n| node_text(src, n).to_owned())
                .unwrap_or_default();
            facts
                .routes
                .extend(extract_route_entry(src, args, &app_name, consts, enc));
        }
        _ => {}
    }
}

// ── Argument helpers ──────────────────────────────────────────────────────────

/// Returns the path value, optional source range, and optional quote-prefix width.
/// The quote-prefix width is the UTF-16 width of the string prefix+opening-quote(s)
/// (e.g. `"` → 1, `r"` → 2, `"""` → 3). `None` for constant/variable paths.
fn extract_path_arg(
    src: &[u8],
    args: Node<'_>,
    consts: &HashMap<String, String>,
    enc: crate::offset::Encoding,
) -> Option<(PrefixValue, Option<Range>, Option<u32>)> {
    // First positional argument
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "string" => {
                let raw = node_text(src, child);
                let range = Some(range_from_node(child, src, enc));
                let qw = Some(string_quote_prefix_width(raw));
                if is_fstring(raw) {
                    return Some((PrefixValue::Unresolved, range, qw));
                }
                return Some((PrefixValue::Literal(unquote(raw)), range, qw));
            }
            "identifier" => {
                let name = node_text(src, child);
                if let Some(val) = consts.get(name) {
                    return Some((PrefixValue::Literal(val.clone()), None, None));
                }
                return Some((PrefixValue::Unresolved, None, None));
            }
            "(" | ")" | "," => {}
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            _ => {}
        }
    }
    Some((PrefixValue::Unresolved, None, None))
}

/// Return the UTF-16 width of a Python string literal's prefix + opening quote(s).
/// e.g. `"` → 1, `r"` → 2, `"""` → 3, `r"""` → 4.
fn string_quote_prefix_width(raw: &str) -> u32 {
    let s = raw.trim();
    let after_prefix = s.trim_start_matches(['f', 'r', 'b', 'u', 'F', 'R', 'B', 'U']);
    let prefix_len = s.len() - after_prefix.len();
    let quote_len = if after_prefix.starts_with("\"\"\"") || after_prefix.starts_with("'''") {
        3
    } else {
        1
    };
    (prefix_len + quote_len) as u32
}

fn extract_prefix_kwarg(
    src: &[u8],
    args: Node<'_>,
    consts: &HashMap<String, String>,
    kwarg: &str,
) -> PrefixValue {
    if let Some(val) = find_kwarg_node(src, args, kwarg) {
        if let Some(s) = string_value(src, val) {
            return PrefixValue::Literal(s);
        }
        if val.kind() == "identifier" {
            let name = node_text(src, val);
            if let Some(s) = consts.get(name) {
                return PrefixValue::Literal(s.clone());
            }
        }
        return PrefixValue::Unresolved;
    }
    PrefixValue::Literal(String::new())
}

fn extract_kwarg_string(src: &[u8], args: Node<'_>, kwarg: &str) -> Option<String> {
    let val = find_kwarg_node(src, args, kwarg)?;
    match val.kind() {
        "identifier" => Some(node_text(src, val).to_owned()),
        "attribute" => Some(node_text(src, val).to_owned()),
        _ => string_value(src, val),
    }
}

fn extract_kwarg_string_with_range(
    src: &[u8],
    args: Node<'_>,
    kwarg: &str,
    enc: crate::offset::Encoding,
) -> (Option<String>, Option<Range>) {
    let Some(val) = find_kwarg_node(src, args, kwarg) else {
        return (None, None);
    };
    let range = Some(range_from_node(val, src, enc));
    let name = match val.kind() {
        "identifier" => Some(node_text(src, val).to_owned()),
        "attribute" => Some(node_text(src, val).to_owned()),
        _ => string_value(src, val),
    };
    (name, range)
}

fn extract_kwarg_u16(src: &[u8], args: Node<'_>, kwarg: &str) -> Option<u16> {
    let val = find_kwarg_node(src, args, kwarg)?;
    node_text(src, val).parse().ok()
}

fn extract_methods_kwarg(src: &[u8], args: Node<'_>) -> Option<Vec<Method>> {
    let val = find_kwarg_node(src, args, "methods")?;
    // val should be a list like ["GET", "POST"]
    if val.kind() != "list" {
        return None;
    }
    let mut methods = vec![];
    let mut cursor = val.walk();
    for child in val.children(&mut cursor) {
        if let Some(s) = string_value(src, child) {
            match s.to_uppercase().as_str() {
                "GET" => methods.push(Method::Get),
                "POST" => methods.push(Method::Post),
                "PUT" => methods.push(Method::Put),
                "DELETE" => methods.push(Method::Delete),
                "PATCH" => methods.push(Method::Patch),
                "OPTIONS" => methods.push(Method::Options),
                "HEAD" => methods.push(Method::Head),
                "WEBSOCKET" => methods.push(Method::WebSocket),
                _ => {}
            }
        }
    }
    if methods.is_empty() {
        None
    } else {
        Some(methods)
    }
}

fn depends_arg_name<'a>(src: &'a [u8], node: Node<'_>) -> Option<String> {
    match node.kind() {
        "identifier" => Some(node_text(src, node).to_owned()),
        "attribute" => {
            let mut parts: Vec<&'a str> = Vec::new();
            collect_dotted_parts(src, node, &mut parts);
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("."))
            }
        }
        "call" => {
            let func = node.child_by_field_name("function")?;
            depends_arg_name(src, func)
        }
        _ => None,
    }
}

fn collect_dotted_parts<'a>(src: &'a [u8], node: Node<'_>, parts: &mut Vec<&'a str>) {
    match node.kind() {
        "identifier" => parts.push(node_text(src, node)),
        "attribute" => {
            if let Some(obj) = node.child_by_field_name("object") {
                collect_dotted_parts(src, obj, parts);
            }
            if let Some(attr) = node.child_by_field_name("attribute") {
                parts.push(node_text(src, attr));
            }
        }
        _ => {}
    }
}

fn extract_dependencies_kwarg(src: &[u8], args: Node<'_>) -> Vec<String> {
    let val = match find_kwarg_node(src, args, "dependencies") {
        Some(v) => v,
        None => return vec![],
    };
    if val.kind() != "list" {
        return vec![];
    }
    let mut deps = vec![];
    let mut cursor = val.walk();
    for child in val.children(&mut cursor) {
        if child.kind() == "call" {
            // Depends(name), Depends(auth.name), or Depends(factory())
            if let Some(arg_list) = child.child_by_field_name("arguments") {
                let mut cursor2 = arg_list.walk();
                for sub in arg_list.children(&mut cursor2) {
                    if matches!(sub.kind(), "identifier" | "attribute" | "call")
                        && let Some(name) = depends_arg_name(src, sub)
                    {
                        deps.push(name);
                        break;
                    }
                }
            }
        }
    }
    deps
}

fn extract_string_list_kwarg(src: &[u8], args: Node<'_>, kwarg: &str) -> Vec<String> {
    let val = match find_kwarg_node(src, args, kwarg) {
        Some(v) => v,
        None => return vec![],
    };
    let mut result = vec![];
    let mut cursor = val.walk();
    for child in val.children(&mut cursor) {
        if let Some(s) = string_value(src, child) {
            result.push(s);
        }
    }
    result
}

fn extract_positional_text(src: &[u8], args: Node<'_>, pos: usize) -> String {
    let mut positional = 0usize;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => {}
            "," | "(" | ")" => {}
            _ => {
                if positional == pos {
                    return node_text(src, child).to_owned();
                }
                positional += 1;
            }
        }
    }
    String::new()
}

// ── Low-level helpers ─────────────────────────────────────────────────────────

fn find_kwarg_node<'a>(src: &[u8], args: Node<'a>, name: &str) -> Option<Node<'a>> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "keyword_argument" {
            let key = child.child(0)?;
            if node_text(src, key) == name {
                return child.child(2); // key = value
            }
        }
    }
    None
}

fn string_value(src: &[u8], node: Node<'_>) -> Option<String> {
    if node.kind() == "string" {
        Some(unquote(node_text(src, node)))
    } else if node.kind() == "concatenated_string" {
        // "a" "b" → "ab"
        let mut s = String::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "string" {
                s.push_str(&unquote(node_text(src, child)));
            }
        }
        Some(s)
    } else {
        None
    }
}

fn is_fstring(s: &str) -> bool {
    let s = s.trim();
    s.starts_with('f')
        || s.starts_with('F')
        || s.starts_with("rf")
        || s.starts_with("fr")
        || s.starts_with("RF")
        || s.starts_with("FR")
}

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_file;

    fn test_uri() -> Uri {
        "file:///app/main.py".parse().unwrap()
    }

    fn run(src: &str) -> FileFacts {
        let bytes = src.as_bytes();
        let tree = parse_file(bytes);
        extract(bytes, &tree, &test_uri(), crate::offset::Encoding::Utf8)
    }

    #[test]
    fn extracts_simple_get_route() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/items")
def list_items(): ...
"#);
        assert_eq!(facts.routes.len(), 1);
        let route = &facts.routes[0];
        assert_eq!(route.handler_name, "list_items");
        assert_eq!(route.methods, vec![Method::Get]);
        assert!(matches!(&route.path, PrefixValue::Literal(p) if p == "/items"));
    }

    #[test]
    fn extracts_router_prefix() {
        let facts = run(r#"
from fastapi import APIRouter
router = APIRouter(prefix="/books", tags=["books"])

@router.post("/{book_id}", response_model="Book", status_code=201)
def create_book(book_id: int): ...
"#);
        assert_eq!(facts.routers.len(), 1);
        assert_eq!(facts.routers[0].name, "router");
        assert!(matches!(&facts.routers[0].prefix, PrefixValue::Literal(p) if p == "/books"));

        assert_eq!(facts.routes.len(), 1);
        let r = &facts.routes[0];
        assert_eq!(r.handler_name, "create_book");
        assert_eq!(r.methods, vec![Method::Post]);
        assert_eq!(r.response_model.as_deref(), Some("Book"));
    }

    #[test]
    fn extracts_include_router() {
        let facts = run(r#"
from fastapi import FastAPI
from app.routers import books
app = FastAPI()
app.include_router(books.router, prefix="/api")
"#);
        assert_eq!(facts.includes.len(), 1);
        let inc = &facts.includes[0];
        assert_eq!(inc.app_name, "app");
        assert!(matches!(&inc.prefix, PrefixValue::Literal(p) if p == "/api"));
    }

    #[test]
    fn include_router_captures_dependencies() {
        let facts = run(r#"
from fastapi import FastAPI, Depends
from app.routers import items
app = FastAPI()
app.include_router(items.router, dependencies=[Depends(get_token_header)])
"#);
        assert_eq!(facts.includes.len(), 1);
        assert_eq!(facts.includes[0].dependencies, vec!["get_token_header"]);
    }

    #[test]
    fn resolves_module_constant_prefix() {
        let facts = run(r#"
PREFIX = "/api"
from fastapi import APIRouter
router = APIRouter(prefix=PREFIX)
"#);
        assert_eq!(facts.routers.len(), 1);
        assert!(matches!(&facts.routers[0].prefix, PrefixValue::Literal(p) if p == "/api"));
    }

    #[test]
    fn websocket_route() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.websocket("/ws")
async def ws_endpoint(websocket): ...
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::WebSocket]);
    }

    #[test]
    fn broken_syntax_yields_partial_facts() {
        // Incomplete decorator should not panic (P3)
        let src = b"@app.get\ndef foo(): ...";
        let tree = parse_file(src);
        let _ = extract(src, &tree, &test_uri(), crate::offset::Encoding::Utf8); // must not panic
    }

    #[test]
    fn fstring_path_yields_unresolved() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()
name = "world"

@app.get(f"/hello/{name}")
def hello(): ...
"#);
        assert_eq!(facts.routes.len(), 1);
        assert!(matches!(facts.routes[0].path, PrefixValue::Unresolved));
    }

    #[test]
    fn annotated_constant_resolved() {
        // PREFIX: str = "/api" is an annotated assignment — must still be collected
        let facts = run(r#"
PREFIX: str = "/api"
from fastapi import APIRouter
router = APIRouter(prefix=PREFIX)
"#);
        assert_eq!(facts.routers.len(), 1);
        assert!(matches!(&facts.routers[0].prefix, PrefixValue::Literal(p) if p == "/api"));
    }

    #[test]
    fn trace_method_extracted() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.trace("/trace-me")
def trace_handler(): ...
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::Trace]);
    }

    #[test]
    fn factory_pattern_extracts_at_any_depth() {
        // REQ-ROUTE-12: facts extracted inside function bodies
        let facts = run(r#"
from fastapi import APIRouter, FastAPI

def create_app(debug: bool = False) -> FastAPI:
    app = FastAPI(debug=debug)
    router = APIRouter(prefix="/items")

    @router.get("/")
    def list_items():
        return []

    @router.get("/{item_id}")
    def get_item(item_id: int):
        return {"id": item_id}

    app.include_router(router)
    return app

app = create_app()
"#);
        // AppDecl inside factory body
        assert!(
            facts.apps.iter().any(|a| a.name == "app"),
            "AppDecl not found; apps: {:?}",
            facts.apps.iter().map(|a| &a.name).collect::<Vec<_>>()
        );
        // RouterDecl with prefix
        assert!(
            facts.routers.iter().any(|r| r.name == "router"),
            "RouterDecl 'router' not found"
        );
        assert!(matches!(
            &facts.routers.iter().find(|r| r.name == "router").unwrap().prefix,
            PrefixValue::Literal(p) if p == "/items"
        ));
        // Routes extracted from decorated defs inside factory
        assert!(
            facts.routes.iter().any(|r| r.handler_name == "list_items"),
            "list_items route not found"
        );
        assert!(
            facts.routes.iter().any(|r| r.handler_name == "get_item"),
            "get_item route not found"
        );
        // include_router call
        assert!(
            facts.includes.iter().any(|i| i.app_name == "app"),
            "include_router call not found"
        );
    }

    #[test]
    fn nested_factory_local_constant() {
        // Function-local string constants resolve for factory-local APIRouter (REQ-ROUTE-12)
        let facts = run(r#"
from fastapi import APIRouter, FastAPI

def create_app():
    PREFIX = "/v2"
    router = APIRouter(prefix=PREFIX)
    return router
"#);
        assert_eq!(facts.routers.len(), 1);
        assert_eq!(facts.routers[0].name, "router");
        assert!(
            matches!(
                &facts.routers[0].prefix,
                PrefixValue::Literal(p) if p == "/v2"
            ),
            "expected /v2, got {:?}",
            facts.routers[0].prefix
        );
    }

    // ── REQ-STAR-01: table-style Route / WebSocketRoute extraction ────────────

    #[test]
    fn route_table_simple_get() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Route
app = Starlette(routes=[Route("/items", list_items)])
"#);
        assert_eq!(facts.routes.len(), 1);
        let r = &facts.routes[0];
        assert_eq!(r.handler_name, "list_items");
        assert_eq!(r.methods, vec![Method::Get]);
        assert!(matches!(&r.path, PrefixValue::Literal(p) if p == "/items"));
        assert_eq!(r.object_name, "app");
    }

    #[test]
    fn route_table_explicit_post() {
        let facts = run(r#"
from starlette.routing import Route
app = Starlette(routes=[Route("/items", create_item, methods=["POST"])])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::Post]);
        assert_eq!(facts.routes[0].handler_name, "create_item");
    }

    #[test]
    fn route_table_multiple_methods_each_becomes_own_fact() {
        // One Route entry with methods=["GET","POST"] → two RouteFacts
        let facts = run(r#"
from starlette.routing import Route
app = Starlette(routes=[Route("/items", items_view, methods=["GET", "POST"])])
"#);
        assert_eq!(facts.routes.len(), 2);
        let methods: Vec<&Method> = facts.routes.iter().map(|r| &r.methods[0]).collect();
        assert!(methods.contains(&&Method::Get));
        assert!(methods.contains(&&Method::Post));
        assert!(facts.routes.iter().all(|r| r.handler_name == "items_view"));
    }

    #[test]
    fn route_table_websocket() {
        let facts = run(r#"
from starlette.routing import WebSocketRoute
app = Starlette(routes=[WebSocketRoute("/ws", ws_handler)])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::WebSocket]);
        assert_eq!(facts.routes[0].handler_name, "ws_handler");
        assert!(matches!(&facts.routes[0].path, PrefixValue::Literal(p) if p == "/ws"));
    }

    #[test]
    fn route_table_mixed_routes_and_websocket() {
        let facts = run(r#"
from starlette.routing import Route, WebSocketRoute
app = Starlette(routes=[
    Route("/items", list_items),
    WebSocketRoute("/ws", ws_handler),
])
"#);
        assert_eq!(facts.routes.len(), 2);
        assert!(facts.routes.iter().any(|r| r.methods == vec![Method::Get]));
        assert!(
            facts
                .routes
                .iter()
                .any(|r| r.methods == vec![Method::WebSocket])
        );
    }

    #[test]
    fn route_table_endpoint_class_captured() {
        // REQ-STAR-02: endpoint classes count as handlers
        let facts = run(r#"
from starlette.routing import Route
app = Starlette(routes=[Route("/items", ItemsView)])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].handler_name, "ItemsView");
    }

    #[test]
    fn route_table_route_name_kwarg() {
        let facts = run(r#"
from starlette.routing import Route
app = Starlette(routes=[Route("/items", list_items, name="items-list")])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].route_name.as_deref(), Some("items-list"));
    }

    #[test]
    fn route_table_fastapi_ctor() {
        // FastAPI also accepts routes= for Starlette-compatible table routing
        let facts = run(r#"
from fastapi import FastAPI
from starlette.routing import Route
app = FastAPI(routes=[Route("/ping", ping_handler)])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].handler_name, "ping_handler");
    }

    #[test]
    fn route_table_app_decl_still_recorded() {
        // AppDecl must still be emitted even when routes= is present
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Route
app = Starlette(routes=[Route("/items", list_items)])
"#);
        assert_eq!(facts.apps.len(), 1);
        assert_eq!(facts.apps[0].name, "app");
    }

    #[test]
    fn return_annotation_bare_identifier_extracted() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/books")
def list_books() -> Book:
    pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].return_annotation, Some("Book".to_owned()));
    }

    #[test]
    fn return_annotation_builtin_suppressed() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/none")
def nothing() -> None:
    pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(
            facts.routes[0].return_annotation, None,
            "None should be suppressed"
        );
    }

    #[test]
    fn return_annotation_response_type_suppressed() {
        let facts = run(r#"
from fastapi import FastAPI
from fastapi.responses import JSONResponse
app = FastAPI()

@app.get("/json")
def get_json() -> JSONResponse:
    pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(
            facts.routes[0].return_annotation, None,
            "JSONResponse should be suppressed"
        );
    }

    #[test]
    fn return_annotation_subscript_yields_none() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/books")
def list_books() -> list[Book]:
    pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(
            facts.routes[0].return_annotation, None,
            "subscript should be suppressed"
        );
    }

    #[test]
    fn return_annotation_attribute_yields_none() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/books")
def get_book() -> schemas.Book:
    pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(
            facts.routes[0].return_annotation, None,
            "attribute access should be suppressed"
        );
    }

    #[test]
    fn return_annotation_absent_when_no_annotation() {
        let facts = run(r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/books")
def list_books():
    pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(
            facts.routes[0].return_annotation, None,
            "no annotation should be None"
        );
    }

    // ── REQ-STAR-01 / REQ-STAR-04: Mount handling ─────────────────────────────

    #[test]
    fn mount_routes_kwarg_flattens_with_prefix() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Route, Mount

app = Starlette(routes=[
    Mount("/api", routes=[
        Route("/books", list_books),
        Route("/users", list_users),
    ]),
])
"#);
        assert_eq!(facts.routes.len(), 2);
        let paths: Vec<&str> = facts
            .routes
            .iter()
            .filter_map(|r| {
                if let PrefixValue::Literal(p) = &r.path {
                    Some(p.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            paths.contains(&"/api/books"),
            "nested route should be /api/books"
        );
        assert!(
            paths.contains(&"/api/users"),
            "nested route should be /api/users"
        );
    }

    #[test]
    fn mount_nested_websocket_route() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import WebSocketRoute, Mount

app = Starlette(routes=[
    Mount("/ws", routes=[
        WebSocketRoute("/chat", chat_handler),
    ]),
])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::WebSocket]);
        assert!(matches!(&facts.routes[0].path, PrefixValue::Literal(p) if p == "/ws/chat"));
    }

    #[test]
    fn mount_app_identifier_creates_include() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Mount

subapp = Starlette()
app = Starlette(routes=[
    Mount("/sub", app=subapp),
])
"#);
        assert_eq!(
            facts.routes.len(),
            0,
            "cross-app mount should produce include, not route"
        );
        assert_eq!(facts.includes.len(), 1);
        assert_eq!(facts.includes[0].target, "subapp");
        assert!(matches!(&facts.includes[0].prefix, PrefixValue::Literal(p) if p == "/sub"));
    }

    #[test]
    fn mount_staticfiles_creates_terminal_mount_route() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Mount
from starlette.staticfiles import StaticFiles

app = Starlette(routes=[
    Mount("/static", app=StaticFiles(directory="static"), name="static"),
])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::Mount]);
        assert!(
            matches!(&facts.routes[0].path, PrefixValue::Literal(p) if p == "/static"),
            "path should be /static, got {:?}",
            facts.routes[0].path
        );
        assert_eq!(facts.routes[0].route_name, Some("static".to_owned()));
    }

    #[test]
    fn unnamed_mount_has_no_route_name() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Mount
from starlette.staticfiles import StaticFiles

app = Starlette(routes=[
    Mount("/static", app=StaticFiles(directory="static")),
])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::Mount]);
        assert_eq!(
            facts.routes[0].route_name, None,
            "unnamed mount should have no route_name"
        );
    }

    #[test]
    fn imperative_app_mount_creates_include() {
        let facts = run(r#"
from starlette.applications import Starlette

subapp = Starlette()
app = Starlette()
app.mount("/sub", subapp)
"#);
        assert_eq!(facts.includes.len(), 1);
        assert_eq!(facts.includes[0].target, "subapp");
        assert!(matches!(&facts.includes[0].prefix, PrefixValue::Literal(p) if p == "/sub"));
    }

    #[test]
    fn imperative_app_mount_staticfiles_terminal() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.staticfiles import StaticFiles

app = Starlette()
app.mount("/static", StaticFiles(directory="static"), name="static")
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].methods, vec![Method::Mount]);
        assert!(
            matches!(&facts.routes[0].path, PrefixValue::Literal(p) if p == "/static"),
            "got {:?}",
            facts.routes[0].path
        );
        assert_eq!(facts.routes[0].route_name, Some("static".to_owned()));
    }

    #[test]
    fn mount_nested_mount_double_prefix() {
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Route, Mount

app = Starlette(routes=[
    Mount("/outer", routes=[
        Mount("/inner", routes=[
            Route("/item", get_item),
        ]),
    ]),
])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert!(
            matches!(&facts.routes[0].path, PrefixValue::Literal(p) if p == "/outer/inner/item"),
            "double-mount prefix should produce /outer/inner/item, got {:?}",
            facts.routes[0].path
        );
    }

    #[test]
    fn mount_table_style_positional_app() {
        // Mount("/sub", subapp) — positional, no app= keyword
        let facts = run(r#"
from starlette.applications import Starlette
from starlette.routing import Mount

subapp = Starlette()
app = Starlette(routes=[
    Mount("/sub", subapp),
])
"#);
        assert_eq!(facts.includes.len(), 1);
        assert_eq!(facts.includes[0].target, "subapp");
        assert!(matches!(&facts.includes[0].prefix, PrefixValue::Literal(p) if p == "/sub"));
    }

    #[test]
    fn imperative_add_route_creates_route_fact() {
        let facts = run(r#"
from starlette.applications import Starlette

app = Starlette()
app.add_route("/health", health_check, methods=["GET"])
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].handler_name, "health_check");
        assert!(matches!(&facts.routes[0].path, PrefixValue::Literal(p) if p == "/health"));
        assert_eq!(facts.routes[0].methods, vec![Method::Get]);
    }

    #[test]
    fn two_routes_same_path_on_router_both_extracted() {
        let facts = run(r#"
from fastapi import APIRouter
router = APIRouter()

@router.get("/import-from-pca")
async def view2(): ...

@router.get("/import-from-pca")
async def pca_import_view(
    pca_id: str,
):
    pass
"#);
        assert_eq!(
            facts.routes.len(),
            2,
            "both routes must be extracted; got {:?}",
            facts
                .routes
                .iter()
                .map(|r| &r.handler_name)
                .collect::<Vec<_>>()
        );
        let names: Vec<_> = facts
            .routes
            .iter()
            .map(|r| r.handler_name.as_str())
            .collect();
        assert!(names.contains(&"view2"), "view2 must be in routes");
        assert!(
            names.contains(&"pca_import_view"),
            "pca_import_view must be in routes"
        );
        for route in &facts.routes {
            assert!(
                matches!(&route.path, crate::state::PrefixValue::Literal(p) if p == "/import-from-pca"),
                "route {} path should be /import-from-pca",
                route.handler_name,
            );
        }
    }

    #[test]
    fn route_dependencies_plain_identifier() {
        let facts = run(r#"
from fastapi import FastAPI, Depends
app = FastAPI()
@app.get("/items", dependencies=[Depends(verify_token)])
def list_items(): pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].dependencies, vec!["verify_token"]);
    }

    #[test]
    fn route_dependencies_dotted_attribute() {
        let facts = run(r#"
from fastapi import FastAPI, Depends
import auth
app = FastAPI()
@app.get("/items", dependencies=[Depends(auth.get_user)])
def list_items(): pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].dependencies, vec!["auth.get_user"]);
    }

    #[test]
    fn route_dependencies_called_factory() {
        let facts = run(r#"
from fastapi import FastAPI, Depends
app = FastAPI()
@app.get("/items", dependencies=[Depends(get_db())])
def list_items(): pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(facts.routes[0].dependencies, vec!["get_db"]);
    }

    #[test]
    fn route_dependencies_multiple_mixed() {
        let facts = run(r#"
from fastapi import FastAPI, Depends
import auth
app = FastAPI()
@app.get("/items", dependencies=[Depends(verify_token), Depends(auth.rate_limit), Depends(get_db())])
def list_items(): pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(
            facts.routes[0].dependencies,
            vec!["verify_token", "auth.rate_limit", "get_db"]
        );
    }

    // ── class-attribute router (REQ-ROUTE-13) ─────────────────────────────────

    #[test]
    fn self_router_assignment_creates_router_decl() {
        let facts = run(r#"
from fastapi import APIRouter

class MyView:
    def __init__(self):
        self.router = APIRouter(prefix="/items")
"#);
        let router = facts
            .routers
            .iter()
            .find(|r| r.name == "self.router");
        assert!(
            router.is_some(),
            "self.router = APIRouter() must produce a RouterDecl with name 'self.router'"
        );
        let router = router.unwrap();
        assert!(
            matches!(&router.prefix, PrefixValue::Literal(p) if p == "/items"),
            "prefix must be extracted"
        );
    }

    #[test]
    fn self_router_decorator_sets_object_name() {
        let facts = run(r#"
from fastapi import APIRouter

class MyView:
    def __init__(self):
        self.router = APIRouter()

    @self.router.get("/list")
    def list_items(self): pass
"#);
        assert_eq!(facts.routes.len(), 1);
        assert_eq!(
            facts.routes[0].object_name, "self.router",
            "route object_name must be 'self.router'"
        );
    }
}
