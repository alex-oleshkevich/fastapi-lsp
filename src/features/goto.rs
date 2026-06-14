use tower_lsp_server::ls_types::{
    GotoDefinitionResponse, Location as LspLocation, Position, Range, Uri,
};

use crate::state::WorkspaceState;
use crate::util::position_in_range;

pub fn goto(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<GotoDefinitionResponse> {
    let edge = edge_at(state, uri, pos)?;
    let locations = resolve_edge(state, edge);
    if locations.is_empty() {
        None
    } else if locations.len() == 1 {
        Some(GotoDefinitionResponse::Scalar(locations.into_iter().next().unwrap()))
    } else {
        Some(GotoDefinitionResponse::Array(locations))
    }
}

pub fn references(
    state: &WorkspaceState,
    uri: &Uri,
    pos: Position,
    include_declaration: bool,
) -> Vec<LspLocation> {
    let facts = match state.file_facts.get(uri) {
        Some(f) => f,
        None => return vec![],
    };
    let linked = state.linked.load();
    let mut locs: Vec<LspLocation> = vec![];

    // References on a handler: url_for call sites + client call sites (REQ-NAV-02).
    // Not early-returning so a function that is both a handler and a dep target reports both.
    if let Some(route) = linked
        .route_index
        .values()
        .flat_map(|r| r.iter())
        .find(|r| {
            &r.handler.uri == uri
                && position_in_range(pos, r.handler.range.start, r.handler.range.end)
        })
    {
        if include_declaration {
            locs.push(LspLocation { uri: uri.clone(), range: route.handler.range });
        }
        let name = route.name.clone();
        let route_id = route.id.clone();

        // url_for sites that reference this name
        for fe in state.file_facts.iter() {
            let fv = fe.value();
            for site in &fv.url_for_sites {
                if site.name == name {
                    locs.push(LspLocation { uri: fv.uri.clone(), range: site.range });
                }
            }
        }

        // Client-call sites from the test_refs index
        if let Some(sites) = linked.test_refs.get(&route_id) {
            for site in sites {
                locs.push(LspLocation { uri: site.location.uri.clone(), range: site.location.range });
            }
        }
    }

    // References on a dependency def: Depends call sites + override sites (REQ-DI-05).
    // Checked independently — not else-if — in case the cursor is on a function that is both
    // a route handler and used as a Depends target.
    if let Some(dep) = facts.dep_defs.iter().find(|d| {
        position_in_range(pos, d.node_id.range.start, d.node_id.range.end)
    }) {
        if include_declaration {
            locs.push(LspLocation { uri: uri.clone(), range: dep.node_id.range });
        }
        let dep_name = dep.name.clone();
        let dep_node_id = dep.node_id.clone();
        for fe in state.file_facts.iter() {
            let fv = fe.value();
            for dep_ref in &fv.dep_refs {
                if dep_ref.name == dep_name {
                    locs.push(LspLocation { uri: fv.uri.clone(), range: dep_ref.range });
                }
            }
        }
        // Include override sites from the dep graph (REQ-DI-05)
        if let Some(sites) = linked.dep_graph.override_sites.get(&dep_node_id) {
            for site in sites {
                locs.push(LspLocation { uri: site.uri.clone(), range: site.range });
            }
        }
    }

    locs
}

// ── Edge dispatch ─────────────────────────────────────────────────────────────

enum Edge {
    UrlForName(String),
    IncludeTarget(String),
    DependsName(String),
    EnvKey(String),
    /// Cursor is on a client-call path string; navigate to the matched handler(s).
    ClientPath(Uri, Range),
    /// Cursor is on a template name string; navigate to the template file (REQ-NAV-01).
    TemplateName(String),
}

fn edge_at(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Edge> {
    let facts = state.file_facts.get(uri)?;

    // url_for call sites
    for site in &facts.url_for_sites {
        if position_in_range(pos, site.range.start, site.range.end) {
            return Some(Edge::UrlForName(site.name.clone()));
        }
    }

    // include_router targets
    for inc in &facts.includes {
        if position_in_range(pos, inc.range.start, inc.range.end) {
            return Some(Edge::IncludeTarget(inc.target.clone()));
        }
    }

    // Depends references
    for dep_ref in &facts.dep_refs {
        if position_in_range(pos, dep_ref.range.start, dep_ref.range.end) {
            return Some(Edge::DependsName(dep_ref.name.clone()));
        }
    }

    // Env lookup sites
    for site in &facts.env_lookups {
        if position_in_range(pos, site.range.start, site.range.end) {
            return Some(Edge::EnvKey(site.key.clone()));
        }
    }

    // Client call path strings (in test files).
    // Hit-test against path_range (the static path portion) — not the whole call expression —
    // so clicking inside f-string interpolations doesn't hijack normal goto-definition.
    for call in &facts.client_calls {
        if position_in_range(pos, call.path_range.start, call.path_range.end) {
            return Some(Edge::ClientPath(uri.clone(), call.range));
        }
    }

    // Template name strings — TemplateResponse / get_template calls (REQ-NAV-01)
    for tpl in &facts.templates {
        if position_in_range(pos, tpl.range.start, tpl.range.end) {
            return Some(Edge::TemplateName(tpl.path.clone()));
        }
    }

    None
}

fn resolve_edge(state: &WorkspaceState, edge: Edge) -> Vec<LspLocation> {
    match edge {
        Edge::UrlForName(name) => {
            let linked = state.linked.load();
            linked
                .route_names
                .get(&name)
                .into_iter()
                .flat_map(|ids| ids.iter())
                .filter_map(|id| {
                    linked.route_index.get(id).and_then(|records| records.first()).map(|r| {
                        LspLocation { uri: r.handler.uri.clone(), range: r.handler.range }
                    })
                })
                .collect()
        }

        Edge::IncludeTarget(target) => {
            // Navigate to the router/app declaration
            for fe in state.file_facts.iter() {
                let fv = fe.value();
                for router in &fv.routers {
                    if router.name == target || target.ends_with(&format!(".{}", router.name)) {
                        return vec![LspLocation {
                            uri: fv.uri.clone(),
                            range: router.range,
                        }];
                    }
                }
                for app in &fv.apps {
                    if app.name == target {
                        return vec![LspLocation { uri: fv.uri.clone(), range: app.range }];
                    }
                }
            }
            vec![]
        }

        Edge::DependsName(name) => {
            for fe in state.file_facts.iter() {
                let fv = fe.value();
                for dep in &fv.dep_defs {
                    if dep.name == name {
                        return vec![LspLocation {
                            uri: fv.uri.clone(),
                            range: dep.node_id.range,
                        }];
                    }
                }
            }
            vec![]
        }

        Edge::EnvKey(key) => {
            let linked = state.linked.load();
            linked
                .env_index
                .get(&key)
                .map(|entry| {
                    entry
                        .locations
                        .iter()
                        .map(|loc| LspLocation { uri: loc.uri.clone(), range: loc.range })
                        .collect()
                })
                .unwrap_or_default()
        }

        // Navigate from client-call path string to the matched handler(s) (REQ-NAV-01).
        // Uses call_site_index for O(1) lookup; multiple matches return all for picker.
        Edge::ClientPath(call_uri, call_range) => {
            let linked = state.linked.load();
            let mut locs: Vec<LspLocation> = vec![];
            let mut seen: std::collections::HashSet<(Uri, Range)> = std::collections::HashSet::new();
            if let Some(route_ids) = linked.call_site_index.get(&(call_uri, call_range)) {
                for route_id in route_ids {
                    if let Some(records) = linked.route_index.get(route_id) {
                        for record in records {
                            let key = (record.handler.uri.clone(), record.handler.range);
                            if seen.insert(key) {
                                locs.push(LspLocation {
                                    uri: record.handler.uri.clone(),
                                    range: record.handler.range,
                                });
                            }
                        }
                    }
                }
            }
            locs
        }

        Edge::TemplateName(path) => {
            let linked = state.linked.load();
            if let Some(tpl_uri) = linked.template_index.get(&path) {
                // Navigate to the top of the template file (line 0, col 0).
                let loc = LspLocation {
                    uri: tpl_uri.clone(),
                    range: Range::default(),
                };
                vec![loc]
            } else {
                vec![]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tower_lsp_server::ls_types::{Position, Range};

    use crate::config::ResolvedConfig;
    use crate::state::{DepDef, DepRef, FileFacts, Linked, Location as StateLocation, NodeId};

    #[test]
    fn goto_client_call_navigates_to_handler() {
        use crate::state::{ClientCall, ClientCallSite, FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId, RouteRecord};

        let uri_test: Uri = "file:///tests/test_routes.py".parse().unwrap();
        let uri_app: Uri = "file:///app.py".parse().unwrap();

        let call_range = Range { start: Position::new(5, 18), end: Position::new(5, 30) };
        let handler_range = Range { start: Position::new(10, 4), end: Position::new(10, 14) };

        let mut facts = FileFacts::new(uri_test.clone());
        let path_range = Range { start: Position::new(5, 19), end: Position::new(5, 29) };
        facts.client_calls.push(ClientCall {
            fixture_name: "client".to_owned(),
            method: Method::Get,
            path: "/items".to_owned(),
            is_prefix: false,
            path_depth: None,
            range: call_range,
            path_range,
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_test.clone(), facts);

        let route_id = RouteId("app.list_items".to_owned());
        let site = ClientCallSite {
            method: Method::Get,
            path: "/items".to_owned(),
            location: StateLocation { uri: uri_test.clone(), range: call_range },
        };

        let mut linked = Linked::default();
        linked.test_refs.insert(route_id.clone(), vec![site]);
        linked.call_site_index
            .entry((uri_test.clone(), call_range))
            .or_default()
            .push(route_id.clone());
        linked.route_index.insert(route_id, vec![RouteRecord {
            id: RouteId("app.list_items".to_owned()),
            ordinal: 0,
            name: "list_items".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/items".to_owned()),
            decorator_path: "/items".to_owned(),
            chain: vec![],
            handler: StateLocation { uri: uri_app.clone(), range: handler_range },
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
            handler_params_known: false,
        }]);
        state.linked.store(Arc::new(linked));

        let result = goto(&state, &uri_test, Position::new(5, 22)).unwrap();
        let locs = match result {
            tower_lsp_server::ls_types::GotoDefinitionResponse::Scalar(l) => vec![l],
            tower_lsp_server::ls_types::GotoDefinitionResponse::Array(ls) => ls,
            _ => vec![],
        };
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, uri_app);
        assert_eq!(locs[0].range, handler_range);
    }

    #[test]
    fn clicking_inside_fstring_interpolation_does_not_jump_to_route() {
        // Clicking on `uuid.uuid4` inside `{uuid.uuid4()}` must NOT jump to the router —
        // that position is inside the interpolation, not the static path prefix.
        use crate::state::{ClientCall, FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId, RouteRecord};

        let uri_test: Uri = "file:///tests/test_routes.py".parse().unwrap();
        let uri_app: Uri = "file:///app.py".parse().unwrap();

        // Simulate: client.delete(f"/v1/{item_id}")
        // call_range covers the whole expression; path_range covers only the prefix `/v1/`
        let call_range = Range { start: Position::new(0, 0), end: Position::new(0, 40) };
        let handler_range = Range { start: Position::new(10, 4), end: Position::new(10, 14) };
        // path_range covers only the f-string prefix before first interpolation: f"/v1/"
        let path_range = Range { start: Position::new(0, 15), end: Position::new(0, 20) };

        let mut facts = FileFacts::new(uri_test.clone());
        facts.client_calls.push(ClientCall {
            fixture_name: "client".to_owned(),
            method: Method::Delete,
            path: "/v1/".to_owned(),
            is_prefix: true,
            path_depth: Some(3),
            range: call_range,
            path_range,
        });

        let state = crate::state::WorkspaceState::new(
            crate::config::ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_test.clone(), facts);

        let route_id = RouteId("app.delete_item".to_owned());
        let mut linked = Linked::default();
        linked.call_site_index
            .entry((uri_test.clone(), call_range))
            .or_default()
            .push(route_id.clone());
        linked.route_index.insert(route_id, vec![RouteRecord {
            id: RouteId("app.delete_item".to_owned()),
            ordinal: 0,
            name: "delete_item".to_owned(),
            method: Method::Delete,
            resolved_path: ResolvedPath::Resolved("/v1/{item_id}".to_owned()),
            decorator_path: "/v1/{item_id}".to_owned(),
            chain: vec![],
            handler: StateLocation { uri: uri_app.clone(), range: handler_range },
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
            handler_params_known: false,
        }]);
        state.linked.store(Arc::new(linked));

        // Click at col 30 — inside `{item_id}` interpolation, OUTSIDE the path_range (cols 15-20)
        let result = goto(&state, &uri_test, Position::new(0, 30));
        // Must return None (no goto target) — interpolation clicks fall through to normal goto
        assert!(result.is_none(), "clicking inside f-string interpolation must not jump to route handler");
    }

    #[test]
    fn references_on_route_handler_includes_client_call_sites() {
        use crate::state::{ClientCallSite, FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId, RouteRecord};

        let uri_app: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/test_routes.py".parse().unwrap();

        let handler_range = Range { start: Position::new(3, 4), end: Position::new(3, 14) };
        let call_range = Range { start: Position::new(8, 18), end: Position::new(8, 26) };

        let facts_app = FileFacts::new(uri_app.clone());
        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_app.clone(), facts_app);

        let route_id = RouteId("app.get_user".to_owned());
        let site = ClientCallSite {
            method: Method::Get,
            path: "/users/1".to_owned(),
            location: StateLocation { uri: uri_test.clone(), range: call_range },
        };

        let mut linked = Linked::default();
        linked.test_refs.insert(route_id.clone(), vec![site]);
        linked.route_index.insert(route_id, vec![RouteRecord {
            id: RouteId("app.get_user".to_owned()),
            ordinal: 0,
            name: "get_user".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/users/{id}".to_owned()),
            decorator_path: "/users/{id}".to_owned(),
            chain: vec![],
            handler: StateLocation { uri: uri_app.clone(), range: handler_range },
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
            handler_params_known: false,
        }]);
        state.linked.store(Arc::new(linked));

        let locs = references(&state, &uri_app, Position::new(3, 8), false);
        assert_eq!(locs.len(), 1, "expected the client call site");
        assert_eq!(locs[0].uri, uri_test);
        assert_eq!(locs[0].range, call_range);
    }

    #[test]
    fn references_include_declaration_adds_handler_location() {
        use crate::state::{FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId, RouteRecord};

        let uri_app: Uri = "file:///app.py".parse().unwrap();
        let handler_range = Range { start: Position::new(2, 4), end: Position::new(2, 12) };

        let facts_app = FileFacts::new(uri_app.clone());
        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_app.clone(), facts_app);

        let route_id = RouteId("app.ping:GET".to_owned());
        let mut linked = Linked::default();
        linked.route_index.insert(route_id, vec![RouteRecord {
            id: RouteId("app.ping:GET".to_owned()),
            ordinal: 0,
            name: "ping".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/ping".to_owned()),
            decorator_path: "/ping".to_owned(),
            chain: vec![],
            handler: StateLocation { uri: uri_app.clone(), range: handler_range },
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
            handler_params_known: false,
        }]);
        state.linked.store(Arc::new(linked));

        // include_declaration=false → no handler range
        let locs = references(&state, &uri_app, Position::new(2, 6), false);
        assert!(!locs.iter().any(|l| l.range == handler_range), "declaration should not appear when include_declaration=false");

        // include_declaration=true → handler range appears
        let locs_with = references(&state, &uri_app, Position::new(2, 6), true);
        assert!(locs_with.iter().any(|l| l.range == handler_range && l.uri == uri_app), "declaration should appear when include_declaration=true");
    }

    #[test]
    fn references_on_dep_def_returns_dep_refs_and_override_sites() {
        let uri_app: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/conftest.py".parse().unwrap();

        let def_range = Range { start: Position::new(1, 4), end: Position::new(1, 10) };
        let dep_ref_range = Range { start: Position::new(5, 20), end: Position::new(5, 26) };
        let override_range = Range { start: Position::new(10, 32), end: Position::new(10, 38) };

        let mut facts_app = FileFacts::new(uri_app.clone());
        facts_app.dep_defs.push(DepDef {
            name: "get_db".to_owned(),
            node_id: NodeId { uri: uri_app.clone(), range: def_range },
            has_yield: true,
            param_names: vec![],
        });
        facts_app.dep_refs.push(DepRef {
            name: "get_db".to_owned(),
            range: dep_ref_range,
            is_called: false,
            callee_range: None,
            containing_func: Some("my_route".to_owned()),
            caller_node_id: None,
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_app.clone(), facts_app);

        let def_node = NodeId { uri: uri_app.clone(), range: def_range };
        let mut linked = Linked::default();
        linked.dep_graph.override_sites.insert(
            def_node,
            vec![StateLocation { uri: uri_test.clone(), range: override_range }],
        );
        state.linked.store(Arc::new(linked));

        let locs = references(&state, &uri_app, Position::new(1, 6), false);

        assert_eq!(locs.len(), 2, "expected dep_ref + override site");
        let ranges: Vec<Range> = locs.iter().map(|l| l.range).collect();
        assert!(ranges.contains(&dep_ref_range), "missing dep_ref location");
        assert!(ranges.contains(&override_range), "missing override site location");
        let override_loc = locs.iter().find(|l| l.range == override_range).unwrap();
        assert_eq!(override_loc.uri, uri_test);
    }
}
