use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp_server::ls_types::Uri;

use crate::state::{
    ClientCallSite, DepGraph, EnvEntry, FileFacts, IncludeCall, Linked, Location, Method,
    MiddlewareCall, MwInit, MwKwarg, MwSource, NodeId, PathTrie, PrefixValue, ResolvedPath,
    RouteId, RouteRecord, RouterDecl, SettingsClassDecl, SettingsField, TrieNode, WorkspaceState,
};

pub async fn relink(state: &Arc<WorkspaceState>) {
    let generation_before = state.current_generation();
    let (template_roots, settings_env_files) = {
        let cfg = state.config.read().await;
        (cfg.template_roots.clone(), cfg.settings_env_files.clone())
    };
    let new_linked = build(state, generation_before, &template_roots, &settings_env_files);

    if state.current_generation() != generation_before {
        tracing::debug!("generation moved during link, discarding stale snapshot");
        return;
    }

    state.linked.store(Arc::new(new_linked));
    tracing::debug!("linked generation {generation_before}");
}

fn build(state: &WorkspaceState, generation: u64, template_roots: &[std::path::PathBuf], settings_env_files: &[String]) -> Linked {
    // Collect all facts into indexed lookups
    let ctx = LinkContext::from_state(state);

    let mut route_index: HashMap<RouteId, Vec<RouteRecord>> = HashMap::new();
    let mut route_names: HashMap<String, Vec<RouteId>> = HashMap::new();
    let mut ordinal = 0u32;

    // Sort by URI then handler start line for stable ordinal assignment across runs.
    // DashMap iteration order is non-deterministic (hash-dependent), so we must sort here
    // before assigning ordinals that are later used in route/duplicate and route/shadowed
    // comparisons.
    let mut all_facts: Vec<_> = state.file_facts.iter().collect();
    all_facts.sort_by(|a, b| a.key().as_str().cmp(b.key().as_str()));

    for entry in all_facts {
        let facts = entry.value();
        for route in &facts.routes {
            for method in &route.methods {
                let id = RouteId::new(&facts.uri, &route.handler_name, method);

                // Follow include chain — may branch when a router is
                // included in multiple places (xrm.3). Deduplicate paths to
                // prevent N identical records when many aliased routers share
                // the same object name (e.g. `from X import router as X_router`).
                let resolved_paths = {
                    let mut paths = ctx.resolve_route_paths(
                        &facts.uri,
                        &route.object_name,
                        &route.path,
                    );
                    let mut seen = std::collections::HashSet::new();
                    paths.retain(|p| seen.insert(p.clone()));
                    paths
                };

                let route_name = route.route_name.clone()
                    .unwrap_or_else(|| route.handler_name.clone());
                let decorator_path = match &route.path {
                    PrefixValue::Literal(p) => p.clone(),
                    PrefixValue::Unresolved => String::new(),
                };
                let path_params = crate::util::extract_path_params(&decorator_path);

                // Index by route name once per id (REQ-ROUTE-10).
                // MOUNT routes only enter route_names when explicitly named — unnamed terminal
                // mounts would otherwise pollute route_names with handler class names like
                // "StaticFiles", triggering false url/unknown-name suppressions.
                let add_to_route_names = *method != Method::Mount || route.route_name.is_some();
                if add_to_route_names {
                    route_names.entry(route_name.clone()).or_default().push(id.clone());
                }

                let middleware = ctx.middleware_chain(&route.object_name);

                for resolved in resolved_paths {
                    let record = RouteRecord {
                        id: id.clone(),
                        ordinal,
                        name: route_name.clone(),
                        method: method.clone(),
                        resolved_path: resolved,
                        decorator_path: decorator_path.clone(),
                        chain: vec![],
                        handler: Location {
                            uri: facts.uri.clone(),
                            range: route.handler_range,
                        },
                        path_params: path_params.clone(),
                        response_model: route.response_model.clone(),
                        response_model_range: route.response_model_range,
                        return_annotation: route.return_annotation.clone(),
                        dependencies: route.dependencies.clone(),
                        middleware: middleware.clone(),
                        path_range: route.path_range,
                        path_quote_width: route.path_quote_width,
                        handler_params: route.handler_params.clone(),
                        handler_param_ranges: route.handler_param_ranges.clone(),
                        params_insert_pos: route.params_insert_pos,
                        handler_has_splat_args: route.handler_has_splat_args,
                        handler_params_known: route.handler_params_known,
                    };

                    route_index.entry(id.clone()).or_default().push(record);
                    ordinal += 1;
                }
            }
        }
    }

    // Sort route_names vecs so lookup order is deterministic (RouteId Ord = lexicographic on URI)
    for ids in route_names.values_mut() {
        ids.sort_unstable();
    }

    let env_index = build_env_index(state);
    let env_file_keys = build_env_file_keys(state, settings_env_files);
    let middleware_classes = build_middleware_classes(state);
    let dep_graph = build_dep_graph(state);
    let dep_cycle_map = build_cycle_map(&dep_graph);
    let path_trie = build_path_trie(&route_index);
    let test_refs = build_test_refs(state, &path_trie, &route_index);

    // Inverted index: call-site location → RouteIds (O(1) goto lookup)
    let mut call_site_index: HashMap<(Uri, tower_lsp_server::ls_types::Range), Vec<RouteId>> =
        HashMap::new();
    for (route_id, sites) in &test_refs {
        for site in sites {
            call_site_index
                .entry((site.location.uri.clone(), site.location.range))
                .or_default()
                .push(route_id.clone());
        }
    }

    let model_index = build_model_index(state);
    let template_index = build_template_index(template_roots);
    let (proven_dep_names, dep_params) = build_dep_caches(state);

    Linked {
        generation,
        route_index,
        route_names,
        path_trie,
        dep_graph,
        dep_cycle_map,
        template_index,
        model_index,
        env_index,
        env_file_keys,
        middleware_classes,
        test_refs,
        call_site_index,
        proven_dep_names,
        dep_params,
    }
}

/// Build a set of env keys from files whose basename is in `settings_env_files`.
fn build_env_file_keys(
    state: &WorkspaceState,
    settings_env_files: &[String],
) -> std::collections::HashSet<String> {
    let mut keys = std::collections::HashSet::new();
    for entry in state.env_file_entries.iter() {
        let uri = entry.key();
        let filename = uri.as_str().rsplit('/').next().unwrap_or("");
        if settings_env_files.iter().any(|f| {
            std::path::Path::new(f)
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == filename)
                .unwrap_or(false)
        }) {
            for dotenv_entry in entry.value().iter() {
                keys.insert(dotenv_entry.key.clone());
            }
        }
    }
    keys
}

fn build_dep_caches(state: &WorkspaceState) -> (std::collections::HashSet<String>, HashMap<String, Vec<String>>) {
    let mut proven = std::collections::HashSet::new();
    let mut dep_params: HashMap<String, Vec<String>> = HashMap::new();
    for entry in state.file_facts.iter() {
        let facts = entry.value();
        for def in &facts.dep_defs {
            if def.has_yield {
                proven.insert(def.name.clone());
            }
            dep_params.entry(def.name.clone()).or_insert_with(|| def.param_names.clone());
        }
        for dep_ref in &facts.dep_refs {
            if !dep_ref.is_called && !dep_ref.name.is_empty() {
                proven.insert(dep_ref.name.clone());
            }
        }
        // Deps referenced only inside a type alias (e.g. `Alias = Annotated[T, Depends(fn)]`)
        // have no dep_refs, but are genuine deps — mark them proven via the alias map.
        for fn_name in facts.dep_type_aliases.values() {
            if !fn_name.is_empty() {
                proven.insert(fn_name.clone());
            }
        }
    }
    (proven, dep_params)
}

fn build_model_index(state: &WorkspaceState) -> HashMap<String, Vec<crate::state::ModelRecord>> {
    let mut index: HashMap<String, Vec<crate::state::ModelRecord>> = HashMap::new();
    for entry in state.file_facts.iter() {
        let facts = entry.value();
        for model in &facts.models {
            index.entry(model.name.clone()).or_default().push(crate::state::ModelRecord {
                name: model.name.clone(),
                location: crate::state::Location {
                    uri: facts.uri.clone(),
                    range: model.range,
                },
                is_settings: model.is_settings,
            });
        }
    }
    index
}

/// Build the template index: maps root-relative path (with `/` separators) → file URI.
/// Higher-precedence roots come first; first-entry wins on path collision (Jinja2 first-match
/// loader semantics, REQ-TPL-02).
fn build_template_index(template_roots: &[std::path::PathBuf]) -> HashMap<String, Uri> {
    let mut index: HashMap<String, Uri> = HashMap::new();
    for root in template_roots {
        if !root.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| match e {
                Ok(e) => Some(e),
                Err(err) => { tracing::warn!("template root scan error: {err}"); None }
            })
            .filter(|e| e.file_type().is_file())
        {
            if let Ok(rel) = entry.path().strip_prefix(root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if !index.contains_key(&rel_str)
                    && let Some(uri) = crate::uri::path_to_uri(entry.path()) {
                        index.insert(rel_str, uri);
                    }
            }
        }
    }
    index
}

// ── Link context: cross-file indexes ─────────────────────────────────────────

struct LinkContext {
    /// router_name → Vec<(RouterDecl, source_uri)>  — all declarations with that name
    routers: HashMap<String, Vec<(RouterDecl, Uri)>>,
    /// app_names (FastAPI / Starlette instances)
    apps: std::collections::HashSet<String>,
    /// target_name → Vec<(IncludeCall, source_uri)>
    includes_by_target: HashMap<String, Vec<(IncludeCall, Uri)>>,
    /// object_name → Vec<MiddlewareCall> (in source order; reversed at use site for add_middleware)
    middlewares_by_obj: HashMap<String, Vec<MiddlewareCall>>,
}

impl LinkContext {
    fn from_state(state: &WorkspaceState) -> Self {
        let mut routers: HashMap<String, Vec<(RouterDecl, Uri)>> = HashMap::new();
        let mut apps = std::collections::HashSet::new();
        let mut includes_by_target: HashMap<String, Vec<(IncludeCall, Uri)>> = HashMap::new();

        for entry in state.file_facts.iter() {
            let facts: &FileFacts = entry.value();
            let uri = entry.key().clone();

            for app in &facts.apps {
                apps.insert(app.name.clone());
            }

            for router in &facts.routers {
                routers.entry(router.name.clone()).or_default().push((router.clone(), uri.clone()));
            }

            for inc in &facts.includes {
                // Index by the full target text and by the last component (suffix match)
                let key = inc.target.clone();
                includes_by_target.entry(key).or_default().push((inc.clone(), uri.clone()));

                // If target is dotted (books.router), also index by the last segment
                if let Some(suffix) = inc.target.rsplit('.').next()
                    && suffix != inc.target {
                        includes_by_target
                            .entry(suffix.to_owned())
                            .or_default()
                            .push((inc.clone(), uri.clone()));
                    }

                // If the include target is an import alias (e.g. `router as projects_router`),
                // also index under the original name so routes that use the original name resolve.
                // Example: `from projects.router import router as projects_router` +
                // `app.include_router(projects_router)` → also index under "router".
                if let Some(original) = facts.import_alias_originals.get(&inc.target) {
                    includes_by_target
                        .entry(original.clone())
                        .or_default()
                        .push((inc.clone(), uri.clone()));
                }
            }
        }

        // Build middleware index: object_name → Vec<MiddlewareCall> in source order
        let mut middlewares_by_obj: HashMap<String, Vec<MiddlewareCall>> = HashMap::new();
        for entry in state.file_facts.iter() {
            let facts: &FileFacts = entry.value();
            for mw in &facts.middlewares {
                if !mw.app_name.is_empty() {
                    middlewares_by_obj
                        .entry(mw.app_name.clone())
                        .or_default()
                        .push(mw.clone());
                }
            }
        }

        Self { routers, apps, includes_by_target, middlewares_by_obj }
    }

    /// Compute the middleware chain for a route registered on `object_name`.
    /// Returns names in execution order (outermost first):
    ///   app-level (reversed source) → router-level (reversed source) → ...
    /// Each level's add_middleware registrations are reversed (prepend semantics).
    fn middleware_chain(&self, object_name: &str) -> Vec<String> {
        // BFS from innermost (route's router) to outermost (app), branching over ALL include sites.
        let mut levels: Vec<Vec<String>> = vec![];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

        queue.push_back(object_name.to_owned());
        seen.insert(object_name.to_owned());

        while let Some(current) = queue.pop_front() {
            // Collect middleware at this level (source order), then reverse for prepend semantics
            if let Some(mws) = self.middlewares_by_obj.get(&current) {
                let mut names: Vec<String> = mws
                    .iter()
                    .map(|m| match &m.source {
                        MwSource::Class(n) => n.clone(),
                        MwSource::DecoratorFn(n) => n.clone(),
                    })
                    .collect();
                names.reverse();
                if !names.is_empty() {
                    levels.push(names);
                }
            }

            if self.apps.contains(&current) {
                continue;
            }

            // Branch over ALL include sites (not just first)
            if let Some(includes) = self.includes_by_target.get(&current) {
                for (inc, _) in includes {
                    if !inc.app_name.is_empty() && seen.insert(inc.app_name.clone()) {
                        queue.push_back(inc.app_name.clone());
                    }
                }
            }
        }

        // Reverse levels so outermost (app) comes first in execution order
        levels.reverse();
        levels.into_iter().flatten().collect()
    }

    /// Resolve all full paths for a route registered on `object_name` with
    /// decorator `path`. Returns one entry per mount site (multiple when a
    /// router is included in more than one place — REQ-ROUTE-12 / xrm.3).
    fn resolve_route_paths(
        &self,
        route_uri: &Uri,
        object_name: &str,
        path: &PrefixValue,
    ) -> Vec<ResolvedPath> {
        let route_segment = match path {
            PrefixValue::Literal(p) => p.clone(),
            PrefixValue::Unresolved => return vec![ResolvedPath::Unresolved],
        };
        self.resolve_paths_from(object_name, route_uri, vec![route_segment], 0)
    }

    /// Find the router declaration for `name`, preferring `prefer_uri` when
    /// multiple files define a variable with the same name.
    fn find_router<'a>(&'a self, name: &str, prefer_uri: &Uri) -> Option<&'a RouterDecl> {
        let entries = self.routers.get(name)?;
        if let Some((r, _)) = entries.iter().find(|(_, u)| u == prefer_uri) {
            return Some(r);
        }
        if entries.len() == 1 {
            Some(&entries[0].0)
        } else {
            None
        }
    }

    /// Recursive helper: accumulates prefix segments walking up the include
    /// chain and branches whenever a router has multiple include sites.
    fn resolve_paths_from(
        &self,
        object_name: &str,
        prefer_uri: &Uri,
        segments: Vec<String>,
        depth: usize,
    ) -> Vec<ResolvedPath> {
        if depth > 32 {
            return vec![ResolvedPath::Unresolved];
        }

        if self.apps.contains(object_name) {
            let mut rev = segments;
            rev.reverse();
            return vec![ResolvedPath::Resolved(join_path_segments(&rev))];
        }

        // Push the router's own prefix before climbing to its include sites
        let mut segments = segments;
        if let Some(router) = self.find_router(object_name, prefer_uri) {
            match &router.prefix {
                PrefixValue::Literal(p) => {
                    if !p.is_empty() {
                        segments.push(p.clone());
                    }
                }
                PrefixValue::Unresolved => return vec![ResolvedPath::Unresolved],
            }
        }

        let Some(includes) = self.includes_by_target.get(object_name) else {
            // Router not yet included anywhere (REQ-ROUTE-05)
            return vec![ResolvedPath::Unresolved];
        };

        // Branch: one resolved path per include site
        let mut results = Vec::new();
        for (inc, inc_uri) in includes.iter() {
            let mut branch = segments.clone();
            match &inc.prefix {
                PrefixValue::Literal(p) => {
                    if !p.is_empty() {
                        branch.push(p.clone());
                    }
                }
                PrefixValue::Unresolved => {
                    results.push(ResolvedPath::Unresolved);
                    continue;
                }
            }
            if inc.app_name.is_empty() {
                results.push(ResolvedPath::Unresolved);
                continue;
            }
            // The include call's app_name refers to a name in inc_uri's file
            results.extend(self.resolve_paths_from(&inc.app_name, inc_uri, branch, depth + 1));
        }

        if results.is_empty() { vec![ResolvedPath::Unresolved] } else { results }
    }
}

/// Join path segments, collapsing any doubled `//` at joins (REQ-ROUTE-04).
fn join_path_segments(segments: &[String]) -> String {
    let mut result = String::new();
    for seg in segments {
        if result.is_empty() {
            result.push_str(seg);
        } else if result.ends_with('/') && seg.starts_with('/') {
            result.push_str(&seg[1..]);
        } else if !result.ends_with('/') && !seg.starts_with('/') {
            result.push('/');
            result.push_str(seg);
        } else {
            result.push_str(seg);
        }
    }
    result
}

// ── Path parameter extraction ─────────────────────────────────────────────────
// (also registered in util.rs)

// ── Env index ────────────────────────────────────────────────────────────────

fn build_env_index(state: &WorkspaceState) -> HashMap<String, EnvEntry> {
    let mut index: HashMap<String, EnvEntry> = HashMap::new();

    // Sort by URI for deterministic first-wins precedence (mirrors template_index behaviour)
    let mut entries: Vec<_> = state.env_file_entries.iter().collect();
    entries.sort_by(|a, b| a.key().as_str().cmp(b.key().as_str()));

    for entry in entries {
        let uri = entry.key();
        let dotenv_entries = entry.value();
        for (key, new_entry) in crate::parsing::dotenv::into_env_entries(dotenv_entries, uri) {
            index
                .entry(key)
                .and_modify(|existing| existing.locations.extend(new_entry.locations.clone()))
                .or_insert(new_entry);
        }
    }

    // Build a cross-file map of settings class name → (uri, decl) for inheritance resolution.
    let class_map: HashMap<String, (String, SettingsClassDecl)> = state
        .file_facts
        .iter()
        .flat_map(|entry| {
            let uri = entry.key().as_str().to_owned();
            entry
                .value()
                .settings_classes
                .iter()
                .map(|cls| (cls.class_name.clone(), (uri.clone(), cls.clone())))
                .collect::<Vec<_>>()
        })
        .collect();

    // For each settings class, collect own + inherited fields (cycle-safe) and index by env key.
    for class_name in class_map.keys() {
        let mut visited = std::collections::HashSet::new();
        for (uri_str, field) in collect_settings_fields(class_name, &class_map, &mut visited) {
            let Some(key) = &field.env_key else { continue };
            let Ok(field_uri) = uri_str.parse::<Uri>() else { continue };
            let new_entry = EnvEntry {
                value: String::new(),
                locations: vec![Location { uri: field_uri, range: field.range }],
                from_process_env: false,
            };
            index
                .entry(key.clone())
                .and_modify(|e| {
                    if !e.locations.iter().any(|l| l.range == new_entry.locations[0].range
                        && l.uri == new_entry.locations[0].uri) {
                        e.locations.extend(new_entry.locations.clone());
                    }
                })
                .or_insert(new_entry);
        }
    }

    index
}

/// Recursively collect fields from a settings class and its recognized superclasses.
/// `visited` prevents cycles in the inheritance graph.
fn collect_settings_fields(
    class_name: &str,
    class_map: &HashMap<String, (String, SettingsClassDecl)>,
    visited: &mut std::collections::HashSet<String>,
) -> Vec<(String, SettingsField)> {
    if !visited.insert(class_name.to_owned()) {
        return vec![];
    }
    let Some((uri_str, cls)) = class_map.get(class_name) else {
        return vec![];
    };
    let mut fields: Vec<(String, SettingsField)> = cls
        .fields
        .iter()
        .map(|f| (uri_str.clone(), f.clone()))
        .collect();
    for parent in &cls.superclass_names {
        fields.extend(collect_settings_fields(parent, class_map, visited));
    }
    fields
}

// ── Middleware class index ────────────────────────────────────────────────────

fn build_middleware_classes(state: &WorkspaceState) -> HashMap<String, Vec<MwInit>> {
    let mut index: HashMap<String, Vec<MwInit>> = HashMap::new();

    for entry in state.file_facts.iter() {
        let facts = entry.value();
        for cls in &facts.mw_classes {
            index.entry(cls.class_name.clone()).or_default().push(MwInit {
                location: Location {
                    uri: facts.uri.clone(),
                    range: cls.range,
                },
                kwargs: cls.kwargs.clone(),
            });
        }
    }

    for (class_name, kwargs) in crate::parsing::middleware::STOCK_MIDDLEWARE {
        if !index.contains_key(*class_name)
            && let Ok(sentinel_uri) = "file:///builtins/starlette".parse::<Uri>() {
                index.entry(class_name.to_string()).or_default().push(MwInit {
                    location: Location {
                        uri: sentinel_uri,
                        range: Default::default(),
                    },
                    kwargs: kwargs.iter().map(|(name, detail)| MwKwarg {
                        name: name.to_string(),
                        detail: Some(detail.to_string()),
                    }).collect(),
                });
            }
    }

    index
}

// ── Dep graph ────────────────────────────────────────────────────────────────

/// Build the bidirectional dependency graph (REQ-DI-02, REQ-IDX-04).
///
/// Name resolution is local-first then unique-workspace: if a name resolves to
/// exactly one definition in the workspace, that definition is used. Ambiguous
/// names (multiple definitions) and names not found in the workspace produce no
/// edge (P4 — unbindable names are silent).
fn build_dep_graph(state: &WorkspaceState) -> DepGraph {
    // Step 1: Build name → Vec<NodeId> index across the whole workspace.
    let mut def_index: HashMap<String, Vec<NodeId>> = HashMap::new();
    for entry in state.file_facts.iter() {
        let facts = entry.value();
        for def in &facts.dep_defs {
            def_index.entry(def.name.clone()).or_default().push(def.node_id.clone());
        }
    }

    let mut uses: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let mut used_by: HashMap<NodeId, Vec<NodeId>> = HashMap::new();

    for entry in state.file_facts.iter() {
        let facts = entry.value();

        for dep_ref in &facts.dep_refs {
            if dep_ref.name.is_empty() {
                continue;
            }
            let callee_id = match resolve_def(&dep_ref.name, &facts.uri, &def_index) {
                Some(id) => id,
                None => continue,
            };

            // Find the caller: prefer the precise NodeId emitted by the parser;
            // fall back to first-name-match for DepRefs constructed without it.
            let caller_id = if let Some(id) = dep_ref.caller_node_id.clone() {
                Some(id)
            } else {
                match &dep_ref.containing_func {
                    Some(fname) => facts.dep_defs.iter()
                        .find(|d| d.name == *fname)
                        .map(|d| d.node_id.clone()),
                    None => None,
                }
            };

            let caller_id = match caller_id {
                Some(id) => id,
                None => continue, // module-scope Depends or caller not found
            };

            uses.entry(caller_id.clone()).or_default().push(callee_id.clone());
            used_by.entry(callee_id).or_default().push(caller_id);
        }
    }

    // Dedup edges — multiple Depends(same_fn) in one function would otherwise
    // insert the same target NodeId more than once.
    for v in uses.values_mut() {
        let mut seen = std::collections::HashSet::new();
        v.retain(|id| seen.insert(id.clone()));
    }
    for v in used_by.values_mut() {
        let mut seen = std::collections::HashSet::new();
        v.retain(|id| seen.insert(id.clone()));
    }

    // Build override_sites: dep NodeId → locations where it is overridden (REQ-DI-05)
    let mut override_sites: HashMap<NodeId, Vec<Location>> = HashMap::new();
    for entry in state.file_facts.iter() {
        let facts = entry.value();
        for site in &facts.override_sites {
            if let Some(node_id) = resolve_def(&site.name, &facts.uri, &def_index) {
                override_sites.entry(node_id).or_default().push(Location {
                    uri: facts.uri.clone(),
                    range: site.range,
                });
            }
        }
    }

    DepGraph { uses, used_by, override_sites }
}

// ── Cycle detection (Tarjan's SCC) ───────────────────────────────────────────

/// Compute a map of cycle-member NodeId → ordered cycle (REQ-DI-04).
/// Only SCCs of size > 1 or self-loops are included.
pub fn build_cycle_map(dep_graph: &DepGraph) -> HashMap<NodeId, Vec<NodeId>> {
    let sccs = tarjan_sccs(&dep_graph.uses);
    let mut map = HashMap::new();
    for scc in sccs {
        for node in &scc {
            map.insert(node.clone(), scc.clone());
        }
    }
    map
}

fn tarjan_sccs(uses: &HashMap<NodeId, Vec<NodeId>>) -> Vec<Vec<NodeId>> {
    let mut state = TarjanState::default();
    // Visit all nodes that appear as sources or targets
    let all_nodes: std::collections::HashSet<&NodeId> = uses.keys()
        .chain(uses.values().flatten())
        .collect();
    for node in all_nodes {
        if !state.indices.contains_key(node) {
            tarjan_visit(node, uses, &mut state);
        }
    }
    // Keep only cycles: SCCs of size > 1 or self-loops
    state.sccs.into_iter().filter(|scc| {
        scc.len() > 1
            || uses.get(&scc[0]).map(|ns| ns.contains(&scc[0])).unwrap_or(false)
    }).collect()
}

#[derive(Default)]
struct TarjanState {
    counter: usize,
    stack: Vec<NodeId>,
    on_stack: std::collections::HashSet<NodeId>,
    indices: HashMap<NodeId, usize>,
    lowlinks: HashMap<NodeId, usize>,
    sccs: Vec<Vec<NodeId>>,
}

fn tarjan_visit(start: &NodeId, uses: &HashMap<NodeId, Vec<NodeId>>, s: &mut TarjanState) {
    struct Frame {
        v: NodeId,
        neighbors: Vec<NodeId>,
        ni: usize,
    }

    s.indices.insert(start.clone(), s.counter);
    s.lowlinks.insert(start.clone(), s.counter);
    s.counter += 1;
    s.stack.push(start.clone());
    s.on_stack.insert(start.clone());
    let mut work: Vec<Frame> = vec![Frame {
        neighbors: uses.get(start).cloned().unwrap_or_default(),
        v: start.clone(),
        ni: 0,
    }];

    loop {
        let step = match work.last_mut() {
            None => break,
            Some(frame) => {
                let v = frame.v.clone();
                if frame.ni < frame.neighbors.len() {
                    let w = frame.neighbors[frame.ni].clone();
                    frame.ni += 1;
                    (v, Some(w))
                } else {
                    (v, None)
                }
            }
        };
        match step {
            (v, Some(w)) => {
                if !s.indices.contains_key(&w) {
                    s.indices.insert(w.clone(), s.counter);
                    s.lowlinks.insert(w.clone(), s.counter);
                    s.counter += 1;
                    s.stack.push(w.clone());
                    s.on_stack.insert(w.clone());
                    work.push(Frame {
                        neighbors: uses.get(&w).cloned().unwrap_or_default(),
                        v: w,
                        ni: 0,
                    });
                } else if s.on_stack.contains(&w) {
                    let w_idx = s.indices[&w];
                    let v_ll = s.lowlinks[&v];
                    s.lowlinks.insert(v, v_ll.min(w_idx));
                }
            }
            (v, None) => {
                work.pop();
                if s.lowlinks[&v] == s.indices[&v] {
                    let mut scc = vec![];
                    loop {
                        let w = s.stack.pop().expect("Tarjan stack non-empty");
                        s.on_stack.remove(&w);
                        scc.push(w.clone());
                        if w == v {
                            break;
                        }
                    }
                    s.sccs.push(scc);
                }
                if let Some(parent) = work.last_mut() {
                    let v_ll = s.lowlinks[&v];
                    let p_ll = s.lowlinks[&parent.v];
                    s.lowlinks.insert(parent.v.clone(), p_ll.min(v_ll));
                }
            }
        }
    }
}

/// Resolve a dep name to a single NodeId: local file first, then unique workspace match.
fn resolve_def(
    name: &str,
    file_uri: &tower_lsp_server::ls_types::Uri,
    def_index: &HashMap<String, Vec<NodeId>>,
) -> Option<NodeId> {
    let candidates = def_index.get(name)?;

    // Local file takes priority.
    let local: Vec<_> = candidates.iter().filter(|id| &id.uri == file_uri).collect();
    if local.len() == 1 {
        return Some(local[0].clone());
    }

    // Fall back to unique workspace-wide match.
    if candidates.len() == 1 {
        return Some(candidates[0].clone());
    }

    // Ambiguous — no edge (P4).
    None
}

// ── Path trie (REQ-IDX-03) ───────────────────────────────────────────────────

/// Build a path trie from all resolved routes (REQ-IDX-03).
fn build_path_trie(route_index: &HashMap<RouteId, Vec<RouteRecord>>) -> PathTrie {
    let mut trie = PathTrie::default();
    for (id, records) in route_index {
        for record in records {
            if let ResolvedPath::Resolved(path) = &record.resolved_path {
                let segs = path_segments(path);
                trie_insert(&mut trie.root, &segs, id.clone());
            }
        }
    }
    trie
}

/// Insert a route at the path described by `segments`.
fn trie_insert(node: &mut TrieNode, segments: &[&str], id: RouteId) {
    if segments.is_empty() {
        node.routes.push(id);
        return;
    }
    let seg = segments[0];
    let rest = &segments[1..];

    if seg.starts_with('{') && seg.ends_with('}') {
        let inner = &seg[1..seg.len() - 1];
        if inner.ends_with(":path") {
            // {name:path} is a multi-segment wildcard; Starlette requires it to be
            // the last segment in a pattern, so rest is normally empty.
            let name = inner.strip_suffix(":path").unwrap_or(inner).to_owned();
            let (_, child) = node.path_param.get_or_insert_with(|| {
                (name, Box::new(TrieNode::default()))
            });
            trie_insert(child, rest, id);
        } else {
            // All single-segment wildcards ({id}, {id:int}, {id:uuid}, etc.) share
            // one param slot per trie level — the name is retained from first insert;
            // subsequent routes at the same level reuse the same child node (correct
            // for matching since param names are irrelevant for path matching).
            let name = inner.split(':').next().unwrap_or(inner).to_owned();
            let (_, child) = node.param.get_or_insert_with(|| {
                (name, Box::new(TrieNode::default()))
            });
            trie_insert(child, rest, id);
        }
    } else {
        let child = node.literal.entry(seg.to_owned()).or_default();
        trie_insert(child, rest, id);
    }
}

/// Concrete-path lookup: returns all route IDs whose resolved path matches
/// `segments`. `{param}` matches any single non-empty segment; `{p:path}`
/// matches any number of remaining segments (REQ-IDX-03).
fn trie_lookup(node: &TrieNode, segments: &[&str]) -> Vec<RouteId> {
    if segments.is_empty() {
        return node.routes.clone();
    }
    let seg = segments[0];
    let rest = &segments[1..];
    let mut matches = vec![];

    // Literal (including the trailing-slash empty-string segment)
    if let Some(child) = node.literal.get(seg) {
        matches.extend(trie_lookup(child, rest));
    }
    // Param only matches non-empty segments
    if !seg.is_empty() {
        if let Some((_, child)) = &node.param {
            matches.extend(trie_lookup(child, rest));
        }
        // path_param consumes this segment and all remaining
        if let Some((_, child)) = &node.path_param {
            matches.extend(child.routes.clone());
        }
    }

    matches
}

/// Split a URL path into trie segments. Strips query string, removes the
/// leading empty string from the leading `/`, but preserves the trailing
/// empty string that encodes a trailing slash (REQ-TLINK-02).
fn path_segments(path: &str) -> Vec<&str> {
    let path = path.split('?').next().unwrap_or(path);
    let mut segs: Vec<&str> = path.split('/').collect();
    if segs.first().copied() == Some("") {
        segs.remove(0);
    }
    segs
}

// ── Test refs index (REQ-TLINK-02) ───────────────────────────────────────────

/// Match all `ClientCall` facts against the path trie and build the
/// `test_refs` index: route → list of call sites.
fn build_test_refs(
    state: &WorkspaceState,
    trie: &PathTrie,
    route_index: &HashMap<RouteId, Vec<RouteRecord>>,
) -> HashMap<RouteId, Vec<ClientCallSite>> {
    let mut test_refs: HashMap<RouteId, Vec<ClientCallSite>> = HashMap::new();

    for entry in state.file_facts.iter() {
        let facts = entry.value();
        let file_uri = entry.key();

        for call in &facts.client_calls {
            let matched_ids = if call.is_prefix {
                match_prefix(route_index, &call.path, &call.method, call.path_depth)
            } else {
                match_call(trie, route_index, &call.path, &call.method)
            };
            if matched_ids.is_empty() {
                continue;
            }
            let site = ClientCallSite {
                method: call.method.clone(),
                path: call.path.clone(),
                location: Location {
                    uri: file_uri.clone(),
                    range: call.range,
                },
            };
            for id in matched_ids {
                test_refs.entry(id).or_default().push(site.clone());
            }
        }
    }

    test_refs
}

/// Look up matching route IDs for a client call path + method.
/// Tries exact path first, then a slash-insensitive retry (REQ-TLINK-02).
fn match_call(
    trie: &PathTrie,
    route_index: &HashMap<RouteId, Vec<RouteRecord>>,
    path: &str,
    method: &crate::state::Method,
) -> Vec<RouteId> {
    let segs = path_segments(path);
    let mut ids = filter_by_method(trie_lookup(&trie.root, &segs), route_index, method);

    if ids.is_empty() {
        // Slash-insensitive retry: toggle trailing slash.
        // Strip query string first so ?q=foo doesn't interfere with slash detection.
        let path_no_query = path.split('?').next().unwrap_or(path);
        let alt: String = if path_no_query.ends_with('/') {
            path_no_query.trim_end_matches('/').to_owned()
        } else {
            format!("{}/", path_no_query)
        };
        let alt_segs = path_segments(&alt);
        ids = filter_by_method(trie_lookup(&trie.root, &alt_segs), route_index, method);
    }

    ids
}

/// Find route IDs whose resolved path starts with `prefix` and match `method`.
/// When `path_depth` is provided, further filters to routes with the same segment count,
/// disambiguating cases like `/v1/projects/{}/documents` vs `/v1/projects/{}/installers/{}`.
fn match_prefix(
    route_index: &HashMap<RouteId, Vec<RouteRecord>>,
    prefix: &str,
    method: &crate::state::Method,
    path_depth: Option<usize>,
) -> Vec<RouteId> {
    if prefix.is_empty() {
        return vec![];
    }
    route_index
        .iter()
        .filter_map(|(id, records)| {
            records.first().and_then(|rec| {
                if &rec.method == method {
                    if let ResolvedPath::Resolved(ref path) = rec.resolved_path {
                        if path.starts_with(prefix) {
                            if let Some(expected_depth) = path_depth {
                                let route_depth = path.trim_end_matches('/').split('/').count();
                                if route_depth != expected_depth {
                                    return None;
                                }
                            }
                            return Some(id.clone());
                        }
                    }
                }
                None
            })
        })
        .collect()
}

fn filter_by_method(
    ids: Vec<RouteId>,
    route_index: &HashMap<RouteId, Vec<RouteRecord>>,
    method: &crate::state::Method,
) -> Vec<RouteId> {
    ids.into_iter()
        .filter(|id| {
            route_index
                .get(id)
                .and_then(|recs| recs.first())
                .map(|rec| &rec.method == method)
                .unwrap_or(false)
        })
        .collect()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_path_no_double_slash() {
        assert_eq!(join_path_segments(&["/api".into(), "/books".into(), "/{id}".into()]),
            "/api/books/{id}");
    }

    #[test]
    fn join_path_empty_segments_skipped() {
        assert_eq!(join_path_segments(&["".into(), "/books".into(), "/".into()]),
            "/books/");
    }

    #[test]
    fn join_path_trailing_slash_preserved() {
        // Trailing slash is significant (REQ-ROUTE-04)
        assert_eq!(join_path_segments(&["/api".into(), "/books".into(), "/".into()]),
            "/api/books/");
    }

    #[test]
    fn join_path_collapse_double_slash() {
        // "/api/" + "/books" → "/api/books"
        assert_eq!(join_path_segments(&["/api/".into(), "/books".into()]),
            "/api/books");
    }

    fn make_uri(s: &str) -> Uri {
        s.parse().unwrap()
    }

    fn make_ctx(
        routers: &[(&str, &str)],       // (name, prefix)
        apps: &[&str],
        includes: &[(&str, &str, &str)], // (target, prefix, app_name)
    ) -> LinkContext {
        let mut ctx = LinkContext {
            routers: HashMap::new(),
            apps: std::collections::HashSet::new(),
            includes_by_target: HashMap::new(),
            middlewares_by_obj: HashMap::new(),
        };
        for &(name, prefix) in routers {
            ctx.routers.entry(name.to_owned()).or_default().push((
                RouterDecl {
                    name: name.to_owned(),
                    prefix: PrefixValue::Literal(prefix.to_owned()),
                    tags: vec![],
                    range: Default::default(),
                },
                make_uri("file:///app/router.py"),
            ));
        }
        for &app in apps {
            ctx.apps.insert(app.to_owned());
        }
        for &(target, prefix, app_name) in includes {
            ctx.includes_by_target
                .entry(target.to_owned())
                .or_default()
                .push((
                    IncludeCall {
                        target: target.to_owned(),
                        prefix: PrefixValue::Literal(prefix.to_owned()),
                        app_name: app_name.to_owned(),
                        dependencies: vec![],
                        range: Default::default(),
                    },
                    make_uri("file:///app/main.py"),
                ));
        }
        ctx
    }

    #[test]
    fn single_mount_resolves() {
        let ctx = make_ctx(
            &[("router", "/books")],
            &["app"],
            &[("router", "/v1", "app")],
        );
        let paths = ctx.resolve_route_paths(
            &make_uri("file:///app/router.py"),
            "router",
            &PrefixValue::Literal("/{id}".into()),
        );
        assert_eq!(paths.len(), 1);
        assert!(matches!(&paths[0], ResolvedPath::Resolved(p) if p == "/v1/books/{id}"));
    }

    #[test]
    fn multiple_mounts_produce_multiple_records() {
        // Same router included at /v1 and /v2
        let ctx = make_ctx(
            &[("router", "/books")],
            &["app"],
            &[
                ("router", "/v1", "app"),
                ("router", "/v2", "app"),
            ],
        );
        let paths = ctx.resolve_route_paths(
            &make_uri("file:///app/router.py"),
            "router",
            &PrefixValue::Literal("/{id}".into()),
        );
        assert_eq!(paths.len(), 2);
        let resolved: Vec<_> = paths.iter()
            .filter_map(|p| if let ResolvedPath::Resolved(s) = p { Some(s.as_str()) } else { None })
            .collect();
        assert!(resolved.contains(&"/v1/books/{id}"));
        assert!(resolved.contains(&"/v2/books/{id}"));
    }

    #[test]
    fn multi_mount_paths_contain_both_prefixes() {
        // Verifies that both mount points are present; no path is silently dropped.
        // This is a determinism guard: previously only includes[0] was followed.
        let ctx = make_ctx(
            &[("router", "")], // no router-level prefix
            &["app"],
            &[("router", "/v1", "app"), ("router", "/v2", "app")],
        );
        let paths = ctx.resolve_route_paths(
            &make_uri("file:///app/router.py"),
            "router",
            &PrefixValue::Literal("/items".into()),
        );
        assert_eq!(paths.len(), 2, "both mount points must produce a record");
        let strs: Vec<&str> = paths.iter()
            .filter_map(|p| if let ResolvedPath::Resolved(s) = p { Some(s.as_str()) } else { None })
            .collect();
        assert!(strs.contains(&"/v1/items"), "/v1 mount should resolve");
        assert!(strs.contains(&"/v2/items"), "/v2 mount should resolve");
    }

    #[test]
    fn trailing_slash_preserved() {
        let ctx = make_ctx(
            &[("router", "/books/")],
            &["app"],
            &[("router", "/api", "app")],
        );
        let paths = ctx.resolve_route_paths(
            &make_uri("file:///app/router.py"),
            "router",
            &PrefixValue::Literal("/".into()),
        );
        assert_eq!(paths.len(), 1);
        assert!(matches!(&paths[0], ResolvedPath::Resolved(p) if p == "/api/books/"));
    }

    // ── dep_graph helpers ─────────────────────────────────────────────────────

    fn make_def_index(defs: &[(&str, &str)]) -> HashMap<String, Vec<NodeId>> {
        let mut index: HashMap<String, Vec<NodeId>> = HashMap::new();
        for &(name, uri_str) in defs {
            let uri: Uri = uri_str.parse().unwrap();
            let node_id = NodeId { uri, range: Default::default() };
            index.entry(name.to_owned()).or_default().push(node_id);
        }
        index
    }

    // ── SCC / cycle tests ─────────────────────────────────────────────────────

    fn make_node(uri_str: &str) -> NodeId {
        NodeId { uri: uri_str.parse().unwrap(), range: Default::default() }
    }

    fn make_uses(pairs: &[(&str, &str)]) -> HashMap<NodeId, Vec<NodeId>> {
        let mut map: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for &(from, to) in pairs {
            map.entry(make_node(from)).or_default().push(make_node(to));
        }
        map
    }

    #[test]
    fn scc_no_cycle() {
        // A → B, no cycle
        let uses = make_uses(&[("file:///a.py", "file:///b.py")]);
        let sccs = tarjan_sccs(&uses);
        assert!(sccs.is_empty(), "no cycle expected");
    }

    #[test]
    fn scc_self_loop() {
        let uses = make_uses(&[("file:///a.py", "file:///a.py")]);
        let sccs = tarjan_sccs(&uses);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 1);
    }

    #[test]
    fn scc_two_node_cycle() {
        // A → B → A
        let uses = make_uses(&[
            ("file:///a.py", "file:///b.py"),
            ("file:///b.py", "file:///a.py"),
        ]);
        let sccs = tarjan_sccs(&uses);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 2);
        let sccs_flat: std::collections::HashSet<NodeId> = sccs.into_iter().flatten().collect();
        assert!(sccs_flat.contains(&make_node("file:///a.py")));
        assert!(sccs_flat.contains(&make_node("file:///b.py")));
    }

    #[test]
    fn scc_three_node_cycle() {
        // A → B → C → A
        let uses = make_uses(&[
            ("file:///a.py", "file:///b.py"),
            ("file:///b.py", "file:///c.py"),
            ("file:///c.py", "file:///a.py"),
        ]);
        let sccs = tarjan_sccs(&uses);
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 3);
    }

    #[test]
    fn scc_deep_linear_chain_no_stack_overflow() {
        // Build a chain of 10_000 nodes: n0 → n1 → … → n9999.
        // Recursive Tarjan would overflow; iterative should complete without panic.
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(9_999);
        for i in 0..9_999usize {
            pairs.push((format!("file:///n{i}.py"), format!("file:///n{}.py", i + 1)));
        }
        let pair_refs: Vec<(&str, &str)> = pairs.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let uses = make_uses(&pair_refs);
        let sccs = tarjan_sccs(&uses);
        assert!(sccs.is_empty(), "linear chain has no cycles");
    }

    #[test]
    fn scc_cycle_map_covers_all_members() {
        let uses = make_uses(&[
            ("file:///a.py", "file:///b.py"),
            ("file:///b.py", "file:///a.py"),
        ]);
        let dep_graph = DepGraph { uses, ..Default::default() };
        let cycle_map = build_cycle_map(&dep_graph);
        assert!(cycle_map.contains_key(&make_node("file:///a.py")));
        assert!(cycle_map.contains_key(&make_node("file:///b.py")));
    }

    #[test]
    fn override_sites_populated_from_file_facts() {
        // Verify build_dep_graph maps override sites to the correct NodeId
        let uri_app: Uri = "file:///app.py".parse().unwrap();
        let uri_test: Uri = "file:///tests/conftest.py".parse().unwrap();
        use crate::state::{DepDef, FileFacts, OverrideSite};
        use tower_lsp_server::ls_types::Position;

        let def_range = tower_lsp_server::ls_types::Range {
            start: Position::new(1, 4),
            end: Position::new(1, 10),
        };
        let override_range = tower_lsp_server::ls_types::Range {
            start: Position::new(5, 20),
            end: Position::new(5, 26),
        };

        let mut facts_app = FileFacts::new(uri_app.clone());
        facts_app.dep_defs.push(DepDef {
            name: "get_db".to_owned(),
            node_id: NodeId { uri: uri_app.clone(), range: def_range },
            has_yield: true,
            param_names: vec![],
        });

        let mut facts_test = FileFacts::new(uri_test.clone());
        facts_test.override_sites.push(OverrideSite {
            name: "get_db".to_owned(),
            range: override_range,
        });

        // Use WorkspaceState directly
        let state = crate::state::WorkspaceState::new(
            crate::config::ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_app.clone(), facts_app);
        state.file_facts.insert(uri_test.clone(), facts_test);

        let dep_graph = build_dep_graph(&state);
        let def_node = NodeId { uri: uri_app.clone(), range: def_range };
        assert!(dep_graph.override_sites.contains_key(&def_node));
        let sites = &dep_graph.override_sites[&def_node];
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].uri, uri_test);
        assert_eq!(sites[0].range, override_range);
    }

    #[test]
    fn resolve_def_local_first() {
        let uri_a: Uri = "file:///a.py".parse().unwrap();
        let index = make_def_index(&[("get_db", "file:///a.py"), ("get_db", "file:///b.py")]);
        let result = resolve_def("get_db", &uri_a, &index).unwrap();
        assert_eq!(result.uri, uri_a);
    }

    #[test]
    fn resolve_def_unique_workspace_fallback() {
        let uri_a: Uri = "file:///a.py".parse().unwrap();
        let index = make_def_index(&[("get_db", "file:///b.py")]);
        let result = resolve_def("get_db", &uri_a, &index).unwrap();
        assert_eq!(result.uri, "file:///b.py".parse::<Uri>().unwrap());
    }

    #[test]
    fn resolve_def_ambiguous_returns_none() {
        let uri_a: Uri = "file:///a.py".parse().unwrap();
        let index = make_def_index(&[("get_db", "file:///b.py"), ("get_db", "file:///c.py")]);
        assert!(resolve_def("get_db", &uri_a, &index).is_none());
    }

    #[test]
    fn resolve_def_not_found_returns_none() {
        let uri_a: Uri = "file:///a.py".parse().unwrap();
        let index: HashMap<String, Vec<NodeId>> = HashMap::new();
        assert!(resolve_def("missing", &uri_a, &index).is_none());
    }

    fn make_ctx_with_mw(
        routers: &[(&str, &str)],
        apps: &[&str],
        includes: &[(&str, &str, &str)],
        mw: &[(&str, &str)], // (app_name, class_name)
    ) -> LinkContext {
        let mut ctx = make_ctx(routers, apps, includes);
        for &(app_name, class_name) in mw {
            ctx.middlewares_by_obj
                .entry(app_name.to_owned())
                .or_default()
                .push(MiddlewareCall {
                    app_name: app_name.to_owned(),
                    source: MwSource::Class(class_name.to_owned()),
                    range: Default::default(),
                    kwargs_start: None,
                    present_kwargs: vec![],
                });
        }
        ctx
    }

    #[test]
    fn middleware_reversed_within_level() {
        // add_middleware(A), add_middleware(B) → B runs first (prepend semantics)
        let ctx = make_ctx_with_mw(
            &[],
            &["app"],
            &[],
            &[("app", "A"), ("app", "B")],
        );
        let chain = ctx.middleware_chain("app");
        assert_eq!(chain, vec!["B", "A"]);
    }

    #[test]
    fn app_middleware_outermost_first() {
        // app.add_middleware(AppMw), router.add_middleware(RouterMw)
        let ctx = make_ctx_with_mw(
            &[("router", "/books")],
            &["app"],
            &[("router", "", "app")],
            &[("app", "AppMw"), ("router", "RouterMw")],
        );
        let chain = ctx.middleware_chain("router");
        assert_eq!(chain, vec!["AppMw", "RouterMw"]);
    }

    #[test]
    fn middleware_chain_visits_all_include_sites() {
        // Router mounted in both app1 (M1) and app2 (M2) — both should appear
        let ctx = make_ctx_with_mw(
            &[("router", "/books")],
            &["app1", "app2"],
            &[("router", "", "app1"), ("router", "", "app2")],
            &[("app1", "M1"), ("app2", "M2")],
        );
        let chain = ctx.middleware_chain("router");
        assert!(chain.contains(&"M1".to_owned()), "M1 should appear");
        assert!(chain.contains(&"M2".to_owned()), "M2 should appear");
    }

    #[test]
    fn middleware_chain_no_infinite_loop_on_cycle() {
        // Even a degenerate include cycle should not loop forever
        let mut ctx = make_ctx(
            &[("r", "/x")],
            &["app"],
            &[("r", "", "app")],
        );
        // Add a cycle: app also has an include pointing back at r (shouldn't happen in real code
        // but guard is needed; seen set prevents revisits)
        ctx.includes_by_target.entry("app".to_owned()).or_default().push((
            crate::state::IncludeCall {
                app_name: "app".to_owned(),
                target: "r".to_owned(),
                prefix: crate::state::PrefixValue::Literal("".to_owned()),
                dependencies: vec![],
                range: Default::default(),
            },
            "file:///app.py".parse().unwrap(),
        ));
        let chain = ctx.middleware_chain("r"); // must terminate
        drop(chain);
    }

    #[test]
    fn unincluded_router_is_unresolved() {
        let ctx = make_ctx(
            &[("router", "/books")],
            &["app"],
            &[], // no includes
        );
        let paths = ctx.resolve_route_paths(
            &make_uri("file:///app/router.py"),
            "router",
            &PrefixValue::Literal("/{id}".into()),
        );
        assert_eq!(paths.len(), 1);
        assert!(matches!(paths[0], ResolvedPath::Unresolved));
    }

    // ── Trie tests ────────────────────────────────────────────────────────────

    fn rid(s: &str) -> RouteId {
        RouteId(s.to_owned())
    }

    #[test]
    fn trie_literal_path_match() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["api", "users"], rid("get_users"));
        assert_eq!(trie_lookup(&root, &["api", "users"]), vec![rid("get_users")]);
    }

    #[test]
    fn trie_literal_no_partial_match() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["api", "users"], rid("get_users"));
        assert!(trie_lookup(&root, &["api"]).is_empty());
    }

    #[test]
    fn trie_param_matches_any_segment() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["users", "{user_id}"], rid("get_user"));
        assert_eq!(trie_lookup(&root, &["users", "42"]), vec![rid("get_user")]);
        assert_eq!(trie_lookup(&root, &["users", "abc"]), vec![rid("get_user")]);
    }

    #[test]
    fn trie_param_does_not_match_empty_segment() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["users", "{user_id}"], rid("get_user"));
        assert!(trie_lookup(&root, &["users", ""]).is_empty());
    }

    #[test]
    fn trie_path_param_matches_single_remaining_segment() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["files", "{path:path}"], rid("get_file"));
        assert_eq!(trie_lookup(&root, &["files", "readme.txt"]), vec![rid("get_file")]);
    }

    #[test]
    fn trie_path_param_matches_multiple_remaining_segments() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["files", "{path:path}"], rid("get_file"));
        assert_eq!(trie_lookup(&root, &["files", "a", "b", "c"]), vec![rid("get_file")]);
    }

    #[test]
    fn trie_trailing_slash_distinct() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["items"], rid("no_slash"));
        trie_insert(&mut root, &["items", ""], rid("with_slash"));
        assert_eq!(trie_lookup(&root, &["items"]), vec![rid("no_slash")]);
        assert_eq!(trie_lookup(&root, &["items", ""]), vec![rid("with_slash")]);
    }

    #[test]
    fn path_segments_strips_query_string() {
        assert_eq!(path_segments("/api/users?page=1"), vec!["api", "users"]);
    }

    #[test]
    fn path_segments_preserves_trailing_slash() {
        assert_eq!(path_segments("/api/users/"), vec!["api", "users", ""]);
    }

    #[test]
    fn path_segments_no_trailing_slash() {
        assert_eq!(path_segments("/api/users"), vec!["api", "users"]);
    }

    #[test]
    fn match_call_slash_insensitive_retry() {
        // Route: /items (no trailing slash) — client calls /items/ (with slash)
        let id = rid("get_items:GET");
        let mut route_index = HashMap::new();
        route_index.insert(id.clone(), vec![RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "get_items".into(),
            method: crate::state::Method::Get,
            resolved_path: ResolvedPath::Resolved("/items".into()),
            decorator_path: "/items".into(),
            chain: vec![],
            handler: Location { uri: "file:///a.py".parse().unwrap(), range: Default::default() },
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
        let trie = build_path_trie(&route_index);

        // Exact: /items/ does NOT match /items → retry fires and finds /items
        let matched = match_call(&trie, &route_index, "/items/", &crate::state::Method::Get);
        assert_eq!(matched, vec![id]);
    }

    #[test]
    fn trie_int_converter_goes_to_param_branch() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["users", "{id:int}"], rid("get_user"));
        assert_eq!(trie_lookup(&root, &["users", "42"]), vec![rid("get_user")]);
        assert!(trie_lookup(&root, &["users", ""]).is_empty());
    }

    #[test]
    fn trie_uuid_converter_goes_to_param_branch() {
        let mut root = TrieNode::default();
        trie_insert(&mut root, &["items", "{item_id:uuid}"], rid("get_item"));
        assert_eq!(trie_lookup(&root, &["items", "abc-123"]), vec![rid("get_item")]);
    }

    #[test]
    fn path_segments_root_path() {
        // "/" → one empty segment
        assert_eq!(path_segments("/"), vec![""]);
        // and empty string
        assert_eq!(path_segments(""), Vec::<&str>::new());
    }

    #[test]
    fn match_call_slash_retry_with_query_string() {
        // Route: /items (no slash) — client calls /items/?page=1
        let id = rid("get_items:GET");
        let mut route_index = HashMap::new();
        route_index.insert(id.clone(), vec![RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "get_items".into(),
            method: crate::state::Method::Get,
            resolved_path: ResolvedPath::Resolved("/items".into()),
            decorator_path: "/items".into(),
            chain: vec![],
            handler: Location {
                uri: "file:///a.py".parse().unwrap(),
                range: Default::default(),
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
        }]);
        let trie = build_path_trie(&route_index);
        // Slash-insensitive retry must find /items despite the trailing /? in input
        let matched = match_call(&trie, &route_index, "/items/?page=1", &crate::state::Method::Get);
        assert_eq!(matched, vec![id]);
    }

    #[test]
    fn match_call_method_filter() {
        // Route GET /users — POST call must not match
        let id = rid("list_users:GET");
        let mut route_index = HashMap::new();
        route_index.insert(id.clone(), vec![RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "list_users".into(),
            method: crate::state::Method::Get,
            resolved_path: ResolvedPath::Resolved("/users".into()),
            decorator_path: "/users".into(),
            chain: vec![],
            handler: Location { uri: "file:///a.py".parse().unwrap(), range: Default::default() },
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
        let trie = build_path_trie(&route_index);

        assert!(match_call(&trie, &route_index, "/users", &crate::state::Method::Post).is_empty());
        assert_eq!(match_call(&trie, &route_index, "/users", &crate::state::Method::Get), vec![id]);
    }

    // ── route_names gate for MOUNT routes ─────────────────────────────────────

    fn make_mount_fact(app_name: &str, path: &str, route_name: Option<&str>) -> crate::state::RouteFact {
        crate::state::RouteFact {
            handler_name: route_name.unwrap_or("StaticFiles").to_owned(),
            handler_range: tower_lsp_server::ls_types::Range::default(),
            object_name: app_name.to_owned(),
            methods: vec![Method::Mount],
            path: PrefixValue::Literal(path.to_owned()),
            path_range: None,
            path_quote_width: None,
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            status_code: None,
            dependencies: vec![],
            route_name: route_name.map(|s| s.to_owned()),
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: false,
        }
    }

    #[test]
    fn named_mount_appears_in_route_names() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let state = crate::state::WorkspaceState::new(
            crate::config::ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        let mut facts = crate::state::FileFacts::new(uri.clone());
        facts.apps.push(crate::state::AppDecl { name: "app".to_owned(), range: tower_lsp_server::ls_types::Range::default() });
        facts.routes.push(make_mount_fact("app", "/static", Some("static")));
        state.file_facts.insert(uri, facts);

        let linked = build(&state, 1, &[], &[]);
        assert!(linked.route_names.contains_key("static"),
            "named mount should appear in route_names");
    }

    #[test]
    fn unnamed_mount_not_in_route_names() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let state = crate::state::WorkspaceState::new(
            crate::config::ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        let mut facts = crate::state::FileFacts::new(uri.clone());
        facts.apps.push(crate::state::AppDecl { name: "app".to_owned(), range: tower_lsp_server::ls_types::Range::default() });
        facts.routes.push(make_mount_fact("app", "/static", None));
        state.file_facts.insert(uri, facts);

        let linked = build(&state, 1, &[], &[]);
        assert!(!linked.route_names.contains_key("StaticFiles"),
            "unnamed mount handler_name should NOT appear in route_names");
    }

    // ── Template index tests ──────────────────────────────────────────────────

    fn tmp_tpl_dir() -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fastapi-lsp-tpl-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn template_index_scans_files_in_root() {
        let root = tmp_tpl_dir();
        std::fs::write(root.join("index.html"), "").unwrap();
        std::fs::create_dir(root.join("emails")).unwrap();
        std::fs::write(root.join("emails").join("welcome.html"), "").unwrap();

        let index = build_template_index(&[root.clone()]);
        let _ = std::fs::remove_dir_all(&root);

        assert!(index.contains_key("index.html"));
        assert!(index.contains_key("emails/welcome.html"));
    }

    #[test]
    fn template_index_first_root_wins_on_collision() {
        let root1 = tmp_tpl_dir();
        let root2 = tmp_tpl_dir();
        std::fs::write(root1.join("base.html"), "root1").unwrap();
        std::fs::write(root2.join("base.html"), "root2").unwrap();

        let index = build_template_index(&[root1.clone(), root2.clone()]);
        let _ = std::fs::remove_dir_all(&root1);
        let _ = std::fs::remove_dir_all(&root2);

        let uri = index.get("base.html").expect("base.html must be in index");
        assert!(uri.path().as_str().contains(root1.file_name().unwrap().to_str().unwrap()),
            "higher-precedence root1 must win: got {uri:?}");
    }

    #[test]
    fn template_index_nonexistent_root_skipped() {
        let missing = std::path::PathBuf::from("/tmp/fastapi-lsp-missing-tpl-dir-xyz");
        let index = build_template_index(&[missing]);
        assert!(index.is_empty());
    }

    #[test]
    fn template_index_empty_when_no_roots() {
        let index = build_template_index(&[]);
        assert!(index.is_empty());
    }

    #[test]
    fn router_name_collision_prefers_same_file() {
        // Two files each declare `router = APIRouter(prefix=...)`.
        // Routes from file_a should resolve using file_a's router prefix,
        // not file_b's prefix (which a last-write-wins map would give).
        let file_a: Uri = "file:///app/a.py".parse().unwrap();
        let file_b: Uri = "file:///app/b.py".parse().unwrap();
        let main_uri: Uri = "file:///app/main.py".parse().unwrap();

        let mut ctx = LinkContext {
            routers: HashMap::new(),
            apps: std::collections::HashSet::new(),
            includes_by_target: HashMap::new(),
            middlewares_by_obj: HashMap::new(),
        };
        // Both files declare a variable named "router" with different prefixes.
        ctx.routers.entry("router".to_owned()).or_default().push((
            RouterDecl { name: "router".to_owned(), prefix: PrefixValue::Literal("/a".to_owned()), tags: vec![], range: Default::default() },
            file_a.clone(),
        ));
        ctx.routers.entry("router".to_owned()).or_default().push((
            RouterDecl { name: "router".to_owned(), prefix: PrefixValue::Literal("/b".to_owned()), tags: vec![], range: Default::default() },
            file_b.clone(),
        ));
        ctx.apps.insert("app".to_owned());
        // Both routers are included into the same app.
        ctx.includes_by_target.entry("router".to_owned()).or_default().push((
            IncludeCall { app_name: "app".to_owned(), target: "router".to_owned(), prefix: PrefixValue::Literal(String::new()), dependencies: vec![], range: Default::default() },
            main_uri.clone(),
        ));

        // Route in file_a on "router" should resolve using file_a's prefix "/a".
        let resolved = ctx.resolve_route_paths(&file_a, "router", &PrefixValue::Literal("/items".to_owned()));
        assert_eq!(resolved, vec![ResolvedPath::Resolved("/a/items".to_owned())]);

        // Route in file_b on "router" should resolve using file_b's prefix "/b".
        let resolved = ctx.resolve_route_paths(&file_b, "router", &PrefixValue::Literal("/items".to_owned()));
        assert_eq!(resolved, vec![ResolvedPath::Resolved("/b/items".to_owned())]);
    }
}
