use tower_lsp_server::ls_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Uri};

use crate::state::{EnvEntry, FileFacts, ResolvedPath, RouteRecord, WorkspaceState};
use crate::util::{is_secret_key, position_in_range};

pub fn hover(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Hover> {
    let linked = state.linked.load();
    let facts = state.file_facts.get(uri)?;

    // Check if cursor is on a route handler or its decorator
    let record = route_record_at(state, uri, pos, &facts)?;

    let md = route_card(&linked, record);
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: md,
        }),
        range: None,
    })
}

fn route_record_at(
    state: &WorkspaceState,
    uri: &Uri,
    pos: Position,
    _facts: &FileFacts,
) -> Option<RouteRecord> {
    let linked = state.linked.load();
    // Find a RouteRecord whose handler range contains the position
    linked
        .route_index
        .values()
        .flat_map(|records| records.iter())
        .find(|r| {
            &r.handler.uri == uri
                && position_in_range(pos, r.handler.range.start, r.handler.range.end)
        })
        .cloned()
}

fn route_card(
    _linked: &crate::state::Linked,
    record: RouteRecord,
) -> String {
    let path_str = match &record.resolved_path {
        ResolvedPath::Resolved(p) => p.clone(),
        ResolvedPath::Unresolved => format!("⟨unresolved⟩{}", record.decorator_path),
    };

    let mut lines = vec![format!("**{}** `{}`", record.method, path_str)];
    lines.push(String::new()); // blank line after heading

    let chain_parts: Vec<String> = record
        .chain
        .iter()
        .map(|link| {
            if link.prefix.is_empty() {
                link.object_name.clone()
            } else {
                format!("{} `{}`", link.object_name, link.prefix)
            }
        })
        .collect();
    if !chain_parts.is_empty() {
        lines.push(format!("- chain: {}", chain_parts.join(" → ")));
    }

    if let Some(model) = &record.response_model {
        lines.push(format!("- response model: `{model}`"));
    }

    if !record.dependencies.is_empty() {
        lines.push(format!("- dependencies: {}", record.dependencies.join(", ")));
    }

    if !record.path_params.is_empty() {
        let params: Vec<_> = record.path_params.iter().map(|p| p.name.as_str()).collect();
        lines.push(format!("- path params: {}", params.join(", ")));
    }

    if !record.middleware.is_empty() {
        lines.push("- middleware:".to_owned());
        for mw in &record.middleware {
            lines.push(format!("  - `{mw}`"));
        }
    }

    lines.join("\n")
}

/// Hover on a dependency function definition — shows graph summary + override sites (REQ-HOV-04).
pub fn dep_hover(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Hover> {
    let facts = state.file_facts.get(uri)?;
    let linked = state.linked.load();

    let dep = facts.dep_defs.iter().find(|d| {
        crate::util::position_in_range(pos, d.node_id.range.start, d.node_id.range.end)
    })?;

    // Routes that use this dep — either via decorator `dependencies=[Depends(dep)]`
    // or via a parameter annotation `def handler(x = Depends(dep))`.
    //
    // Keyed by handler *function* name (extracted from RouteId) so the lookup works even when
    // the route was given an explicit `name=` kwarg that differs from the function name.
    let handler_func_to_display: std::collections::HashMap<String, String> = linked
        .route_index
        .values()
        .flat_map(|records| records.iter())
        .map(|r| {
            let id_str = r.id.0.as_str();
            let uri_str = r.handler.uri.as_str();
            let func_name = id_str
                .strip_prefix(uri_str)
                .and_then(|rest| {
                    let rest = rest.trim_start_matches(':');
                    let pos = rest.rfind(':')?;
                    Some(rest[..pos].to_owned())
                })
                .unwrap_or_else(|| r.name.clone());
            (func_name, r.name.clone())
        })
        .collect();

    let mut using_route_set: std::collections::HashSet<String> = linked
        .route_index
        .values()
        .flat_map(|records| records.iter())
        .filter(|r| r.dependencies.iter().any(|d| d == &dep.name))
        .map(|r| r.name.clone())
        .collect();

    for fe in state.file_facts.iter() {
        for dep_ref in &fe.dep_refs {
            if dep_ref.name == dep.name {
                if let Some(func) = &dep_ref.containing_func {
                    if let Some(display) = handler_func_to_display.get(func.as_str()) {
                        using_route_set.insert(display.clone());
                    }
                }
            }
        }
    }

    // Deps used via a type alias (e.g. `CurrentPrivateContract = Annotated[T, Depends(fn)]`)
    // have no dep_ref at all — track them through dep_type_aliases → plain_typed_params.
    let alias_names: std::collections::HashSet<String> = state
        .file_facts
        .iter()
        .flat_map(|e| {
            e.value()
                .dep_type_aliases
                .iter()
                .filter(|(_, fn_name)| fn_name.as_str() == dep.name)
                .map(|(alias, _)| alias.clone())
                .collect::<Vec<_>>()
        })
        .collect();
    if !alias_names.is_empty() {
        for fe in state.file_facts.iter() {
            for param in &fe.plain_typed_params {
                if alias_names.contains(&param.type_name) {
                    if let Some(display) = handler_func_to_display.get(param.containing_func.as_str()) {
                        using_route_set.insert(display.clone());
                    }
                }
            }
        }
    }

    let mut using_routes: Vec<String> = using_route_set.into_iter().collect();
    using_routes.sort();

    // Dep functions that have a Depends(this) inside them (dep → dep edges)
    let using_deps: Vec<String> = linked
        .dep_graph
        .used_by
        .get(&dep.node_id)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| {
                    state
                        .file_facts
                        .get(&id.uri)
                        .and_then(|f| f.dep_defs.iter().find(|d| d.node_id == *id).map(|d| d.name.clone()))
                })
                .collect()
        })
        .unwrap_or_default();

    // Deps that this function itself depends on
    let uses_deps: Vec<String> = linked
        .dep_graph
        .uses
        .get(&dep.node_id)
        .map(|ids| {
            ids.iter()
                .filter_map(|id| {
                    state
                        .file_facts
                        .get(&id.uri)
                        .and_then(|f| f.dep_defs.iter().find(|d| d.node_id == *id).map(|d| d.name.clone()))
                })
                .collect()
        })
        .unwrap_or_default();

    let route_count = using_routes.len();
    let dep_count = using_deps.len();

    let summary = match (route_count, dep_count) {
        (0, 0) => "unused".to_owned(),
        (r, 0) => format!("used by {} route{}", r, if r == 1 { "" } else { "s" }),
        (0, d) => format!("used by {} dependenc{}", d, if d == 1 { "y" } else { "ies" }),
        (r, d) => format!(
            "used by {} route{}, {} dependenc{}",
            r, if r == 1 { "" } else { "s" },
            d, if d == 1 { "y" } else { "ies" }
        ),
    };

    let mut lines = vec![format!("**dependency** `{}` — {}", dep.name, summary)];
    lines.push(String::new());

    // "used by" detail line
    let mut used_by_parts: Vec<String> = using_routes
        .iter()
        .map(|n| format!("`{n}` (route)"))
        .collect();
    used_by_parts.extend(using_deps.iter().map(|n| format!("`{n}` (dependency)")));
    if used_by_parts.is_empty() {
        lines.push("- used by: —".to_owned());
    } else {
        lines.push(format!("- used by: {}", used_by_parts.join(" · ")));
    }

    // "uses" line
    if uses_deps.is_empty() {
        lines.push("- uses: —".to_owned());
    } else {
        let uses_str: Vec<String> = uses_deps.iter().map(|n| format!("`{n}`")).collect();
        lines.push(format!("- uses: {}", uses_str.join(" · ")));
    }

    if dep.has_yield {
        lines.push("- generator (yields value, returns on cleanup)".to_owned());
    }

    // Override sites (REQ-DI-05)
    if let Some(sites) = linked.dep_graph.override_sites.get(&dep.node_id)
        && !sites.is_empty() {
            let site_strs: Vec<String> = sites
                .iter()
                .map(|loc| {
                    let parts: Vec<&str> = loc.uri.as_str().split('/').collect();
                    let n = parts.len();
                    let display = if n >= 2 {
                        format!("{}/{}", parts[n - 2], parts[n - 1])
                    } else {
                        parts.last().copied().unwrap_or("?").to_owned()
                    };
                    format!("`{}:{}`", display, loc.range.start.line + 1)
                })
                .collect();
            lines.push(format!("- overridden in: {}", site_strs.join(", ")));
        }

    Some(make_hover(lines.join("\n")))
}

pub fn include_hover(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Hover> {
    let facts = state.file_facts.get(uri)?;
    let linked = state.linked.load();

    let inc = facts
        .includes
        .iter()
        .find(|i| position_in_range(pos, i.range.start, i.range.end))?;

    let target = &inc.target;
    // For a dotted import like "books.router" the Python object name is the last segment
    let target_obj = target.rsplit('.').next().unwrap_or(target.as_str());

    // Routes in the declaring file use the original name (`router`), not the import alias
    // (`projects_router`). Resolve through import_alias_originals so counts aren't zero.
    let resolved_obj = facts
        .import_alias_originals
        .get(target_obj)
        .map(|s| s.as_str())
        .unwrap_or(target_obj);

    // When an alias is resolved, use the prefix to avoid matching unrelated routers that
    // share the same local variable name (many modules name their router `router`).
    let prefix_filter: Option<&str> = if resolved_obj != target_obj {
        match &inc.prefix {
            crate::state::PrefixValue::Literal(p) if !p.is_empty() => Some(p.as_str()),
            _ => None,
        }
    } else {
        None
    };

    // Build the set of (handler_uri, handler_name) pairs that belong to this router.
    // This avoids the substring-on-RouteId false-positive bug.
    let matching_handlers: std::collections::HashSet<(Uri, String)> = state
        .file_facts
        .iter()
        .flat_map(|entry| {
            let h_uri = entry.key().clone();
            entry
                .value()
                .routes
                .iter()
                .filter(|rf| rf.object_name == target_obj || rf.object_name == resolved_obj)
                .map(move |rf| (h_uri.clone(), rf.handler_name.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    let is_matching = |r: &&crate::state::RouteRecord| {
        // RouteId format: "{uri}:{handler_name}:{method}" — split from right
        let handler_name = r.id.0.rsplit(':').nth(1).unwrap_or("").to_owned();
        if !matching_handlers.contains(&(r.handler.uri.clone(), handler_name)) {
            return false;
        }
        if let Some(prefix) = prefix_filter {
            match &r.resolved_path {
                ResolvedPath::Resolved(p) => p.starts_with(prefix),
                ResolvedPath::Unresolved => true,
            }
        } else {
            true
        }
    };

    // Count routes under this router
    let route_count = linked
        .route_index
        .values()
        .flat_map(|records| records.iter())
        .filter(is_matching)
        .count();

    let prefix_str = match &inc.prefix {
        crate::state::PrefixValue::Literal(p) if !p.is_empty() => format!(" under `{p}`"),
        _ => String::new(),
    };

    let md = format!("**router** `{target}` — {route_count} routes{prefix_str}");

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: md,
        }),
        range: None,
    })
}

// ── Env hover (REQ-HOV-05) ────────────────────────────────────────────────────

pub fn env_hover(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Hover> {
    let facts = state.file_facts.get(uri)?;
    let linked = state.linked.load();

    // Find an env lookup site under the cursor
    let site = facts.env_lookups.iter().find(|s| {
        position_in_range(pos, s.range.start, s.range.end)
    })?;

    let key = &site.key;
    let md = env_entry_card(key, linked.env_index.get(key));
    Some(make_hover(md))
}

pub fn settings_field_hover(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<Hover> {
    let facts = state.file_facts.get(uri)?;
    let linked = state.linked.load();

    // Find a settings field whose range contains the position
    for cls in &facts.settings_classes {
        for field in &cls.fields {
            if position_in_range(pos, field.range.start, field.range.end) {
                let key = field.env_key.as_deref().unwrap_or(&field.field_name);
                let md = env_entry_card(key, linked.env_index.get(key));
                return Some(make_hover(md));
            }
        }
    }
    None
}

fn env_entry_card(key: &str, entry: Option<&EnvEntry>) -> String {
    let value_str = match entry {
        None => "[not in workspace env files]".to_owned(),
        Some(e) => {
            if is_secret_key(key) {
                "••••••".to_owned()
            } else {
                format!("`{}`", e.value)
            }
        }
    };

    let mut lines = vec![format!("`{key}` = {value_str}")];

    if let Some(e) = entry {
        let locations: Vec<String> = e
            .locations
            .iter()
            .map(|loc| {
                let path = loc.uri.as_str().split('/').next_back().unwrap_or("?");
                format!("`.{}:{}`", path, loc.range.start.line + 1)
            })
            .collect();
        if !locations.is_empty() {
            lines.push(String::new());
            lines.push(format!("defined in: {}", locations.join(" · ")));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tower_lsp_server::ls_types::{Position, Range};

    use crate::config::ResolvedConfig;
    use crate::state::{
        DepDef, FileFacts, Linked, Location as StateLocation, Method, NodeId, ResolvedPath,
        RouteId, RouteRecord,
    };

    use super::dep_hover;

    fn make_route(name: &str, uri_str: &str, deps: Vec<String>) -> (RouteId, RouteRecord) {
        let uri: tower_lsp_server::ls_types::Uri = uri_str.parse().unwrap();
        let id = RouteId(format!("app.{name}"));
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: name.to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved(format!("/{name}")),
            decorator_path: format!("/{name}"),
            chain: vec![],
            handler: StateLocation { uri, range: Range::default() },
            path_params: vec![],
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: deps,
            middleware: vec![],
            path_range: None,
            path_quote_width: None,
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: false,
        };
        (id, record)
    }

    #[test]
    fn dep_hover_shows_used_by_route_count_in_header() {
        let uri: tower_lsp_server::ls_types::Uri = "file:///app.py".parse().unwrap();
        let def_range = Range {
            start: Position::new(2, 4),
            end: Position::new(2, 10),
        };

        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(DepDef {
            name: "get_db".to_owned(),
            node_id: NodeId { uri: uri.clone(), range: def_range },
            has_yield: false,
            param_names: vec![],
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        let (id, record) = make_route("list_books", "file:///app.py", vec!["get_db".to_owned()]);
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let hover = dep_hover(&state, &uri, Position::new(2, 6)).unwrap();
        let md = match hover.contents {
            tower_lsp_server::ls_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(md.contains("used by 1 route"), "header should say '1 route', got: {md}");
        assert!(md.contains("`list_books` (route)"), "used-by line should list route");
        assert!(md.contains("uses: —"), "uses line should show — when no sub-deps");
    }

    #[test]
    fn dep_hover_unused_dep_says_unused() {
        let uri: tower_lsp_server::ls_types::Uri = "file:///app.py".parse().unwrap();
        let def_range = Range {
            start: Position::new(5, 4),
            end: Position::new(5, 12),
        };
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(DepDef {
            name: "unused_dep".to_owned(),
            node_id: NodeId { uri: uri.clone(), range: def_range },
            has_yield: false,
            param_names: vec![],
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri.clone(), facts);

        let hover = dep_hover(&state, &uri, Position::new(5, 8)).unwrap();
        let md = match hover.contents {
            tower_lsp_server::ls_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(md.contains("unused"), "header should say 'unused', got: {md}");
        assert!(md.contains("used by: —"), "unused dep should show '- used by: —'");
        assert!(!md.contains("(route)"), "unused dep should not list any routes");
    }

    #[test]
    fn dep_hover_override_site_appears() {
        let uri_app: tower_lsp_server::ls_types::Uri = "file:///app.py".parse().unwrap();
        let uri_test: tower_lsp_server::ls_types::Uri =
            "file:///tests/conftest.py".parse().unwrap();
        let def_range = Range {
            start: Position::new(1, 4),
            end: Position::new(1, 10),
        };
        let override_range = Range {
            start: Position::new(10, 32),
            end: Position::new(10, 38),
        };

        let mut facts = FileFacts::new(uri_app.clone());
        facts.dep_defs.push(DepDef {
            name: "get_db".to_owned(),
            node_id: NodeId { uri: uri_app.clone(), range: def_range },
            has_yield: true,
            param_names: vec![],
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_app.clone(), facts);

        let def_node = NodeId { uri: uri_app.clone(), range: def_range };
        let mut linked = Linked::default();
        linked.dep_graph.override_sites.insert(
            def_node,
            vec![StateLocation { uri: uri_test, range: override_range }],
        );
        state.linked.store(Arc::new(linked));

        let hover = dep_hover(&state, &uri_app, Position::new(1, 6)).unwrap();
        let md = match hover.contents {
            tower_lsp_server::ls_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(md.contains("overridden in"), "should mention override site");
        assert!(md.contains("tests/conftest.py"), "should show dir/file path");
        assert!(md.contains("generator"), "should mention has_yield");
    }

    #[test]
    fn dep_hover_counts_parameter_level_depends() {
        // Route with `def handler(db = Depends(get_db))` — dep_ref in file_facts,
        // NOT in RouteRecord.dependencies (which only captures decorator-level deps).
        let uri_app: tower_lsp_server::ls_types::Uri = "file:///app.py".parse().unwrap();
        let def_range = Range {
            start: Position::new(1, 4),
            end: Position::new(1, 10),
        };
        let dep_ref_range = Range {
            start: Position::new(20, 15),
            end: Position::new(20, 21),
        };

        let mut facts = FileFacts::new(uri_app.clone());
        facts.dep_defs.push(DepDef {
            name: "get_db".to_owned(),
            node_id: NodeId { uri: uri_app.clone(), range: def_range },
            has_yield: false,
            param_names: vec![],
        });
        facts.dep_refs.push(crate::state::DepRef {
            name: "get_db".to_owned(),
            range: dep_ref_range,
            is_called: false,
            callee_range: None,
            containing_func: Some("read_items".to_owned()),
            caller_node_id: None,
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_app.clone(), facts);

        // Route exists in route_index with `read_items` as handler — no dependencies vec
        let mut linked = Linked::default();
        let (id, record) = make_route("read_items", "file:///app.py", vec![]);
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let hover = dep_hover(&state, &uri_app, Position::new(1, 6)).unwrap();
        let md = match hover.contents {
            tower_lsp_server::ls_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(md.contains("used by 1 route"), "parameter-level dep should count; got: {md}");
        assert!(md.contains("`read_items` (route)"), "should list the route using the dep");
    }

    #[test]
    fn dep_hover_counts_usage_via_type_alias() {
        // _fetch_private_contract is only referenced inside a type alias:
        //   CurrentPrivateContract = Annotated[PrivateContract, Depends(_fetch_private_contract)]
        // Routes use it as `contract: CurrentPrivateContract` — a plain_typed_param, not a dep_ref.
        // The hover must say "used by 1 route", not "unused".
        let uri: tower_lsp_server::ls_types::Uri = "file:///app/deps.py".parse().unwrap();
        let route_uri: tower_lsp_server::ls_types::Uri = "file:///app/routes.py".parse().unwrap();
        let def_range = Range { start: Position::new(3, 4), end: Position::new(3, 28) };

        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(DepDef {
            name: "_fetch_private_contract".to_owned(),
            node_id: NodeId { uri: uri.clone(), range: def_range },
            has_yield: false,
            param_names: vec![],
        });
        facts.dep_type_aliases.insert(
            "CurrentPrivateContract".to_owned(),
            "_fetch_private_contract".to_owned(),
        );

        let mut route_facts = FileFacts::new(route_uri.clone());
        route_facts.plain_typed_params.push(crate::state::PlainTypedParam {
            containing_func: "get_contract".to_owned(),
            param_name: "contract".to_owned(),
            type_name: "CurrentPrivateContract".to_owned(),
            annotation_range: Range::default(),
        });

        let state = make_state();
        state.file_facts.insert(uri.clone(), facts);
        state.file_facts.insert(route_uri.clone(), route_facts);

        let (id, record) = make_route("get_contract", route_uri.as_str(), vec![]);
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let hover = dep_hover(&state, &uri, Position::new(3, 10)).unwrap();
        let md = match hover.contents {
            tower_lsp_server::ls_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(
            md.contains("used by 1 route"),
            "dep used via type alias must not show 'unused'; got: {md}"
        );
        assert!(md.contains("`get_contract` (route)"), "should list the route; got: {md}");
    }

    #[test]
    fn dep_hover_counts_alias_usage_for_route_with_custom_name() {
        // Route has an explicit `name=` kwarg different from the handler function name.
        // The alias param's containing_func is the Python function name, while RouteRecord.name
        // is the custom route name. handler_func_to_display must map by function name so the
        // usage is found and displayed using the route name.
        let uri: tower_lsp_server::ls_types::Uri = "file:///app/deps.py".parse().unwrap();
        let route_uri: tower_lsp_server::ls_types::Uri = "file:///app/routes.py".parse().unwrap();
        let def_range = Range { start: Position::new(3, 4), end: Position::new(3, 28) };

        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(DepDef {
            name: "_fetch_contract".to_owned(),
            node_id: NodeId { uri: uri.clone(), range: def_range },
            has_yield: false,
            param_names: vec![],
        });
        facts
            .dep_type_aliases
            .insert("CurrentContract".to_owned(), "_fetch_contract".to_owned());

        let mut route_facts = FileFacts::new(route_uri.clone());
        route_facts.plain_typed_params.push(crate::state::PlainTypedParam {
            containing_func: "upload_contract_view".to_owned(),
            param_name: "contract".to_owned(),
            type_name: "CurrentContract".to_owned(),
            annotation_range: Range::default(),
        });

        // RouteId in canonical format: "{uri}:{func}:{METHOD}"
        let id = RouteId(format!("{}:upload_contract_view:POST", route_uri.as_str()));
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "contracts.upload".to_owned(), // differs from function name
            method: Method::Post,
            resolved_path: ResolvedPath::Resolved("/contracts/upload".to_owned()),
            decorator_path: "/contracts/upload".to_owned(),
            chain: vec![],
            handler: StateLocation { uri: route_uri.clone(), range: Range::default() },
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
        };

        let state = make_state();
        state.file_facts.insert(uri.clone(), facts);
        state.file_facts.insert(route_uri.clone(), route_facts);

        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let hover = dep_hover(&state, &uri, Position::new(3, 10)).unwrap();
        let md = match hover.contents {
            tower_lsp_server::ls_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(
            md.contains("used by 1 route"),
            "dep used via alias in route with custom name must show usage; got: {md}"
        );
        assert!(
            md.contains("`contracts.upload` (route)"),
            "should display the route name kwarg, not the function name; got: {md}"
        );
    }

    // ── include_hover ─────────────────────────────────────────────────────────

    fn make_state() -> Arc<crate::state::WorkspaceState> {
        crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        )
    }

    #[test]
    fn include_hover_resolves_import_alias_to_count_routes() {
        // `from app.features.projects.router import router as projects_router`
        // `app.include_router(projects_router, prefix="/v1/projects")`
        // Routes in projects/router.py use `object_name == "router"`, NOT "projects_router".
        // Without alias resolution the count would be 0.
        let main_uri: tower_lsp_server::ls_types::Uri = "file:///app/main.py".parse().unwrap();
        let router_uri: tower_lsp_server::ls_types::Uri =
            "file:///app/projects/router.py".parse().unwrap();

        let state = make_state();

        let inc_range = Range {
            start: Position::new(5, 0),
            end: Position::new(5, 50),
        };
        let mut main_facts = FileFacts::new(main_uri.clone());
        main_facts.import_alias_originals.insert("projects_router".to_owned(), "router".to_owned());
        main_facts.includes.push(crate::state::IncludeCall {
            target: "projects_router".to_owned(),
            prefix: crate::state::PrefixValue::Literal("/v1/projects".to_owned()),
            app_name: "app".to_owned(),
            dependencies: vec![],
            range: inc_range,
        });
        state.file_facts.insert(main_uri.clone(), main_facts);

        let mut router_facts = FileFacts::new(router_uri.clone());
        router_facts.routes.push(crate::state::RouteFact {
            handler_name: "list_projects".to_owned(),
            handler_range: Range::default(),
            object_name: "router".to_owned(),
            methods: vec![Method::Get],
            path: crate::state::PrefixValue::Literal("/".to_owned()),
            path_range: None,
            path_quote_width: None,
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            status_code: None,
            dependencies: vec![],
            route_name: None,
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        });
        state.file_facts.insert(router_uri.clone(), router_facts);

        let route_id = RouteId(format!("{}:list_projects:GET", router_uri.as_str()));
        let record = RouteRecord {
            id: route_id.clone(),
            ordinal: 0,
            name: "list_projects".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/v1/projects/".to_owned()),
            decorator_path: "/".to_owned(),
            chain: vec![],
            handler: StateLocation { uri: router_uri, range: Range::default() },
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
        };
        let mut linked = Linked::default();
        linked.route_index.insert(route_id, vec![record]);
        state.linked.store(Arc::new(linked));

        let hover = super::include_hover(&state, &main_uri, Position::new(5, 10)).unwrap();
        let md = match hover.contents {
            tower_lsp_server::ls_types::HoverContents::Markup(m) => m.value,
            _ => panic!("expected markup"),
        };
        assert!(
            md.contains("1 route"),
            "aliased router import must count routes from the declaring file; got: {md}"
        );
    }
}

fn make_hover(md: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: md,
        }),
        range: None,
    }
}
