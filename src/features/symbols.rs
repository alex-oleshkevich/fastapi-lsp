use tower_lsp_server::ls_types::{
    DocumentSymbol, Location as LspLocation, SymbolKind, Uri, WorkspaceSymbol,
    WorkspaceSymbolResponse,
};

use crate::state::{Method, ResolvedPath, RouteRecord, WorkspaceState};

pub fn document_symbols(state: &WorkspaceState, uri: &Uri) -> Vec<DocumentSymbol> {
    let linked = state.linked.load();
    linked
        .route_index
        .values()
        .flat_map(|records| records.iter())
        .filter(|r| &r.handler.uri == uri)
        .map(|r| {
            #[allow(deprecated)]
            DocumentSymbol {
                name: symbol_name(r),
                detail: None,
                kind: symbol_kind(r),
                tags: None,
                deprecated: None,
                range: r.handler.range,
                selection_range: r.handler.range,
                children: None,
            }
        })
        .collect()
}

pub fn workspace_symbols(
    state: &WorkspaceState,
    query: &str,
) -> WorkspaceSymbolResponse {
    let linked = state.linked.load();
    let symbols: Vec<WorkspaceSymbol> = linked
        .route_index
        .values()
        .flat_map(|records| records.iter())
        .filter(|r| matches_query(r, query))
        .map(|r| WorkspaceSymbol {
            name: symbol_name(r),
            kind: symbol_kind(r),
            tags: None,
            container_name: None,
            location: OneOf::Left(LspLocation {
                uri: r.handler.uri.clone(),
                range: r.handler.range,
            }),
            data: None,
        })
        .collect();
    WorkspaceSymbolResponse::Nested(symbols)
}

fn symbol_name(r: &RouteRecord) -> String {
    let path_str = match &r.resolved_path {
        ResolvedPath::Resolved(p) => p.clone(),
        ResolvedPath::Unresolved => format!("⟨unresolved⟩{}", r.decorator_path),
    };
    if r.method == Method::Mount {
        format!("MOUNT {path_str}")
    } else {
        format!("{} {path_str} · {}", r.method, r.name)
    }
}

fn symbol_kind(r: &RouteRecord) -> SymbolKind {
    if r.method == Method::Mount {
        SymbolKind::NAMESPACE
    } else {
        SymbolKind::FUNCTION
    }
}

fn matches_query(r: &RouteRecord, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let path_str = match &r.resolved_path {
        ResolvedPath::Resolved(p) => p.as_str(),
        ResolvedPath::Unresolved => r.decorator_path.as_str(),
    };
    let pl = path_str.to_lowercase();
    let ml = r.method.to_string().to_lowercase();
    let nl = r.name.to_lowercase();
    // Split on whitespace so "GET /items" matches as two tokens, and "GET " (trailing space)
    // doesn't fail because "get".contains("get ") == false.
    query.split_whitespace().all(|tok| {
        let t = tok.to_lowercase();
        pl.contains(&*t) || ml.contains(&*t) || nl.contains(&*t)
    })
}

use tower_lsp_server::ls_types::OneOf;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Method, ResolvedPath, RouteId, RouteRecord};
    use tower_lsp_server::ls_types::{Range, Uri};

    fn make_record(method: Method, path: &str, name: &str) -> RouteRecord {
        use crate::state::Location as StateLocation;
        let uri: Uri = "file:///a.py".parse().unwrap();
        RouteRecord {
            id: RouteId(format!("{method}:{path}")),
            ordinal: 0,
            name: name.to_owned(),
            method,
            resolved_path: ResolvedPath::Resolved(path.to_owned()),
            decorator_path: path.to_owned(),
            chain: vec![],
            handler: StateLocation { uri, range: Range::default() },
            path_params: vec![],
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: None,
            path_quote_width: None,
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        }
    }

    #[test]
    fn symbol_search_method_trailing_space() {
        let r = make_record(Method::Get, "/items", "list_items");
        assert!(matches_query(&r, "GET "), "trailing space after method must still match");
        assert!(matches_query(&r, "get "), "lowercase with trailing space must match");
    }

    #[test]
    fn symbol_search_method_and_path_tokens() {
        let r = make_record(Method::Get, "/v1/items", "list_items");
        assert!(matches_query(&r, "GET items"), "method + path fragment must both match");
        assert!(matches_query(&r, "get /v1"), "lowercase method + prefix must match");
        assert!(!matches_query(&r, "POST items"), "wrong method must not match");
    }

    #[test]
    fn symbol_search_empty_query_matches_all() {
        let r = make_record(Method::Post, "/anything", "handler");
        assert!(matches_query(&r, ""));
        assert!(matches_query(&r, "   ")); // whitespace only → no tokens → all match
    }
}
