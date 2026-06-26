use serde::{Deserialize, Serialize};
use tower_lsp_server::ls_types::{CodeLens, Command, Range, Uri};

use crate::state::{RouteId, WorkspaceState};

#[derive(Serialize, Deserialize)]
struct LensData {
    route_ids: Vec<String>,
}

pub fn code_lenses(state: &WorkspaceState, uri: &Uri) -> Vec<CodeLens> {
    let linked = state.linked.load();
    let mut lenses: Vec<CodeLens> = vec![];

    // ── 1. Test references (deferred) ─────────────────────────────────────────
    {
        type RangeKey = (u32, u32, u32, u32);
        type RangeMap =
            std::collections::HashMap<RangeKey, (tower_lsp_server::ls_types::Range, Vec<String>)>;
        let mut range_map: RangeMap = std::collections::HashMap::new();
        for (route_id, records) in linked.route_index.iter() {
            if linked.test_refs.get(route_id).is_none_or(|s| s.is_empty()) {
                continue;
            }
            // Use the first record per route_id to avoid double-counting multi-mount handlers.
            if let Some(record) = records.iter().find(|r| &r.handler.uri == uri) {
                let r = record.handler.range;
                let key = (r.start.line, r.start.character, r.end.line, r.end.character);
                range_map
                    .entry(key)
                    .or_insert_with(|| (r, vec![]))
                    .1
                    .push(route_id.0.clone());
            }
        }
        for (range, route_ids) in range_map.into_values() {
            lenses.push(CodeLens {
                range,
                command: None,
                data: serde_json::to_value(LensData { route_ids }).ok(),
            });
        }
    }

    // Precompute workspace-wide location maps before taking the per-file ref, to avoid
    // holding two DashMap refs in the same shard simultaneously.
    //
    // Each map: name → Vec<(uri_string, range)> — gives both count and locations for
    // the `editor.showReferences` command arguments.

    // alias_name → dep_fn_name, workspace-wide.
    // Built before dep_usage_locs so we can extend it with alias-chain usages in one pass.
    let alias_to_dep: std::collections::HashMap<String, String> = state
        .file_facts
        .iter()
        .flat_map(|e| {
            e.value()
                .dep_type_aliases
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    // dep_name → all usage locations across the workspace: direct Depends() call sites
    // plus handler params typed with a dep type alias (e.g. `db: DbSession`).
    let dep_usage_locs: std::collections::HashMap<String, Vec<(String, Range)>> = {
        let mut map: std::collections::HashMap<String, Vec<(String, Range)>> =
            std::collections::HashMap::new();
        for entry in state.file_facts.iter() {
            let uri_str = entry.key().as_str().to_owned();
            for dep_ref in &entry.value().dep_refs {
                map.entry(dep_ref.name.clone())
                    .or_default()
                    .push((uri_str.clone(), dep_ref.range));
            }
            for param in &entry.value().plain_typed_params {
                if let Some(dep_fn) = alias_to_dep.get(&param.type_name) {
                    map.entry(dep_fn.clone())
                        .or_default()
                        .push((uri_str.clone(), param.annotation_range));
                }
            }
        }
        map
    };

    // alias_type_name → all plain-typed param annotation locations across the workspace.
    let alias_usage_locs: std::collections::HashMap<String, Vec<(String, Range)>> = {
        let mut map: std::collections::HashMap<String, Vec<(String, Range)>> =
            std::collections::HashMap::new();
        for entry in state.file_facts.iter() {
            let uri_str = entry.key().as_str().to_owned();
            for param in &entry.value().plain_typed_params {
                map.entry(param.type_name.clone())
                    .or_default()
                    .push((uri_str.clone(), param.annotation_range));
            }
        }
        map
    };

    // model_name → number of routes that use it as response_model (or return annotation fallback).
    // Uses first record per RouteId only, so multi-mount routes count once.
    let model_route_counts: std::collections::HashMap<String, usize> = {
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for records in linked.route_index.values() {
            if let Some(record) = records.first() {
                let model = record
                    .response_model
                    .as_ref()
                    .or(record.return_annotation.as_ref());
                if let Some(name) = model {
                    *counts.entry(name.clone()).or_default() += 1;
                }
            }
        }
        counts
    };

    if let Some(facts) = state.file_facts.get(uri) {
        // ── 2. Dependency usage count ─────────────────────────────────────────
        for dep_def in &facts.dep_defs {
            let refs = dep_usage_locs
                .get(&dep_def.name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if refs.len() < 2 {
                continue;
            }
            let pos = dep_def.node_id.range.start;
            lenses.push(CodeLens {
                range: dep_def.node_id.range,
                command: Some(Command {
                    title: format!("{} usages", refs.len()),
                    command: "editor.showReferences".to_owned(),
                    arguments: Some(vec![
                        serde_json::Value::String(dep_def.node_id.uri.as_str().to_owned()),
                        serde_json::json!({"line": pos.line, "character": pos.character}),
                        serde_json::Value::Array(
                            refs.iter().map(|(u, r)| loc_json(u, r)).collect(),
                        ),
                    ]),
                }),
                data: None,
            });
        }

        // ── 3. Router route count ─────────────────────────────────────────────
        for router_decl in &facts.routers {
            let n = facts
                .routes
                .iter()
                .filter(|r| r.object_name == router_decl.name)
                .count();
            if n == 0 {
                continue;
            }
            let suffix = if n == 1 { "" } else { "s" };
            lenses.push(CodeLens {
                range: router_decl.range,
                command: Some(Command {
                    title: format!("{n} route{suffix}"),
                    command: "fastapi-lsp.routerRoutes".to_owned(),
                    arguments: None,
                }),
                data: None,
            });
        }

        // ── 4. Dependency cycle warning ───────────────────────────────────────
        for dep_def in &facts.dep_defs {
            if !linked.dep_cycle_map.contains_key(&dep_def.node_id) {
                continue;
            }
            lenses.push(CodeLens {
                range: dep_def.node_id.range,
                command: Some(Command {
                    title: "⚠ in dependency cycle".to_owned(),
                    command: "fastapi-lsp.depCycle".to_owned(),
                    arguments: None,
                }),
                data: None,
            });
        }

        // ── 5. Response model route usage ─────────────────────────────────────
        for model_fact in &facts.models {
            if model_fact.is_settings {
                continue; // BaseSettings subclasses are not response models
            }
            let n = model_route_counts
                .get(&model_fact.name)
                .copied()
                .unwrap_or(0);
            if n == 0 {
                continue;
            }
            let suffix = if n == 1 { "" } else { "s" };
            lenses.push(CodeLens {
                range: model_fact.range,
                command: Some(Command {
                    title: format!("used in {n} route{suffix}"),
                    command: "fastapi-lsp.modelRoutes".to_owned(),
                    arguments: None,
                }),
                data: None,
            });
        }

        // ── 6. Dependency test override count ─────────────────────────────────
        for dep_def in &facts.dep_defs {
            let n = linked
                .dep_graph
                .override_sites
                .get(&dep_def.node_id)
                .map_or(0, |v| v.len());
            if n == 0 {
                continue;
            }
            let suffix = if n == 1 { "" } else { "s" };
            lenses.push(CodeLens {
                range: dep_def.node_id.range,
                command: Some(Command {
                    title: format!("{n} test override{suffix}"),
                    command: "fastapi-lsp.depOverrides".to_owned(),
                    arguments: None,
                }),
                data: None,
            });
        }

        // ── 7. Dep type alias usage count ─────────────────────────────────────
        // Shows "N usages" above `DbSession = Annotated[T, Depends(fn)]` lines,
        // counting handler params that reference the alias as a plain type annotation.
        for (alias_name, def_range) in &facts.dep_type_alias_ranges {
            let refs = alias_usage_locs
                .get(alias_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if refs.len() < 2 {
                continue;
            }
            let pos = def_range.start;
            lenses.push(CodeLens {
                range: *def_range,
                command: Some(Command {
                    title: format!("{} usages", refs.len()),
                    command: "editor.showReferences".to_owned(),
                    arguments: Some(vec![
                        serde_json::Value::String(uri.as_str().to_owned()),
                        serde_json::json!({"line": pos.line, "character": pos.character}),
                        serde_json::Value::Array(
                            refs.iter().map(|(u, r)| loc_json(u, r)).collect(),
                        ),
                    ]),
                }),
                data: None,
            });
        }
    }

    // Stable ordering so clients that correlate lenses by position get consistent results.
    lenses.sort_by_key(|l| (l.range.start.line, l.range.start.character));
    lenses
}

fn loc_json(uri: &str, range: &Range) -> serde_json::Value {
    serde_json::json!({
        "uri": uri,
        "range": {
            "start": {"line": range.start.line, "character": range.start.character},
            "end":   {"line": range.end.line,   "character": range.end.character}
        }
    })
}

/// Fills in the command for a code lens previously returned by `code_lenses`.
pub fn resolve(state: &WorkspaceState, lens: CodeLens) -> CodeLens {
    let data = match &lens.data {
        Some(v) => v.clone(),
        None => return lens,
    };
    let lens_data: LensData = match serde_json::from_value(data) {
        Ok(d) => d,
        Err(_) => return lens,
    };

    let linked = state.linked.load();
    let n: usize = lens_data
        .route_ids
        .iter()
        .map(|id| {
            linked
                .test_refs
                .get(&RouteId(id.clone()))
                .map_or(0, |s| s.len())
        })
        .sum();

    if n == 0 {
        return lens;
    }

    let suffix = if n == 1 { "" } else { "s" };
    let ids_arg =
        serde_json::to_value(&lens_data.route_ids).unwrap_or(serde_json::Value::Array(vec![]));
    CodeLens {
        command: Some(Command {
            title: format!("▶ {n} test reference{suffix}"),
            command: "fastapi-lsp.showTestRefs".to_owned(),
            arguments: Some(vec![ids_arg]),
        }),
        ..lens
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tower_lsp_server::ls_types::{Position, Range, Uri};

    use crate::config::ResolvedConfig;
    use crate::state::{
        ClientCallSite, DepDef, DepGraph, DepRef, FileFacts, Linked, Location as StateLocation,
        Method, ModelFact, NodeId, PrefixValue, ResolvedPath, RouteFact, RouteId, RouteRecord,
        RouterDecl,
    };

    fn make_route(
        id: &str,
        name: &str,
        path: &str,
        uri: &Uri,
        handler_range: Range,
    ) -> (RouteId, RouteRecord) {
        let rid = RouteId(id.to_owned());
        let record = RouteRecord {
            id: rid.clone(),
            ordinal: 0,
            name: name.to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved(path.to_owned()),
            decorator_path: path.to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: handler_range,
            },
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
        (rid, record)
    }

    fn make_state() -> Arc<crate::state::WorkspaceState> {
        crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ))
    }

    fn make_site(uri: &Uri) -> ClientCallSite {
        ClientCallSite {
            method: Method::Get,
            path: "/path".to_owned(),
            location: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
        }
    }

    #[test]
    fn no_lenses_when_no_test_refs() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let handler_range = Range {
            start: Position::new(2, 4),
            end: Position::new(2, 14),
        };

        let state = make_state();
        let (rid, record) = make_route(
            "app.get_items:GET",
            "get_items",
            "/items",
            &uri,
            handler_range,
        );

        let mut linked = Linked::default();
        linked.route_index.insert(rid, vec![record]);
        state.linked.store(Arc::new(linked));

        assert!(code_lenses(&state, &uri).is_empty());
    }

    #[test]
    fn lens_emitted_for_route_with_test_refs() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/test_api.py".parse().unwrap();
        let handler_range = Range {
            start: Position::new(5, 4),
            end: Position::new(5, 16),
        };

        let state = make_state();
        let (rid, record) = make_route(
            "app.list_users:GET",
            "list_users",
            "/users",
            &uri,
            handler_range,
        );

        let mut linked = Linked::default();
        linked
            .test_refs
            .insert(rid.clone(), vec![make_site(&uri_test)]);
        linked.route_index.insert(rid, vec![record]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].range, handler_range);
        assert!(
            lenses[0].command.is_none(),
            "initial lens must be unresolved"
        );
        assert!(lenses[0].data.is_some());
    }

    #[test]
    fn resolve_fills_command_with_count() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/test_api.py".parse().unwrap();
        let handler_range = Range {
            start: Position::new(3, 4),
            end: Position::new(3, 12),
        };

        let state = make_state();
        let (rid, record) = make_route("app.ping:GET", "ping", "/ping", &uri, handler_range);

        let mut linked = Linked::default();
        linked
            .test_refs
            .insert(rid.clone(), vec![make_site(&uri_test)]);
        linked.route_index.insert(rid, vec![record]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        let resolved = resolve(&state, lenses.into_iter().next().unwrap());
        let cmd = resolved.command.unwrap();
        assert_eq!(cmd.title, "▶ 1 test reference");
        assert_eq!(cmd.command, "fastapi-lsp.showTestRefs");
    }

    #[test]
    fn resolve_plural_for_multiple_refs() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/test_api.py".parse().unwrap();
        let handler_range = Range {
            start: Position::new(7, 4),
            end: Position::new(7, 18),
        };

        let state = make_state();
        let (rid, record) = make_route(
            "app.get_item:GET",
            "get_item",
            "/items/{id}",
            &uri,
            handler_range,
        );

        let sites: Vec<ClientCallSite> = (0..3).map(|_| make_site(&uri_test)).collect();

        let mut linked = Linked::default();
        linked.test_refs.insert(rid.clone(), sites);
        linked.route_index.insert(rid, vec![record]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        let resolved = resolve(&state, lenses.into_iter().next().unwrap());
        assert_eq!(resolved.command.unwrap().title, "▶ 3 test references");
    }

    #[test]
    fn multi_mount_does_not_double_count_refs() {
        // The same route is mounted in two places → two records with the same handler.
        // The test reference count must be 1, not 2.
        let uri: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/test_api.py".parse().unwrap();
        let handler_range = Range {
            start: Position::new(3, 4),
            end: Position::new(3, 12),
        };

        let state = make_state();
        let (rid, record1) = make_route(
            "app.get_items:GET",
            "get_items",
            "/items",
            &uri,
            handler_range,
        );
        // Second mount: same handler, different resolved path (e.g. mounted under /v2)
        let (_, record2) = make_route(
            "app.get_items:GET",
            "get_items",
            "/v2/items",
            &uri,
            handler_range,
        );

        let mut linked = Linked::default();
        linked
            .test_refs
            .insert(rid.clone(), vec![make_site(&uri_test)]);
        linked.route_index.insert(rid, vec![record1, record2]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        assert_eq!(lenses.len(), 1);
        let resolved = resolve(&state, lenses.into_iter().next().unwrap());
        assert_eq!(
            resolved.command.unwrap().title,
            "▶ 1 test reference",
            "should not double-count multi-mount records"
        );
    }

    fn make_dep_def(name: &str, uri: &Uri, row: u32) -> DepDef {
        let range = Range {
            start: Position::new(row, 4),
            end: Position::new(row, 4 + name.len() as u32),
        };
        DepDef {
            name: name.to_owned(),
            node_id: NodeId {
                uri: uri.clone(),
                range,
            },
            has_yield: false,
            param_names: vec![],
        }
    }

    fn make_dep_ref(name: &str, _uri: &Uri) -> DepRef {
        DepRef {
            name: name.to_owned(),
            range: Range::default(),
            is_called: false,
            callee_range: None,
            containing_func: None,
            caller_node_id: None,
        }
    }

    fn make_route_fact(object_name: &str) -> RouteFact {
        RouteFact {
            handler_name: "handler".to_owned(),
            handler_range: Range::default(),
            object_name: object_name.to_owned(),
            methods: vec![Method::Get],
            path: PrefixValue::Literal("/".to_owned()),
            path_range: None,
            path_quote_width: None,
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            status_code: None,
            dependencies: vec![],
            route_name: None,
            route_name_range: None,
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        }
    }

    // ── Dep usage count ───────────────────────────────────────────────────────

    #[test]
    fn dep_usage_lens_emitted_for_used_dep() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let caller_uri: Uri = "file:///app/routes.py".parse().unwrap();

        let state = make_state();

        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(make_dep_def("get_db", &uri, 5));
        state.file_facts.insert(uri.clone(), facts);

        let mut caller_facts = FileFacts::new(caller_uri.clone());
        caller_facts
            .dep_refs
            .push(make_dep_ref("get_db", &caller_uri));
        caller_facts
            .dep_refs
            .push(make_dep_ref("get_db", &caller_uri));
        state.file_facts.insert(caller_uri, caller_facts);

        let lenses = code_lenses(&state, &uri);
        let usage_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .is_some_and(|c| c.command == "editor.showReferences")
        });
        assert!(usage_lens.is_some(), "expected a dep-usage lens");
        assert_eq!(
            usage_lens.unwrap().command.as_ref().unwrap().title,
            "2 usages"
        );
    }

    #[test]
    fn dep_usage_no_lens_when_zero_usages() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(make_dep_def("get_db", &uri, 5));
        state.file_facts.insert(uri.clone(), facts);

        let lenses = code_lenses(&state, &uri);
        assert!(
            lenses.iter().all(|l| l
                .command
                .as_ref()
                .is_none_or(|c| c.command != "editor.showReferences")),
            "no usage lens expected when dep is unreferenced"
        );
    }

    #[test]
    fn dep_usage_no_lens_for_single_usage() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(make_dep_def("get_db", &uri, 3));
        facts.dep_refs.push(make_dep_ref("get_db", &uri));
        state.file_facts.insert(uri.clone(), facts);

        let lenses = code_lenses(&state, &uri);
        assert!(
            lenses.iter().all(|l| l
                .command
                .as_ref()
                .is_none_or(|c| c.command != "editor.showReferences")),
            "single usage must not produce a lens"
        );
    }

    #[test]
    fn dep_usage_plural_label() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let caller_uri: Uri = "file:///app/routes.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(make_dep_def("get_db", &uri, 3));
        state.file_facts.insert(uri.clone(), facts);
        let mut caller = FileFacts::new(caller_uri.clone());
        caller.dep_refs.push(make_dep_ref("get_db", &caller_uri));
        caller.dep_refs.push(make_dep_ref("get_db", &caller_uri));
        state.file_facts.insert(caller_uri, caller);

        let lenses = code_lenses(&state, &uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "editor.showReferences")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "2 usages");
    }

    // ── Router route count ────────────────────────────────────────────────────

    #[test]
    fn router_route_count_lens_emitted() {
        let uri: Uri = "file:///app/router.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.routers.push(RouterDecl {
            name: "router".to_owned(),
            prefix: PrefixValue::Literal("/items".to_owned()),
            tags: vec![],
            range: Range {
                start: Position::new(2, 0),
                end: Position::new(2, 6),
            },
        });
        facts.routes.push(make_route_fact("router"));
        facts.routes.push(make_route_fact("router"));
        facts.routes.push(make_route_fact("other_router")); // different router — excluded
        state.file_facts.insert(uri.clone(), facts);

        let lenses = code_lenses(&state, &uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "fastapi-lsp.routerRoutes")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "2 routes");
    }

    #[test]
    fn router_no_lens_when_no_routes() {
        let uri: Uri = "file:///app/router.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.routers.push(RouterDecl {
            name: "router".to_owned(),
            prefix: PrefixValue::Unresolved,
            tags: vec![],
            range: Range::default(),
        });
        state.file_facts.insert(uri.clone(), facts);

        let lenses = code_lenses(&state, &uri);
        assert!(
            lenses.iter().all(|l| l
                .command
                .as_ref()
                .is_none_or(|c| c.command != "fastapi-lsp.routerRoutes")),
            "no route-count lens expected for empty router"
        );
    }

    // ── Dependency cycle warning ──────────────────────────────────────────────

    #[test]
    fn dep_cycle_lens_emitted_when_in_cycle() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let state = make_state();

        let dep_def = make_dep_def("cyclic_dep", &uri, 10);
        let node_id = dep_def.node_id.clone();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(dep_def);
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        linked.dep_cycle_map.insert(node_id, vec![]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "fastapi-lsp.depCycle")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "⚠ in dependency cycle");
    }

    #[test]
    fn dep_cycle_no_lens_when_not_in_cycle() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(make_dep_def("safe_dep", &uri, 5));
        state.file_facts.insert(uri.clone(), facts);

        let lenses = code_lenses(&state, &uri);
        assert!(
            lenses.iter().all(|l| l
                .command
                .as_ref()
                .is_none_or(|c| c.command != "fastapi-lsp.depCycle")),
            "no cycle lens for dep not in any cycle"
        );
    }

    // ── Response model route usage ────────────────────────────────────────────

    fn make_route_record_with_model(id: &str, uri: &Uri, model: &str) -> (RouteId, RouteRecord) {
        let rid = RouteId(id.to_owned());
        let record = RouteRecord {
            id: rid.clone(),
            ordinal: 0,
            name: id.to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/items".to_owned()),
            decorator_path: "/items".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
            path_params: vec![],
            response_model: Some(model.to_owned()),
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
        (rid, record)
    }

    #[test]
    fn model_lens_emitted_when_used_as_response_model() {
        let uri: Uri = "file:///app/models.py".parse().unwrap();
        let route_uri: Uri = "file:///app/routes.py".parse().unwrap();
        let state = make_state();

        let mut facts = FileFacts::new(uri.clone());
        facts.models.push(ModelFact {
            name: "Item".to_owned(),
            range: Range {
                start: Position::new(4, 0),
                end: Position::new(4, 4),
            },
            is_settings: false,
        });
        state.file_facts.insert(uri.clone(), facts);

        let (rid1, rec1) = make_route_record_with_model("r1:GET", &route_uri, "Item");
        let (rid2, rec2) = make_route_record_with_model("r2:GET", &route_uri, "Item");
        let mut linked = Linked::default();
        linked.route_index.insert(rid1, vec![rec1]);
        linked.route_index.insert(rid2, vec![rec2]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "fastapi-lsp.modelRoutes")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "used in 2 routes");
    }

    #[test]
    fn model_lens_uses_return_annotation_fallback() {
        let uri: Uri = "file:///app/models.py".parse().unwrap();
        let route_uri: Uri = "file:///app/routes.py".parse().unwrap();
        let state = make_state();

        let mut facts = FileFacts::new(uri.clone());
        facts.models.push(ModelFact {
            name: "Widget".to_owned(),
            range: Range::default(),
            is_settings: false,
        });
        state.file_facts.insert(uri.clone(), facts);

        // Route has no response_model but has return_annotation = "Widget"
        let rid = RouteId("r:GET".to_owned());
        let mut rec = make_route_record_with_model("r:GET", &route_uri, "SomethingElse").1;
        rec.response_model = None;
        rec.return_annotation = Some("Widget".to_owned());
        let mut linked = Linked::default();
        linked.route_index.insert(rid, vec![rec]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "fastapi-lsp.modelRoutes")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "used in 1 route");
    }

    #[test]
    fn settings_model_no_lens() {
        let uri: Uri = "file:///app/config.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.models.push(ModelFact {
            name: "AppSettings".to_owned(),
            range: Range::default(),
            is_settings: true, // BaseSettings subclass — must not get a model lens
        });
        state.file_facts.insert(uri.clone(), facts);

        let (rid, rec) = make_route_record_with_model("r:GET", &uri, "AppSettings");
        let mut linked = Linked::default();
        linked.route_index.insert(rid, vec![rec]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        assert!(
            lenses.iter().all(|l| l
                .command
                .as_ref()
                .is_none_or(|c| c.command != "fastapi-lsp.modelRoutes")),
            "BaseSettings subclass must not get a response-model lens"
        );
    }

    // ── Dependency test override count ────────────────────────────────────────

    #[test]
    fn dep_override_lens_emitted() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let test_uri: Uri = "file:///tests/conftest.py".parse().unwrap();
        let state = make_state();

        let dep_def = make_dep_def("get_db", &uri, 8);
        let node_id = dep_def.node_id.clone();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(dep_def);
        state.file_facts.insert(uri.clone(), facts);

        let override_loc = StateLocation {
            uri: test_uri,
            range: Range::default(),
        };
        let mut dep_graph = DepGraph::default();
        dep_graph
            .override_sites
            .insert(node_id, vec![override_loc.clone(), override_loc]);
        state.linked.store(Arc::new(Linked {
            dep_graph,
            ..Default::default()
        }));

        let lenses = code_lenses(&state, &uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "fastapi-lsp.depOverrides")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "2 test overrides");
    }

    #[test]
    fn dep_override_singular_label() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let test_uri: Uri = "file:///tests/conftest.py".parse().unwrap();
        let state = make_state();

        let dep_def = make_dep_def("get_db", &uri, 8);
        let node_id = dep_def.node_id.clone();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_defs.push(dep_def);
        state.file_facts.insert(uri.clone(), facts);

        let mut dep_graph = DepGraph::default();
        dep_graph.override_sites.insert(
            node_id,
            vec![StateLocation {
                uri: test_uri,
                range: Range::default(),
            }],
        );
        state.linked.store(Arc::new(Linked {
            dep_graph,
            ..Default::default()
        }));

        let lenses = code_lenses(&state, &uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "fastapi-lsp.depOverrides")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "1 test override");
    }

    // ── Dep type alias usage count ────────────────────────────────────────────

    #[test]
    fn alias_usage_lens_emitted() {
        // DbSession = typing.Annotated[AsyncSession, fastapi.Depends(get_dbsession)]
        // used as `dbsession: DbSession` in handler params → plain_typed_params
        let alias_uri: Uri = "file:///app/common/db/dependencies.py".parse().unwrap();
        let handler_uri: Uri = "file:///app/features/projects/router.py".parse().unwrap();
        let state = make_state();

        let alias_range = Range {
            start: Position::new(11, 0),
            end: Position::new(11, 60),
        };
        let mut alias_facts = FileFacts::new(alias_uri.clone());
        alias_facts
            .dep_type_alias_ranges
            .insert("DbSession".to_owned(), alias_range);
        state.file_facts.insert(alias_uri.clone(), alias_facts);

        let mut handler_facts = FileFacts::new(handler_uri.clone());
        for i in 0..12u32 {
            handler_facts
                .plain_typed_params
                .push(crate::state::PlainTypedParam {
                    containing_func: "some_handler".to_owned(),
                    param_name: "dbsession".to_owned(),
                    type_name: "DbSession".to_owned(),
                    annotation_range: Range {
                        start: Position::new(i + 10, 15),
                        end: Position::new(i + 10, 24),
                    },
                });
        }
        state.file_facts.insert(handler_uri, handler_facts);

        let lenses = code_lenses(&state, &alias_uri);
        let l = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .is_some_and(|c| c.command == "editor.showReferences")
            })
            .unwrap();
        assert_eq!(l.command.as_ref().unwrap().title, "12 usages");
        assert_eq!(l.range, alias_range);
    }

    #[test]
    fn alias_no_lens_when_unused() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts.dep_type_alias_ranges.insert(
            "UnusedAlias".to_owned(),
            Range {
                start: Position::new(5, 0),
                end: Position::new(5, 40),
            },
        );
        state.file_facts.insert(uri.clone(), facts);

        let lenses = code_lenses(&state, &uri);
        assert!(
            lenses.iter().all(|l| l
                .command
                .as_ref()
                .is_none_or(|c| c.command != "editor.showReferences")),
            "no alias lens when no handler params use it"
        );
    }

    #[test]
    fn alias_no_lens_when_single_usage() {
        let uri: Uri = "file:///app/deps.py".parse().unwrap();
        let state = make_state();
        let mut facts = FileFacts::new(uri.clone());
        facts
            .dep_type_alias_ranges
            .insert("MyAlias".to_owned(), Range::default());
        facts
            .plain_typed_params
            .push(crate::state::PlainTypedParam {
                containing_func: "handler".to_owned(),
                param_name: "x".to_owned(),
                type_name: "MyAlias".to_owned(),
                annotation_range: Range::default(),
            });
        state.file_facts.insert(uri.clone(), facts);

        let lenses = code_lenses(&state, &uri);
        assert!(
            lenses.iter().all(|l| l
                .command
                .as_ref()
                .is_none_or(|c| c.command != "editor.showReferences")),
            "single usage must not produce an alias lens"
        );
    }

    #[test]
    fn deduplicates_lenses_by_handler_range_and_merges_counts() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/test_api.py".parse().unwrap();
        let handler_range = Range {
            start: Position::new(4, 4),
            end: Position::new(4, 14),
        };

        let state = make_state();
        let (rid1, rec1) = make_route("app.handler:GET", "handler", "/path", &uri, handler_range);
        let (rid2, rec2) = make_route("app.handler:POST", "handler", "/path", &uri, handler_range);

        let mut linked = Linked::default();
        linked
            .test_refs
            .insert(rid1.clone(), vec![make_site(&uri_test)]);
        linked.test_refs.insert(
            rid2.clone(),
            vec![make_site(&uri_test), make_site(&uri_test)],
        );
        linked.route_index.insert(rid1, vec![rec1]);
        linked.route_index.insert(rid2, vec![rec2]);
        state.linked.store(Arc::new(linked));

        let lenses = code_lenses(&state, &uri);
        assert_eq!(
            lenses.len(),
            1,
            "duplicate handler ranges should yield one lens"
        );

        // Resolved count must sum refs from both routes (1 + 2 = 3)
        let resolved = resolve(&state, lenses.into_iter().next().unwrap());
        let cmd = resolved.command.unwrap();
        assert_eq!(cmd.title, "▶ 3 test references");
    }
}
