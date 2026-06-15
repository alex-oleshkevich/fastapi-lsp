use std::collections::HashSet;

use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag, Location,
    NumberOrString, Range, Uri,
};

use crate::state::{NodeId, ResolvedPath, RouteRecord, WorkspaceState};

/// Built-in env keys that are never flagged as undefined (REQ-ENV-06 allowlist).
static OS_CI_ALLOWLIST: &[&str] = &[
    // POSIX / common OS
    "HOME",
    "PATH",
    "USER",
    "PORT",
    "HOSTNAME",
    "PWD",
    "SHELL",
    "TERM",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "TZ",
    // GitHub Actions
    "CI",
    "GITHUB_ACTIONS",
    "GITHUB_TOKEN",
    "GITHUB_SHA",
    "GITHUB_REF",
    "GITHUB_RUN_ID",
    "GITHUB_RUN_NUMBER",
    "GITHUB_WORKSPACE",
    "GITHUB_ACTOR",
    "GITHUB_REPOSITORY",
    "GITHUB_EVENT_NAME",
    // GitLab CI
    "GITLAB_CI",
    "CI_JOB_TOKEN",
    "CI_COMMIT_SHA",
    "CI_PROJECT_ID",
    // CircleCI
    "CIRCLECI",
    "CIRCLE_SHA1",
    "CIRCLE_BRANCH",
    // Travis
    "TRAVIS",
    "TRAVIS_COMMIT",
    "TRAVIS_BRANCH",
    // Docker / container
    "DOCKER_HOST",
    // Python
    "PYTHONPATH",
    "DJANGO_SETTINGS_MODULE",
];

/// Returns true when `key` should be suppressed (OS/CI built-in or user-ignored).
pub fn is_env_key_ignored(key: &str, user_ignore: &[String]) -> bool {
    let key_upper = key.to_uppercase();
    if OS_CI_ALLOWLIST.contains(&key_upper.as_str()) {
        return true;
    }
    user_ignore.iter().any(|ig| ig.to_uppercase() == key_upper)
}

pub fn compute(state: &WorkspaceState, uri: &Uri, env_ignore: &[String]) -> Vec<Diagnostic> {
    let facts = match state.file_facts.get(uri) {
        Some(f) => f,
        None => return vec![],
    };
    let linked = state.linked.load();

    let mut diags = vec![];

    for site in &facts.env_lookups {
        if site.has_default {
            continue;
        }
        if is_env_key_ignored(&site.key, env_ignore) {
            continue;
        }
        let key_upper = site.key.to_uppercase();
        if linked.env_index.contains_key(&key_upper) || linked.env_index.contains_key(&site.key) {
            continue;
        }
        diags.push(undefined_key_diag(&site.key, site.key_range));
    }

    for cls in &facts.settings_classes {
        for field in &cls.fields {
            if field.has_default {
                continue;
            }
            let Some(key) = &field.env_key else { continue };
            if is_env_key_ignored(key, env_ignore) {
                continue;
            }
            if linked.env_file_keys.contains(key) {
                continue;
            }
            diags.push(settings_missing_env_diag(key, field.range));
        }
    }

    for dep_ref in &facts.dep_refs {
        if dep_ref.is_called
            && !dep_ref.name.is_empty()
            && linked.proven_dep_names.contains(dep_ref.name.as_str())
        {
            diags.push(depends_called_diag(dep_ref.range));
        }
    }

    // di/cycle: for each dep_def in this file that is a cycle member, find the
    // dep_ref inside that function that continues the cycle.
    for def in &facts.dep_defs {
        if let Some(cycle) = linked.dep_cycle_map.get(&def.node_id) {
            diags.extend(di_cycle_diags_for_member(
                &def.name,
                &facts.dep_refs,
                cycle,
                uri,
                state,
            ));
        }
    }

    // di/override-unused: an override site whose name resolves to nothing in the
    // dep graph — the override can never take effect (REQ-DI-05).
    let dep_names: HashSet<String> = {
        let mut s = HashSet::new();
        for fe in state.file_facts.iter() {
            for d in &fe.dep_defs {
                s.insert(d.name.clone());
            }
        }
        s
    };
    for site in &facts.override_sites {
        if !site.name.is_empty() && !dep_names.contains(&site.name) {
            diags.push(override_unused_diag(&site.name, site.range));
        }
    }

    // Cross-route checks: duplicate, shadowed, duplicate-name (REQ-DIAG-05/07/08)
    // Also used as the gate for url/unknown-name and route/router-not-included (REQ-DIAG-06/08).
    let has_unresolved_routes = linked.route_index.values().any(|records| {
        records
            .iter()
            .any(|r| matches!(r.resolved_path, ResolvedPath::Unresolved))
    });
    diags.extend(cross_route_diags(
        state,
        uri,
        &linked,
        has_unresolved_routes,
    ));

    // route/param-missing-arg + route/arg-missing-param (REQ-DIAG-03/04)
    diags.extend(route_param_checks(state, uri, &linked));

    // model/unknown-response-model (REQ-DIAG: catalog §3.2)
    diags.extend(model_unknown_response_model_checks(
        state, uri, &linked, &facts,
    ));

    // tpl/missing-template (REQ-TPL-05): gated on index being non-empty (P4).
    // An empty index means either no roots are configured or the initial scan hasn't finished —
    // in both cases we can't prove absence, so we stay silent.
    if !linked.template_index.is_empty() {
        let index_keys: Vec<&str> = linked.template_index.keys().map(|k| k.as_str()).collect();
        for tpl in &facts.templates {
            if linked.template_index.contains_key(&tpl.path) {
                continue;
            }
            let suggestion = index_keys
                .iter()
                .copied()
                .filter(|k| edit_distance(k, &tpl.path) <= 2)
                .min_by_key(|k| edit_distance(k, &tpl.path));
            diags.push(missing_template_diag(&tpl.path, suggestion, tpl.range));
        }
    }

    // url/unknown-name + url/param-mismatch (REQ-DIAG-06)
    for site in &facts.url_for_sites {
        if site.name.is_empty() {
            continue;
        }
        match linked.route_names.get(&site.name) {
            None => {
                if !has_unresolved_routes {
                    diags.push(url_unknown_name_diag(&site.name, site.range));
                }
            }
            Some(route_ids) => {
                // url/param-mismatch: skip when **splat kwargs are present (REQ-DIAG-06)
                if site.has_splat_kwargs {
                    continue;
                }
                // Compare against the first fully-resolved non-mount route record for this name.
                // Mount routes are excluded — their param signature is not statically known (P4).
                let record = route_ids.iter().find_map(|id| {
                    linked.route_index.get(id)?.iter().find(|r| {
                        matches!(r.resolved_path, ResolvedPath::Resolved(_))
                            && r.method != crate::state::Method::Mount
                    })
                });
                if let Some(record) = record {
                    let expected: HashSet<&str> =
                        record.path_params.iter().map(|p| p.name.as_str()).collect();
                    let provided: HashSet<&str> =
                        site.kwarg_names.iter().map(|k| k.as_str()).collect();
                    let missing: Vec<&str> = expected.difference(&provided).copied().collect();
                    let extra: Vec<&str> = provided.difference(&expected).copied().collect();
                    if !missing.is_empty() || !extra.is_empty() {
                        diags.push(url_param_mismatch_diag(
                            &site.name, &missing, &extra, site.range,
                        ));
                    }
                }
            }
        }
    }

    // oauth2/unknown-token-url (REQ-DIAG-21): tokenUrl/authorizationUrl references a missing route.
    if !has_unresolved_routes {
        for site in &facts.security_scheme_sites {
            let matched = linked
                .route_index
                .values()
                .flat_map(|records| records.iter())
                .any(|r| matches!(&r.resolved_path, ResolvedPath::Resolved(p) if p == &site.path));
            if !matched {
                diags.push(unknown_token_url_diag(&site.path, site.range));
            }
        }
    }

    // test/unknown-path (REQ-TLINK-04): opt-in, disabled by default.
    // Only fires when routes are fully resolved (same gate as url/unknown-name).
    let test_unknown_paths = state
        .config
        .try_read()
        .map(|c| c.features.test_unknown_paths)
        .unwrap_or(false);
    if test_unknown_paths && !has_unresolved_routes {
        for call in &facts.client_calls {
            let matched = linked
                .call_site_index
                .get(&(uri.clone(), call.range))
                .map(|v| !v.is_empty())
                .unwrap_or(false);
            if !matched {
                diags.push(test_unknown_path_diag(&call.path, call.range));
            }
        }
    }

    diags
}

/// Build `di/cycle` diagnostics for a single cycle member (`member_func`) in `uri`.
fn di_cycle_diags_for_member(
    member_func: &str,
    dep_refs: &[crate::state::DepRef],
    cycle: &[NodeId],
    uri: &Uri,
    state: &WorkspaceState,
) -> Vec<Diagnostic> {
    let mut diags = vec![];

    // Build the display path: "get_a → get_b → get_a"
    let cycle_names: Vec<String> = cycle_node_names(cycle, state);
    if cycle_names.is_empty() {
        return diags;
    }
    let mut path_parts = cycle_names.clone();
    // Close the loop by appending the first name again
    if let Some(first) = cycle_names.first() {
        path_parts.push(first.clone());
    }
    let cycle_path = path_parts.join(" → ");
    let message = format!("Dependency cycle: {cycle_path}.");

    // Find the dep_refs inside this function whose target is the next member in the cycle.
    let next_in_cycle: HashSet<String> = cycle
        .iter()
        .filter_map(|id| dep_node_name(id, state))
        .collect();

    for dep_ref in dep_refs {
        if dep_ref.containing_func.as_deref() != Some(member_func) {
            continue;
        }
        if dep_ref.name.is_empty() || !next_in_cycle.contains(&dep_ref.name) {
            continue;
        }
        // Build relatedInformation: locations of other cycle members' def sites
        let related = cycle
            .iter()
            .filter(|id| dep_node_name(id, state).as_deref() != Some(member_func))
            .filter_map(|id| {
                let name = dep_node_name(id, state)?;
                let msg = format!("also in cycle: {name}");
                Some(DiagnosticRelatedInformation {
                    location: Location {
                        uri: id.uri.clone(),
                        range: id.range,
                    },
                    message: msg,
                })
            })
            .collect::<Vec<_>>();
        diags.push(di_cycle_diag(dep_ref.range, &message, related, uri.clone()));
    }

    diags
}

/// Look up the display name for a dep NodeId by searching dep_defs across all files.
fn dep_node_name(id: &NodeId, state: &WorkspaceState) -> Option<String> {
    let facts = state.file_facts.get(&id.uri)?;
    facts
        .dep_defs
        .iter()
        .find(|d| d.node_id == *id)
        .map(|d| d.name.clone())
}

fn cycle_node_names(cycle: &[NodeId], state: &WorkspaceState) -> Vec<String> {
    cycle
        .iter()
        .filter_map(|id| {
            let facts = state.file_facts.get(&id.uri)?;

            facts
                .dep_defs
                .iter()
                .find(|d| d.node_id == *id)
                .map(|d| d.name.clone())
        })
        .collect()
}

pub fn di_cycle_diag(
    range: Range,
    message: &str,
    related: Vec<DiagnosticRelatedInformation>,
    _uri: Uri,
) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String("di/cycle".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: message.to_owned(),
        related_information: if related.is_empty() {
            None
        } else {
            Some(related)
        },
        ..Default::default()
    }
}

pub fn url_unknown_name_diag(name: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("url/unknown-name".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Unknown route name: {name}."),
        ..Default::default()
    }
}

pub fn url_param_mismatch_diag(
    name: &str,
    missing: &[&str],
    extra: &[&str],
    range: Range,
) -> Diagnostic {
    let mut parts = vec![];
    if !missing.is_empty() {
        parts.push(format!("missing: {}", missing.join(", ")));
    }
    if !extra.is_empty() {
        parts.push(format!("extra: {}", extra.join(", ")));
    }
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("url/param-mismatch".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Wrong url_for arguments for {name}: {}.", parts.join("; ")),
        ..Default::default()
    }
}

// ── Cross-route checks ────────────────────────────────────────────────────────

/// Emit route/duplicate, route/shadowed, route/duplicate-name diagnostics for routes
/// whose handlers live in `uri`. Compares against the full workspace route_index.
fn cross_route_diags(
    state: &WorkspaceState,
    uri: &Uri,
    linked: &crate::state::Linked,
    has_unresolved_routes: bool,
) -> Vec<Diagnostic> {
    let mut diags = vec![];

    // Collect all resolved route records from the workspace, ordered by registration ordinal.
    let mut all_records: Vec<&RouteRecord> = linked
        .route_index
        .values()
        .flat_map(|v| v.iter())
        .filter(|r| matches!(r.resolved_path, ResolvedPath::Resolved(_)))
        .collect();
    all_records.sort_by_key(|r| r.ordinal);

    // Collect records for *this file* (only emit diags for these)
    let file_records: Vec<&RouteRecord> = all_records
        .iter()
        .copied()
        .filter(|r| &r.handler.uri == uri)
        .collect();

    // ── route/router-not-included (REQ-DIAG-08) ──────────────────────────────
    // Run before the file_records early-return: a file may have routers but no resolved
    // routes (e.g. all routes unresolved), and we still need to fire this diagnostic.
    // Per-router gate: only suppress for routers whose name matches an unresolved include
    // target (i.e., something tried to include it but we couldn't resolve the file).
    let unresolved_include_targets: std::collections::HashSet<String> = if has_unresolved_routes {
        let all_known: std::collections::HashSet<String> = state
            .file_facts
            .iter()
            .flat_map(|e| {
                let v = e.value();
                let r: Vec<String> = v.routers.iter().map(|r| r.name.clone()).collect();
                let a: Vec<String> = v.apps.iter().map(|a| a.name.clone()).collect();
                r.into_iter().chain(a)
            })
            .collect();
        state
            .file_facts
            .iter()
            .flat_map(|e| {
                let facts = e.value();
                facts
                    .includes
                    .iter()
                    .filter_map(|inc| {
                        if all_known.contains(&inc.target) {
                            return None;
                        }
                        // Resolve alias: `from X import router as projects_router` → "router"
                        if let Some(original) = facts.import_alias_originals.get(&inc.target)
                            && all_known.contains(original)
                        {
                            return None;
                        }
                        Some(inc.target.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    for file_entry in state.file_facts.iter() {
        if file_entry.key() != uri {
            continue;
        }
        for router in &file_entry.routers {
            let suppressed = unresolved_include_targets
                .iter()
                .any(|t| t == &router.name || t.ends_with(&format!(".{}", router.name)));
            if suppressed {
                continue;
            }
            let has_include = state.file_facts.iter().any(|e| {
                let facts = e.value();
                facts.includes.iter().any(|inc| {
                    // Resolve alias to original name for comparison
                    let resolved = facts
                        .import_alias_originals
                        .get(&inc.target)
                        .map(|s| s.as_str())
                        .unwrap_or(inc.target.as_str());
                    resolved == router.name
                        || resolved.ends_with(&format!(".{}", router.name))
                        || inc.target == router.name
                        || inc.target.ends_with(&format!(".{}", router.name))
                })
            });
            if !has_include {
                diags.push(router_not_included_diag(&router.name, router.range));
            }
        }
    }

    if file_records.is_empty() {
        return diags;
    }

    // ── route/duplicate (REQ-DIAG-05) ────────────────────────────────────────
    // Two routes with same method + same normalized path pattern.
    for record in &file_records {
        let norm = normalize_path(path_str(record));
        let method = &record.method;
        // Find the earliest record with same method + norm pattern that is *not* this handler
        let first_dup = all_records.iter().find(|other| {
            other.ordinal < record.ordinal
                && &other.method == method
                && normalize_path(path_str(other)) == norm
                && (other.handler.uri != record.handler.uri
                    || other.handler.range != record.handler.range)
        });
        if let Some(other) = first_dup {
            diags.push(route_duplicate_diag(
                record.handler.range,
                &format!("{} {}", method, path_str(record)),
                Location {
                    uri: other.handler.uri.clone(),
                    range: other.handler.range,
                },
            ));
        }
    }

    // ── route/shadowed (REQ-DIAG-05) ─────────────────────────────────────────
    // A param route registered before a literal route that its converter would match.
    for record in &file_records {
        let segs = path_segments(path_str(record));
        if !segs.iter().all(|s| !is_param_segment(s)) {
            continue; // record itself has params — it's the shadowing route, not the shadowed one
        }
        // It's a literal route — check if any earlier param route shadows it.
        // Guard: the shadower must itself have at least one param segment (or the
        // path check would incorrectly fire for two identical literal routes).
        let shadower = all_records.iter().find(|other| {
            other.ordinal < record.ordinal
                && other.method == record.method
                && path_segments(path_str(other))
                    .iter()
                    .any(|s| is_param_segment(s))
                && literal_route_shadowed_by(path_str(record), path_str(other))
        });
        if let Some(other) = shadower {
            diags.push(route_shadowed_diag(
                record.handler.range,
                path_str(record),
                path_str(other),
                Location {
                    uri: other.handler.uri.clone(),
                    range: other.handler.range,
                },
            ));
        }
    }

    // ── route/duplicate-name (REQ-DIAG-07) ───────────────────────────────────
    // Two routes with different handlers share a route name (within the same namespace).
    // Only actionable when the name is actually used in a url_for / url_path_for call —
    // if nobody calls url_for('name'), a duplicate name is harmless.
    // The later-registered route (higher ordinal) gets WARNING; the earlier gets HINT.
    let url_for_used_names: std::collections::HashSet<String> = state
        .file_facts
        .iter()
        .flat_map(|e| {
            e.value()
                .url_for_sites
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
        })
        .collect();

    for record in &file_records {
        if record.name.is_empty() {
            continue;
        }
        if !url_for_used_names.contains(&record.name) {
            continue;
        }
        let other_handler = all_records.iter().find(|other| {
            other.name == record.name
                && (other.handler.uri != record.handler.uri
                    || other.handler.range != record.handler.range)
        });
        if let Some(other) = other_handler {
            let other_loc = Location {
                uri: other.handler.uri.clone(),
                range: other.handler.range,
            };
            if record.ordinal >= other.ordinal {
                diags.push(route_duplicate_name_diag(
                    record.handler.range,
                    &record.name,
                    other_loc,
                ));
            } else {
                diags.push(route_duplicate_name_hint(
                    record.handler.range,
                    &record.name,
                    other_loc,
                ));
            }
        }
    }

    diags
}

fn path_str(record: &RouteRecord) -> &str {
    match &record.resolved_path {
        ResolvedPath::Resolved(p) => p.as_str(),
        ResolvedPath::Unresolved => "",
    }
}

/// Normalize a path for duplicate detection: replace `{param}` and `{param:converter}`
/// segments with a placeholder so names don't matter, only structure.
fn normalize_path(path: &str) -> String {
    path.split('/')
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                "{}"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').collect()
}

pub fn is_param_segment(seg: &str) -> bool {
    seg.starts_with('{') && seg.ends_with('}')
}

/// Returns true when the literal path `literal` would be matched by the param route `param_route`.
/// Checks converter compatibility for the matching param segment.
pub fn literal_route_shadowed_by(literal: &str, param_route: &str) -> bool {
    let lit_segs: Vec<&str> = literal.split('/').collect();
    let par_segs: Vec<&str> = param_route.split('/').collect();
    if lit_segs.len() != par_segs.len() {
        return false;
    }
    for (l, p) in lit_segs.iter().zip(par_segs.iter()) {
        if is_param_segment(p) {
            // Check converter — {name:int} does not shadow non-integer literals
            if !converter_accepts(p, l) {
                return false;
            }
        } else if l != p {
            return false;
        }
    }
    true
}

/// Returns true if the converter of a param segment accepts the literal text.
fn converter_accepts(param_seg: &str, literal: &str) -> bool {
    let inner = &param_seg[1..param_seg.len() - 1];
    let converter_name = inner.split(':').nth(1).unwrap_or("str");
    match converter_name {
        "int" => literal.parse::<i64>().is_ok(),
        "float" => literal.parse::<f64>().is_ok(),
        "uuid" => literal.len() == 36 && literal.chars().filter(|&c| c == '-').count() == 4,
        _ => true,
    }
}

// ── Route param checks (REQ-DIAG-03/04) ──────────────────────────────────────

/// Parse `"Depends(fn_name)"` or `"Depends(mod.fn_name)"` → `"fn_name"` / `"mod.fn_name"`.
fn parse_depends_fn_name(text: &str) -> Option<String> {
    let inner = text.trim().strip_prefix("Depends(")?.strip_suffix(')')?;
    let name = inner.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    }
}

/// Compute `route/param-missing-arg` and `route/arg-missing-param` diagnostics for routes
/// whose handlers live in `uri`.
fn route_param_checks(
    state: &WorkspaceState,
    uri: &Uri,
    linked: &crate::state::Linked,
) -> Vec<Diagnostic> {
    let mut diags = vec![];

    // Build a cross-file map: containing_func → Vec<dep_fn_name> from all annotated_params.
    // Used for BFS through nested Depends() chains.
    let sig_deps_map: std::collections::HashMap<String, Vec<String>> = {
        let mut m: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for entry in state.file_facts.iter() {
            for ap in &entry.value().annotated_params {
                if let Some(dep) = parse_depends_fn_name(&ap.depends_text) {
                    m.entry(ap.containing_func.clone()).or_default().push(dep);
                }
            }
        }
        m
    };

    // Build workspace-wide dep_type_aliases: alias_name → dep_fn_name.
    // Populated from module-level `X = Annotated[T, Depends(fn)]` assignments.
    let all_dep_type_aliases: std::collections::HashMap<String, String> = {
        let mut m = std::collections::HashMap::new();
        for entry in state.file_facts.iter() {
            for (alias, dep_fn) in &entry.value().dep_type_aliases {
                m.insert(alias.clone(), dep_fn.clone());
            }
        }
        m
    };

    // Build per-func plain_typed_params lookup: func_name → Vec<type_name>.
    // Populated from handler params like `project: CurrentProject` (plain identifier types).
    let all_plain_typed: std::collections::HashMap<String, Vec<String>> = {
        let mut m: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for entry in state.file_facts.iter() {
            for ptp in &entry.value().plain_typed_params {
                m.entry(ptp.containing_func.clone())
                    .or_default()
                    .push(ptp.type_name.clone());
            }
        }
        m
    };

    let file_records: Vec<&crate::state::RouteRecord> = linked
        .route_index
        .values()
        .flat_map(|v| v.iter())
        .filter(|r| &r.handler.uri == uri)
        .filter(|r| matches!(r.resolved_path, ResolvedPath::Resolved(_)))
        .collect();

    for record in file_records {
        if !record.handler_params_known {
            continue;
        }
        if record.handler_has_splat_args {
            continue;
        }
        if record.path_params.is_empty() {
            continue;
        }

        // Collect all bound argument names via BFS through the full dep graph.
        // Seeds: handler direct params + deps from decorator kwarg + deps from handler signature.
        let mut bound: std::collections::HashSet<String> =
            record.handler_params.iter().cloned().collect();

        // sig_deps_map and all_plain_typed are keyed by the Python function name
        // (containing_func), NOT by the route name kwarg. When `name="foo.bar"` is
        // set on a decorator, record.name diverges from the Python function name, so
        // we must extract the function name from the RouteId instead.
        // RouteId format: "{uri}:{handler_func_name}:{METHOD}"
        let handler_func_name: &str = {
            let id_str = record.id.0.as_str();
            let uri_str = record.handler.uri.as_str();
            if let Some(rest) = id_str.strip_prefix(uri_str) {
                let trimmed = rest.trim_start_matches(':');
                if let Some(pos) = trimmed.rfind(':') {
                    &trimmed[..pos]
                } else {
                    trimmed
                }
            } else {
                &record.name
            }
        };

        let mut queue: std::collections::VecDeque<String> =
            record.dependencies.iter().cloned().collect();
        // Add deps from handler function-signature Depends() (not only dependencies=[] kwarg)
        if let Some(sig_deps) = sig_deps_map.get(handler_func_name) {
            queue.extend(sig_deps.iter().cloned());
        }
        // Add deps from plain-typed params whose type is a known dep type alias.
        // e.g. `project: CurrentProject` where `CurrentProject = Annotated[T, Depends(fetch_project)]`
        if let Some(plain_types) = all_plain_typed.get(handler_func_name) {
            for type_name in plain_types {
                if let Some(dep_fn) = all_dep_type_aliases.get(type_name) {
                    queue.push_back(dep_fn.clone());
                }
            }
        }

        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        while let Some(dep) = queue.pop_front() {
            if !visited.insert(dep.clone()) {
                continue;
            }
            if let Some(params) = linked.dep_params.get(&dep) {
                bound.extend(params.iter().cloned());
            }
            // Follow nested deps: what does this dep function itself Depend on?
            if let Some(nested) = sig_deps_map.get(&dep) {
                queue.extend(nested.iter().cloned());
            }
        }

        for path_param in &record.path_params {
            if bound.contains(&path_param.name) {
                continue;
            }
            // route/param-missing-arg: path param not bound by handler or deps
            let range = param_segment_range(record, &path_param.name);
            diags.push(param_missing_arg_diag(&path_param.name, range));
        }

        // route/arg-missing-param: handler param near-miss (edit distance ≤ 2)
        let path_param_names: std::collections::HashSet<&str> =
            record.path_params.iter().map(|p| p.name.as_str()).collect();
        // Params contributed by dep functions (not the handler itself) — exclude from near-miss.
        let dep_contributed: std::collections::HashSet<&str> = record
            .dependencies
            .iter()
            .flat_map(|dep_name| {
                linked
                    .dep_params
                    .get(dep_name)
                    .into_iter()
                    .flat_map(|v| v.iter().map(|s| s.as_str()))
            })
            .collect();
        let unbound_handler_params: Vec<&str> = record
            .handler_params
            .iter()
            .map(|p| p.as_str())
            .filter(|p| !path_param_names.contains(*p) && !dep_contributed.contains(*p))
            .collect();

        for path_param in &record.path_params {
            if bound.contains(&path_param.name) {
                continue; // already covered by param-missing-arg above — skip
            }
            for &handler_param in &unbound_handler_params {
                if edit_distance(handler_param, &path_param.name) <= 2 {
                    let range = handler_param_range(record, handler_param);
                    diags.push(arg_missing_param_diag(
                        handler_param,
                        &path_param.name,
                        range,
                    ));
                    break;
                }
            }
        }
    }

    dedup_diags(diags)
}

/// Emit `model/unknown-response-model` for routes whose `response_model` symbol
/// is not in the workspace model index and not imported into the handler's file.
fn model_unknown_response_model_checks(
    _state: &WorkspaceState,
    uri: &Uri,
    linked: &crate::state::Linked,
    facts: &crate::state::FileFacts,
) -> Vec<Diagnostic> {
    let mut diags = vec![];

    // Wildcard import (`from x import *`) means any name could be in scope.
    let has_wildcard = facts.imported_names.contains(&"*".to_owned());

    for record in linked
        .route_index
        .values()
        .flat_map(|v| v.iter())
        .filter(|r| &r.handler.uri == uri)
    {
        // Use response_model kwarg first; fall back to `-> T` return annotation.
        let model_name_opt = record
            .response_model
            .as_deref()
            .or(record.return_annotation.as_deref());
        let Some(model_name) = model_name_opt else {
            continue;
        };
        // Strip attribute access — `schemas.Book` → check "Book"
        let bare_name = model_name.rsplit('.').next().unwrap_or(model_name);

        // In model_index: workspace-defined model → OK
        if linked.model_index.contains_key(bare_name) {
            continue;
        }
        // Wildcard import could cover any name → silence (P4)
        if has_wildcard {
            continue;
        }
        // Explicitly imported name → might be external library → silence (P4)
        if facts.imported_names.iter().any(|n| n == bare_name) {
            continue;
        }

        let range = record.response_model_range.unwrap_or(record.handler.range);
        diags.push(unknown_response_model_diag(bare_name, range));
    }

    dedup_diags(diags)
}

/// Remove duplicate diagnostics that arise when the same route is mounted at multiple prefixes.
/// Keyed on (start_line, start_character, message) — same position + same text = same diagnostic.
fn dedup_diags(mut diags: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut seen = HashSet::new();
    diags.retain(|d| {
        seen.insert((
            d.range.start.line,
            d.range.start.character,
            d.message.clone(),
        ))
    });
    diags
}

pub fn unknown_response_model_diag(model_name: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::HINT),
        code: Some(NumberOrString::String(
            "model/unknown-response-model".to_owned(),
        )),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Unknown response model: {model_name}."),
        ..Default::default()
    }
}

/// Compute the range of `{param_name}` inside the path string of a route's decorator.
/// Falls back to the handler range when path_range is unavailable.
pub fn param_segment_range(record: &crate::state::RouteRecord, param_name: &str) -> Range {
    let path_range = match record.path_range {
        Some(r) => r,
        None => return record.handler.range,
    };
    let path = &record.decorator_path;
    let needle_exact = format!("{{{param_name}}}");
    let needle_colon = format!("{{{param_name}:");

    // Find byte position and full token extent
    let (byte_pos, token_end_byte) = if let Some(pos) = path.find(&needle_exact) {
        (pos, pos + needle_exact.len())
    } else if let Some(pos) = path.find(&needle_colon) {
        // {name:converter} — find closing brace
        let end = path[pos..]
            .find('}')
            .map(|i| pos + i + 1)
            .unwrap_or(pos + needle_colon.len());
        (pos, end)
    } else {
        return path_range;
    };

    // Use UTF-16 code-unit counts for LSP column offsets
    let utf16_before = path[..byte_pos].encode_utf16().count() as u32;
    let utf16_token = path[byte_pos..token_end_byte].encode_utf16().count() as u32;
    // Skip opening prefix+quote(s): `"` → 1, `r"` → 2, `"""` → 3, etc.
    let quote_width = record.path_quote_width.unwrap_or(1);
    let col_start = path_range.start.character + quote_width + utf16_before;
    let col_end = col_start + utf16_token;
    Range {
        start: tower_lsp_server::ls_types::Position::new(path_range.start.line, col_start),
        end: tower_lsp_server::ls_types::Position::new(path_range.start.line, col_end),
    }
}

/// Compute the range of a handler parameter in the function signature.
/// Falls back to the handler range.
pub fn handler_param_range(record: &crate::state::RouteRecord, param_name: &str) -> Range {
    record
        .handler_params
        .iter()
        .position(|p| p == param_name)
        .and_then(|idx| record.handler_param_ranges.get(idx).copied())
        .unwrap_or(record.handler.range)
}

/// Simple Levenshtein distance (capped at 3 for performance).
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    if m.abs_diff(n) > 2 {
        return usize::MAX; // length difference alone exceeds threshold
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0usize; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            curr[j] = if a[i - 1] == b[j - 1] {
                prev[j - 1]
            } else {
                1 + prev[j - 1].min(prev[j]).min(curr[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

pub fn param_missing_arg_diag(param_name: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("route/param-missing-arg".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Unbound path parameter: {param_name}."),
        ..Default::default()
    }
}

pub fn arg_missing_param_diag(handler_param: &str, path_param: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::HINT),
        code: Some(NumberOrString::String("route/arg-missing-param".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!(
            "Handler parameter not in path: {handler_param}. Did you mean {{{path_param}}}?"
        ),
        ..Default::default()
    }
}

pub fn route_duplicate_diag(range: Range, pattern: &str, related_loc: Location) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("route/duplicate".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Duplicate route: {pattern}."),
        related_information: Some(vec![DiagnosticRelatedInformation {
            location: related_loc,
            message: "first registration here".to_owned(),
        }]),
        ..Default::default()
    }
}

pub fn route_shadowed_diag(
    range: Range,
    literal_path: &str,
    shadowing_path: &str,
    related_loc: Location,
) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("route/shadowed".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Shadowed route: {literal_path}. Shadowed by {shadowing_path}."),
        related_information: Some(vec![DiagnosticRelatedInformation {
            location: related_loc,
            message: "shadowing route registered here".to_owned(),
        }]),
        ..Default::default()
    }
}

pub fn route_duplicate_name_diag(range: Range, name: &str, other_loc: Location) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("route/duplicate-name".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Duplicate route name: {name}."),
        related_information: Some(vec![DiagnosticRelatedInformation {
            location: other_loc,
            message: "other handler with same name".to_owned(),
        }]),
        ..Default::default()
    }
}

pub fn route_duplicate_name_hint(range: Range, name: &str, other_loc: Location) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::HINT),
        code: Some(NumberOrString::String("route/duplicate-name".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Route name '{name}' first used here."),
        tags: Some(vec![DiagnosticTag::UNNECESSARY]),
        related_information: Some(vec![DiagnosticRelatedInformation {
            location: other_loc,
            message: "re-used here".to_owned(),
        }]),
        ..Default::default()
    }
}

pub fn router_not_included_diag(name: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(
            "route/router-not-included".to_owned(),
        )),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Router not included: {name}."),
        tags: Some(vec![DiagnosticTag::UNNECESSARY]),
        ..Default::default()
    }
}

pub fn override_unused_diag(name: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::INFORMATION),
        code: Some(NumberOrString::String("di/override-unused".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Unused dependency override: {name}."),
        tags: Some(vec![DiagnosticTag::UNNECESSARY]),
        ..Default::default()
    }
}

pub fn depends_called_diag(range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String("di/depends-called".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: "Dependency called with (): remove () to pass the callable.".to_owned(),
        ..Default::default()
    }
}

pub fn undefined_key_diag(key: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::INFORMATION),
        code: Some(NumberOrString::String("env/undefined-key".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Undefined env key: {key}."),
        ..Default::default()
    }
}

pub fn settings_missing_env_diag(key: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(
            "settings/env-key-missing".to_owned(),
        )),
        source: Some("fastapi-lsp".to_owned()),
        message: format!("Required env key undeclared: {key}."),
        ..Default::default()
    }
}

pub fn unknown_token_url_diag(path: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String(
            "oauth2/unknown-token-url".to_owned(),
        )),
        message: format!("no route matches OAuth2 URL `{path}`"),
        source: Some("fastapi-lsp".to_owned()),
        ..Default::default()
    }
}

pub fn test_unknown_path_diag(path: &str, range: Range) -> Diagnostic {
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("test/unknown-path".to_owned())),
        message: format!("no route matches `{path}`"),
        source: Some("fastapi-lsp".to_owned()),
        ..Default::default()
    }
}

pub fn missing_template_diag(path: &str, suggestion: Option<&str>, range: Range) -> Diagnostic {
    let message = match suggestion {
        Some(s) => format!("Template not found: {path}. Did you mean {s}?"),
        None => format!("Template not found: {path}."),
    };
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        code: Some(NumberOrString::String("tpl/missing-template".to_owned())),
        source: Some("fastapi-lsp".to_owned()),
        message,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        FileFacts, Linked, Location as StateLocation, Method, PathConverter, PathParam, RouteId,
        WorkspaceState,
    };
    use std::sync::Arc;

    #[test]
    fn allowlist_contains_common_vars() {
        assert!(OS_CI_ALLOWLIST.contains(&"HOME"));
        assert!(OS_CI_ALLOWLIST.contains(&"PATH"));
        assert!(OS_CI_ALLOWLIST.contains(&"CI"));
        assert!(OS_CI_ALLOWLIST.contains(&"GITHUB_TOKEN"));
    }

    #[test]
    fn undefined_key_message_format() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(1, 10),
            end: Position::new(1, 20),
        };
        let d = undefined_key_diag("APP_TIMEOUT", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::INFORMATION));
        assert!(d.message.contains("APP_TIMEOUT"));
        assert!(d.message.contains("Undefined env key"));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("env/undefined-key".to_owned()))
        );
    }

    #[test]
    fn is_env_key_ignored_allowlist() {
        assert!(is_env_key_ignored("HOME", &[]));
        assert!(is_env_key_ignored("home", &[]));
        assert!(is_env_key_ignored("CI", &[]));
        assert!(is_env_key_ignored("GITHUB_TOKEN", &[]));
    }

    #[test]
    fn is_env_key_ignored_user_list() {
        let user = vec!["MY_INTERNAL_KEY".to_owned()];
        assert!(is_env_key_ignored("MY_INTERNAL_KEY", &user));
        assert!(is_env_key_ignored("my_internal_key", &user));
        assert!(!is_env_key_ignored("OTHER_KEY", &user));
    }

    #[test]
    fn is_env_key_ignored_secret_key_not_suppressed() {
        // SECRET_KEY was removed from the allowlist — it's not an OS/CI var
        assert!(!is_env_key_ignored("SECRET_KEY", &[]));
    }

    #[test]
    fn diagnostic_has_stable_source_and_code() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(0, 0),
            end: Position::new(0, 5),
        };
        let d = undefined_key_diag("MY_KEY", range);
        assert_eq!(d.source.as_deref(), Some("fastapi-lsp"));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("env/undefined-key".to_owned()))
        );
    }

    #[test]
    fn di_cycle_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(1, 4),
            end: Position::new(1, 20),
        };
        let uri: Uri = "file:///a.py".parse().unwrap();
        let d = di_cycle_diag(
            range,
            "dependency cycle: get_a → get_b → get_a",
            vec![],
            uri,
        );
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.code, Some(NumberOrString::String("di/cycle".to_owned())));
        assert_eq!(d.source.as_deref(), Some("fastapi-lsp"));
        assert!(d.message.contains("get_a → get_b → get_a"));
    }

    #[test]
    fn depends_called_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(2, 16),
            end: Position::new(2, 30),
        };
        let d = depends_called_diag(range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.source.as_deref(), Some("fastapi-lsp"));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("di/depends-called".to_owned()))
        );
        assert!(d.message.contains("remove ()"));
    }

    #[test]
    fn override_unused_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(3, 8),
            end: Position::new(3, 22),
        };
        let d = override_unused_diag("old_get_db", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::INFORMATION));
        assert_eq!(d.source.as_deref(), Some("fastapi-lsp"));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("di/override-unused".to_owned()))
        );
        assert!(d.message.contains("old_get_db"));
        assert!(d.message.contains("Unused dependency override"));
    }

    #[test]
    fn url_unknown_name_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(5, 20),
            end: Position::new(5, 36),
        };
        let d = url_unknown_name_diag("get_nosuchroute", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("url/unknown-name".to_owned()))
        );
        assert_eq!(d.source.as_deref(), Some("fastapi-lsp"));
        assert!(d.message.contains("get_nosuchroute"));
        assert!(d.message.contains("Unknown route name"));
    }

    #[test]
    fn url_param_mismatch_missing_param() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(6, 10),
            end: Position::new(6, 30),
        };
        let d = url_param_mismatch_diag("get_book", &["book_id"], &[], range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("url/param-mismatch".to_owned()))
        );
        assert!(d.message.contains("get_book"));
        assert!(d.message.contains("book_id"));
        assert!(d.message.contains("missing"));
    }

    #[test]
    fn url_param_mismatch_extra_kwarg() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(7, 4),
            end: Position::new(7, 24),
        };
        let d = url_param_mismatch_diag("list_books", &[], &["author_id"], range);
        assert!(d.message.contains("extra"));
        assert!(d.message.contains("author_id"));
    }

    #[test]
    fn url_unknown_name_suppressed_when_routes_unresolved() {
        use crate::config::ResolvedConfig;
        use crate::state::{
            FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId,
            RouteRecord, UrlForSite,
        };
        use std::sync::Arc;
        use tower_lsp_server::ls_types::{Position, Uri};

        let uri: Uri = "file:///app/main.py".parse().unwrap();
        let uri2: Uri = "file:///app/router.py".parse().unwrap();

        let mut facts = FileFacts::new(uri.clone());
        facts.url_for_sites.push(UrlForSite {
            name: "some_route".to_owned(),
            kwarg_names: vec![],
            has_splat_kwargs: false,
            range: Range {
                start: Position::new(3, 10),
                end: Position::new(3, 22),
            },
            name_range: None,
        });

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ));
        state.file_facts.insert(uri.clone(), facts);

        // Add an unresolved route to trigger the gate
        let rid = RouteId("app.unknown:GET".to_owned());
        let mut linked = Linked::default();
        linked.route_index.insert(
            rid,
            vec![RouteRecord {
                id: RouteId("app.unknown:GET".to_owned()),
                ordinal: 0,
                name: "unknown".to_owned(),
                method: Method::Get,
                resolved_path: ResolvedPath::Unresolved,
                decorator_path: "/unknown".to_owned(),
                chain: vec![],
                handler: StateLocation {
                    uri: uri2.clone(),
                    range: Range::default(),
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
            }],
        );
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let url_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("url/unknown-name".to_owned())))
            .collect();
        assert!(
            url_diags.is_empty(),
            "url/unknown-name should be suppressed when routes are unresolved"
        );
    }

    #[test]
    fn url_param_mismatch_suppressed_with_splat_kwargs() {
        use crate::config::ResolvedConfig;
        use crate::state::{
            FileFacts, Linked, Location as StateLocation, Method, PathConverter, PathParam,
            ResolvedPath, RouteId, RouteRecord, UrlForSite,
        };
        use std::sync::Arc;
        use tower_lsp_server::ls_types::{Position, Uri};

        let uri: Uri = "file:///app/main.py".parse().unwrap();

        let mut facts = FileFacts::new(uri.clone());
        facts.url_for_sites.push(UrlForSite {
            name: "get_book".to_owned(),
            kwarg_names: vec![],
            has_splat_kwargs: true, // **params style
            range: Range {
                start: Position::new(2, 8),
                end: Position::new(2, 28),
            },
            name_range: None,
        });

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ));
        state.file_facts.insert(uri.clone(), facts);

        let rid = RouteId("app.get_book:GET".to_owned());
        let mut linked = Linked::default();
        linked
            .route_names
            .insert("get_book".to_owned(), vec![rid.clone()]);
        linked.route_index.insert(
            rid,
            vec![RouteRecord {
                id: RouteId("app.get_book:GET".to_owned()),
                ordinal: 0,
                name: "get_book".to_owned(),
                method: Method::Get,
                resolved_path: ResolvedPath::Resolved("/books/{book_id}".to_owned()),
                decorator_path: "/books/{book_id}".to_owned(),
                chain: vec![],
                handler: StateLocation {
                    uri: uri.clone(),
                    range: Range::default(),
                },
                path_params: vec![PathParam {
                    name: "book_id".to_owned(),
                    converter: PathConverter::Int,
                }],
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
            }],
        );
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let mismatch_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("url/param-mismatch".to_owned())))
            .collect();
        assert!(
            mismatch_diags.is_empty(),
            "url/param-mismatch should be suppressed when **splat kwargs present"
        );
    }

    // ── Cross-route check helpers ─────────────────────────────────────────────

    #[test]
    fn normalize_path_replaces_param_names() {
        assert_eq!(normalize_path("/books/{book_id}"), "/books/{}");
        assert_eq!(normalize_path("/books/{id}"), "/books/{}");
        assert_eq!(
            normalize_path("/books/{book_id}/chapters/{chapter_id}"),
            "/books/{}/chapters/{}"
        );
        assert_eq!(normalize_path("/books"), "/books");
        // Trailing slashes are distinct patterns (REQ-DIAG-05) — must NOT be collapsed
        assert_ne!(normalize_path("/books/"), normalize_path("/books"));
        assert_eq!(normalize_path("/books/"), "/books/");
    }

    #[test]
    fn converter_accepts_str_matches_anything() {
        assert!(converter_accepts("{id}", "featured"));
        assert!(converter_accepts("{id:str}", "123"));
    }

    #[test]
    fn converter_accepts_int_rejects_non_integer() {
        assert!(converter_accepts("{id:int}", "42"));
        assert!(!converter_accepts("{id:int}", "featured"));
        assert!(!converter_accepts("{id:int}", "3.14"));
    }

    #[test]
    fn literal_route_shadowed_by_str_param() {
        assert!(literal_route_shadowed_by("/books/featured", "/books/{id}"));
    }

    #[test]
    fn literal_route_not_shadowed_by_int_param() {
        assert!(!literal_route_shadowed_by(
            "/books/featured",
            "/books/{id:int}"
        ));
    }

    #[test]
    fn route_duplicate_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(10, 4),
            end: Position::new(10, 18),
        };
        let uri: Uri = "file:///a.py".parse().unwrap();
        let other = Location {
            uri,
            range: Range::default(),
        };
        let d = route_duplicate_diag(range, "GET /books/{id}", other);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("route/duplicate".to_owned()))
        );
        assert!(d.related_information.is_some());
        assert!(d.message.contains("Duplicate route"));
    }

    #[test]
    fn route_shadowed_not_fired_for_two_identical_literal_routes() {
        use crate::config::ResolvedConfig;
        use crate::state::{FileFacts, Linked};
        use std::sync::Arc;
        use tower_lsp_server::ls_types::Uri;

        let uri: Uri = "file:///router.py".parse().unwrap();
        let (rid1, rec1) = make_route_record(&uri, "view2", 0, 1);
        let (rid2, rec2) = make_route_record(&uri, "pca_import_view", 1, 4);

        let mut linked = Linked::default();
        linked.route_index.insert(rid1, vec![rec1]);
        linked.route_index.insert(rid2, vec![rec2]);

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ));
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let shadowed_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("route/shadowed".to_owned())))
            .collect();
        assert!(
            shadowed_diags.is_empty(),
            "route/shadowed must not fire for two identical literal routes; got {:?}",
            shadowed_diags,
        );
    }

    #[test]
    fn route_shadowed_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(12, 4),
            end: Position::new(12, 20),
        };
        let uri: Uri = "file:///b.py".parse().unwrap();
        let other = Location {
            uri,
            range: Range::default(),
        };
        let d = route_shadowed_diag(range, "/books/featured", "/books/{id}", other);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("route/shadowed".to_owned()))
        );
        assert!(d.message.contains("Shadowed route"));
        assert!(d.related_information.is_some());
    }

    #[test]
    fn route_duplicate_name_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(8, 4),
            end: Position::new(8, 16),
        };
        let uri: Uri = "file:///c.py".parse().unwrap();
        let other = Location {
            uri: uri.clone(),
            range: Range::default(),
        };
        let d = route_duplicate_name_diag(range, "get_book", other);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("route/duplicate-name".to_owned()))
        );
        assert!(d.message.contains("get_book"));
        assert!(d.related_information.as_ref().map(|v| v.len()).unwrap_or(0) > 0);

        let other2 = Location {
            uri,
            range: Range::default(),
        };
        let h = route_duplicate_name_hint(range, "get_book", other2);
        assert_eq!(h.severity, Some(DiagnosticSeverity::HINT));
        assert_eq!(
            h.code,
            Some(NumberOrString::String("route/duplicate-name".to_owned()))
        );
        assert!(h.message.contains("get_book"));
        assert!(h.related_information.as_ref().map(|v| v.len()).unwrap_or(0) > 0);
    }

    #[test]
    fn router_not_included_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(3, 0),
            end: Position::new(3, 14),
        };
        let d = router_not_included_diag("book_router", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String(
                "route/router-not-included".to_owned()
            ))
        );
        assert!(d.message.contains("book_router"));
        assert!(d.message.contains("Router not included"));
    }

    #[test]
    fn settings_missing_env_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(5, 4),
            end: Position::new(5, 14),
        };
        let d = settings_missing_env_diag("DATABASE_URL", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String(
                "settings/env-key-missing".to_owned()
            ))
        );
        assert_eq!(d.source.as_deref(), Some("fastapi-lsp"));
        assert!(d.message.contains("DATABASE_URL"), "message: {}", d.message);
        assert!(d.message.contains("Required"), "message: {}", d.message);
        assert!(
            d.tags.is_none(),
            "settings/env-key-missing should not be tagged UNNECESSARY"
        );
    }

    // ── route/duplicate end-to-end ────────────────────────────────────────────

    fn make_route_record(
        uri: &Uri,
        handler: &str,
        ordinal: u32,
        line: u32,
    ) -> (crate::state::RouteId, crate::state::RouteRecord) {
        use crate::state::{Location as StateLocation, Method, ResolvedPath, RouteId, RouteRecord};
        use tower_lsp_server::ls_types::Position;
        let rid = RouteId(format!("{}:{}:GET", uri.as_str(), handler));
        let rec = RouteRecord {
            id: rid.clone(),
            ordinal,
            name: handler.to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/pca/import-from-pca".to_owned()),
            decorator_path: "/import-from-pca".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range {
                    start: Position::new(line, 0),
                    end: Position::new(line, 10),
                },
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
            handler_params_known: true,
        };
        (rid, rec)
    }

    #[test]
    fn route_duplicate_fires_for_same_path_same_router() {
        use crate::config::ResolvedConfig;
        use crate::state::{FileFacts, Linked};
        use std::sync::Arc;
        use tower_lsp_server::ls_types::Uri;

        let uri: Uri = "file:///router.py".parse().unwrap();
        let (rid1, rec1) = make_route_record(&uri, "view2", 0, 1);
        let (rid2, rec2) = make_route_record(&uri, "pca_import_view", 1, 4);

        let mut linked = Linked::default();
        linked.route_index.insert(rid1, vec![rec1]);
        linked.route_index.insert(rid2, vec![rec2]);

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ));
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let dup_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("route/duplicate".to_owned())))
            .collect();
        assert_eq!(
            dup_diags.len(),
            1,
            "route/duplicate should fire once; got {:?}",
            dup_diags
        );
        assert!(
            dup_diags[0].message.contains("/pca/import-from-pca"),
            "message: {}",
            dup_diags[0].message,
        );
    }

    // ── Route param check helpers ─────────────────────────────────────────────

    #[test]
    fn edit_distance_exact_match() {
        assert_eq!(edit_distance("book_id", "book_id"), 0);
    }

    #[test]
    fn edit_distance_one_typo() {
        assert_eq!(edit_distance("book_idd", "book_id"), 1);
        assert_eq!(edit_distance("bok_id", "book_id"), 1);
    }

    #[test]
    fn edit_distance_two_changes() {
        // "bkk_id" vs "book_id": delete 'o' + substitute first 'k' → 2 operations
        assert_eq!(edit_distance("bkk_id", "book_id"), 2);
    }

    #[test]
    fn edit_distance_fast_exit_on_large_delta() {
        assert!(edit_distance("a", "abcdefgh") > 2);
    }

    #[test]
    fn param_missing_arg_diag_properties() {
        let range = Range::default();
        let d = param_missing_arg_diag("book_id", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("route/param-missing-arg".to_owned()))
        );
        assert_eq!(d.source, Some("fastapi-lsp".to_owned()));
        assert!(d.message.contains("book_id"));
    }

    #[test]
    fn arg_missing_param_diag_properties() {
        let range = Range::default();
        let d = arg_missing_param_diag("book_idd", "book_id", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::HINT));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("route/arg-missing-param".to_owned()))
        );
        assert!(d.message.contains("book_idd"));
        assert!(d.message.contains("book_id"));
    }

    fn make_route_record_with_params(
        uri: &Uri,
        path: &str,
        path_params: Vec<PathParam>,
        handler_params: Vec<String>,
        handler_has_splat_args: bool,
        handler_params_known: bool,
    ) -> (RouteId, RouteRecord) {
        let id = RouteId(format!("app.handler:{path}:GET"));
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "handler".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved(path.to_owned()),
            decorator_path: path.to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
            path_params,
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: None,
            path_quote_width: None,
            handler_params,
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args,
            handler_params_known,
        };
        (id, record)
    }

    #[test]
    fn no_diags_when_param_bound_by_handler() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let path_params = vec![PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        }];
        let (id, record) = make_route_record_with_params(
            &uri,
            "/books/{book_id}",
            path_params,
            vec!["book_id".to_owned()],
            false,
            true,
        );
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let param_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert!(param_diags.is_empty(), "no diag when handler has param");
    }

    #[test]
    fn param_missing_arg_fires_when_unbound() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let path_params = vec![PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        }];
        let (id, record) = make_route_record_with_params(
            &uri,
            "/books/{book_id}",
            path_params,
            vec!["title".to_owned()],
            false,
            true,
        );
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let param_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert_eq!(param_diags.len(), 1);
        assert!(param_diags[0].message.contains("book_id"));
    }

    #[test]
    fn param_missing_arg_not_duplicated_for_multi_mount() {
        // Same handler mounted at /v1 and /v2 → two RouteRecords with same handler range.
        // dedup_diags must produce exactly ONE diagnostic, not two.
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let path_params = vec![PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        }];
        let (id, record_v1) = make_route_record_with_params(
            &uri,
            "/v1/books/{book_id}",
            path_params.clone(),
            vec!["title".to_owned()],
            false,
            true,
        );
        // Second mount: same handler, different resolved path
        let record_v2 = RouteRecord {
            id: id.clone(),
            resolved_path: ResolvedPath::Resolved("/v2/books/{book_id}".to_owned()),
            decorator_path: "/books/{book_id}".to_owned(),
            ..record_v1.clone()
        };
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record_v1, record_v2]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let param_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert_eq!(
            param_diags.len(),
            1,
            "multi-mount must produce exactly one param-missing-arg, not one per mount"
        );
    }

    #[test]
    fn param_missing_arg_suppressed_by_splat() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let path_params = vec![PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        }];
        let (id, record) = make_route_record_with_params(
            &uri,
            "/books/{book_id}",
            path_params,
            vec!["title".to_owned()],
            true,
            true,
        );
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let param_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert!(param_diags.is_empty(), "splat suppresses param-missing-arg");
    }

    #[test]
    fn arg_missing_param_fires_on_near_miss() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let path_params = vec![PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        }];
        // handler has "book_idd" (typo, edit distance 1) but not "book_id"
        let (id, record) = make_route_record_with_params(
            &uri,
            "/books/{book_id}",
            path_params,
            vec!["book_idd".to_owned()],
            false,
            true,
        );
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        assert!(
            diags
                .iter()
                .any(|d| d.code
                    == Some(NumberOrString::String("route/arg-missing-param".to_owned()))),
            "arg-missing-param should fire on near-miss rename"
        );
        assert!(
            diags
                .iter()
                .any(|d| d.code
                    == Some(NumberOrString::String("route/param-missing-arg".to_owned()))),
            "param-missing-arg should also fire"
        );
    }

    #[test]
    fn param_bound_by_dep_suppresses_diag() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        // dep_def with book_id param
        facts.dep_defs.push(crate::state::DepDef {
            name: "verify_owner".to_owned(),
            node_id: crate::state::NodeId {
                uri: uri.clone(),
                range: Range::default(),
            },
            has_yield: false,
            param_names: vec!["book_id".to_owned()],
        });
        state.file_facts.insert(uri.clone(), facts);

        let path_params = vec![PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        }];
        let mut record_base = make_route_record_with_params(
            &uri,
            "/books/{book_id}",
            path_params,
            vec!["user".to_owned()],
            false,
            true,
        );
        // handler depends on verify_owner
        record_base.1.dependencies = vec!["verify_owner".to_owned()];
        let (id, record) = record_base;
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        linked
            .dep_params
            .insert("verify_owner".to_owned(), vec!["book_id".to_owned()]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let param_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert!(
            param_diags.is_empty(),
            "dep binding suppresses param-missing-arg"
        );
    }

    #[test]
    fn param_bound_via_type_alias_dep_with_custom_route_name() {
        // Regression: when a route has `name="api.contracts.create"`, record.name differs from
        // the Python function name. The BFS must look up sig_deps_map and all_plain_typed by
        // the handler function name, not the route name kwarg.
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///router.py".parse().unwrap();

        let handler_func = "create_contract_view";
        let route_name = "api.contracts.create";

        let mut facts = FileFacts::new(uri.clone());
        // dep_def: fetch_contract takes contract_id
        facts.dep_defs.push(crate::state::DepDef {
            name: "fetch_contract".to_owned(),
            node_id: crate::state::NodeId {
                uri: uri.clone(),
                range: Range::default(),
            },
            has_yield: false,
            param_names: vec!["dbsession".to_owned(), "contract_id".to_owned()],
        });
        // dep_type_alias: CurrentContract = Annotated[..., Depends(fetch_contract)]
        facts
            .dep_type_aliases
            .insert("CurrentContract".to_owned(), "fetch_contract".to_owned());
        // plain_typed_param: create_contract_view has `contract: CurrentContract`
        facts
            .plain_typed_params
            .push(crate::state::PlainTypedParam {
                containing_func: handler_func.to_owned(),
                param_name: "contract".to_owned(),
                type_name: "CurrentContract".to_owned(),
                annotation_range: Range::default(),
            });
        state.file_facts.insert(uri.clone(), facts);

        // RouteId must use the real handler func name so extraction works
        let id = RouteId(format!("{}:{}:POST", uri.as_str(), handler_func));
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: route_name.to_owned(), // custom name kwarg — differs from handler func name
            method: Method::Post,
            resolved_path: ResolvedPath::Resolved("/contracts/{contract_id}".to_owned()),
            decorator_path: "/contracts/{contract_id}".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
            path_params: vec![PathParam {
                name: "contract_id".to_owned(),
                converter: PathConverter::Str,
            }],
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: None,
            path_quote_width: None,
            handler_params: vec!["dbsession".to_owned(), "contract".to_owned()],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        };
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        linked.dep_params.insert(
            "fetch_contract".to_owned(),
            vec!["dbsession".to_owned(), "contract_id".to_owned()],
        );
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let param_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert!(
            param_diags.is_empty(),
            "contract_id bound via type alias dep must not produce param-missing-arg even with custom route name; got {:?}",
            param_diags,
        );
    }

    #[test]
    fn route_param_checks_suppressed_when_params_unknown() {
        // Table-style routes (handler_params_known: false) must not emit route/param-missing-arg
        // or route/arg-missing-param — we cannot reliably extract their handler signatures.
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let path_params = vec![PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        }];
        // handler_params_known: false — table-style route, params cannot be extracted
        let (id, record) = make_route_record_with_params(
            &uri,
            "/books/{book_id}",
            path_params,
            vec![],
            false,
            false,
        );
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let route_diags: Vec<_> = diags.iter()
            .filter(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c.starts_with("route/param-")))
            .collect();
        assert!(
            route_diags.is_empty(),
            "route/param-* must be suppressed when handler_params_known is false"
        );
    }

    // ── model/unknown-response-model tests ───────────────────────────────────

    fn make_route_with_response_model(
        uri: &Uri,
        model_name: &str,
    ) -> (RouteId, crate::state::RouteRecord) {
        use crate::state::{ResolvedPath, RouteRecord};
        let id = RouteId(format!("{}:GET", model_name));
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: model_name.to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved(format!("/{}", model_name.to_lowercase())),
            decorator_path: format!("/{}", model_name.to_lowercase()),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
            path_params: vec![],
            response_model: Some(model_name.to_owned()),
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
        (id, record)
    }

    #[test]
    fn unknown_response_model_fires_when_not_in_index_or_imports() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let (id, record) = make_route_with_response_model(&uri, "Book");
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        assert!(
            diags.iter().any(|d| d.code
                == Some(NumberOrString::String(
                    "model/unknown-response-model".to_owned()
                ))),
            "should fire when model is not in index or imports"
        );
    }

    #[test]
    fn unknown_response_model_suppressed_when_in_model_index() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let model_uri: Uri = "file:///models.py".parse().unwrap();
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let (id, record) = make_route_with_response_model(&uri, "Book");
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        linked.model_index.insert(
            "Book".to_owned(),
            vec![crate::state::ModelRecord {
                name: "Book".to_owned(),
                location: StateLocation {
                    uri: model_uri,
                    range: Range::default(),
                },
                is_settings: false,
            }],
        );
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let model_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "model/unknown-response-model".to_owned(),
                    ))
            })
            .collect();
        assert!(
            model_diags.is_empty(),
            "model in index suppresses diagnostic"
        );
    }

    #[test]
    fn unknown_response_model_suppressed_when_imported() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        facts.imported_names.push("Book".to_owned());
        state.file_facts.insert(uri.clone(), facts);
        let (id, record) = make_route_with_response_model(&uri, "Book");
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let model_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "model/unknown-response-model".to_owned(),
                    ))
            })
            .collect();
        assert!(
            model_diags.is_empty(),
            "imported name suppresses diagnostic"
        );
    }

    #[test]
    fn unknown_response_model_suppressed_by_wildcard_import() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        facts.imported_names.push("*".to_owned());
        state.file_facts.insert(uri.clone(), facts);
        let (id, record) = make_route_with_response_model(&uri, "Book");
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let model_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "model/unknown-response-model".to_owned(),
                    ))
            })
            .collect();
        assert!(
            model_diags.is_empty(),
            "wildcard import suppresses diagnostic"
        );
    }

    fn make_route_with_return_annotation(
        uri: &Uri,
        annotation: &str,
    ) -> (RouteId, crate::state::RouteRecord) {
        use crate::state::{ResolvedPath, RouteRecord};
        let id = RouteId(format!("{}:GET", annotation));
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: annotation.to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved(format!("/{}", annotation.to_lowercase())),
            decorator_path: format!("/{}", annotation.to_lowercase()),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
            path_params: vec![],
            response_model: None,
            response_model_range: None,
            return_annotation: Some(annotation.to_owned()),
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
        (id, record)
    }

    #[test]
    fn return_annotation_fires_when_not_in_index_or_imports() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let facts = FileFacts::new(uri.clone());
        state.file_facts.insert(uri.clone(), facts);
        let (id, record) = make_route_with_return_annotation(&uri, "Book");
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let model_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "model/unknown-response-model".to_owned(),
                    ))
            })
            .collect();
        assert_eq!(
            model_diags.len(),
            1,
            "return annotation fires when model not found"
        );
    }

    #[test]
    fn return_annotation_suppressed_when_in_model_index() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let facts = FileFacts::new(uri.clone());
        state.file_facts.insert(uri.clone(), facts);
        let (id, record) = make_route_with_return_annotation(&uri, "Book");
        let mut linked = Linked::default();
        linked.model_index.insert(
            "Book".to_owned(),
            vec![crate::state::ModelRecord {
                name: "Book".to_owned(),
                location: StateLocation {
                    uri: uri.clone(),
                    range: Range::default(),
                },
                is_settings: false,
            }],
        );
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let model_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "model/unknown-response-model".to_owned(),
                    ))
            })
            .collect();
        assert!(
            model_diags.is_empty(),
            "return annotation suppressed when model in index"
        );
    }

    #[test]
    fn return_annotation_suppressed_when_imported() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        facts.imported_names.push("Book".to_owned());
        state.file_facts.insert(uri.clone(), facts);
        let (id, record) = make_route_with_return_annotation(&uri, "Book");
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let model_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "model/unknown-response-model".to_owned(),
                    ))
            })
            .collect();
        assert!(
            model_diags.is_empty(),
            "return annotation suppressed when name is imported"
        );
    }

    #[test]
    fn response_model_kwarg_takes_precedence_over_return_annotation() {
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///a.py".parse().unwrap();
        let facts = FileFacts::new(uri.clone());
        state.file_facts.insert(uri.clone(), facts);
        // response_model="Book" (unknown) but return_annotation="Author" (also unknown)
        // The kwarg takes precedence; diagnostic should name "Book"
        use crate::state::{ResolvedPath, RouteRecord};
        let id = RouteId("test:GET".to_owned());
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "test".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/test".to_owned()),
            decorator_path: "/test".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
            path_params: vec![],
            response_model: Some("Book".to_owned()),
            response_model_range: None,
            return_annotation: Some("Author".to_owned()),
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
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let model_diags: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "model/unknown-response-model".to_owned(),
                    ))
            })
            .collect();
        assert_eq!(model_diags.len(), 1);
        assert!(
            model_diags[0].message.contains("Book"),
            "diagnostic names the kwarg model, not the annotation"
        );
    }

    // ── tpl/missing-template ──────────────────────────────────────────────────

    fn tpl_range() -> Range {
        use tower_lsp_server::ls_types::Position;
        Range {
            start: Position::new(3, 10),
            end: Position::new(3, 24),
        }
    }

    #[test]
    fn missing_template_diag_no_suggestion() {
        let d = missing_template_diag("missing.html", None, tpl_range());
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("tpl/missing-template".to_owned()))
        );
        assert!(d.message.contains("missing.html"), "message contains path");
        assert!(
            !d.message.contains("did you mean"),
            "no suggestion clause when None"
        );
    }

    #[test]
    fn missing_template_diag_with_suggestion() {
        let d = missing_template_diag("book_lst.html", Some("book_list.html"), tpl_range());
        assert!(d.message.contains("book_lst.html"));
        assert!(d.message.contains("book_list.html"));
        assert!(d.message.contains("Did you mean"));
    }

    #[test]
    fn compute_fires_missing_template_when_roots_present() {
        use crate::state::TemplateRef;
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///app.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "missing.html".to_owned(),
            range: tpl_range(),
        });
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        // Index has some entries (roots "exist") but not the referenced template.
        linked.template_index.insert(
            "index.html".to_owned(),
            "file:///tpl/index.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let tpl_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("tpl/missing-template".to_owned())))
            .collect();
        assert_eq!(tpl_diags.len(), 1);
        assert!(tpl_diags[0].message.contains("missing.html"));
    }

    #[test]
    fn compute_suppressed_when_no_template_roots() {
        use crate::state::TemplateRef;
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///app.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "missing.html".to_owned(),
            range: tpl_range(),
        });
        state.file_facts.insert(uri.clone(), facts);
        // Empty index — no roots scanned → stay silent (P4).
        state.linked.store(Arc::new(Linked::default()));

        let diags = compute(&state, &uri, &[]);
        let tpl_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("tpl/missing-template".to_owned())))
            .collect();
        assert!(
            tpl_diags.is_empty(),
            "must not fire when no template roots configured"
        );
    }

    #[test]
    fn compute_no_diag_when_template_found() {
        use crate::state::TemplateRef;
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///app.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "index.html".to_owned(),
            range: tpl_range(),
        });
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        linked.template_index.insert(
            "index.html".to_owned(),
            "file:///tpl/index.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let tpl_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("tpl/missing-template".to_owned())))
            .collect();
        assert!(tpl_diags.is_empty(), "no diag when template is in index");
    }

    #[test]
    fn compute_includes_near_miss_suggestion() {
        use crate::state::TemplateRef;
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///app.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        // "book_lst.html" is edit-distance-1 from "book_list.html"
        facts.templates.push(TemplateRef {
            path: "book_lst.html".to_owned(),
            range: tpl_range(),
        });
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        linked.template_index.insert(
            "book_list.html".to_owned(),
            "file:///tpl/book_list.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let tpl_diags: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("tpl/missing-template".to_owned())))
            .collect();
        assert_eq!(tpl_diags.len(), 1);
        assert!(
            tpl_diags[0].message.contains("book_list.html"),
            "suggestion in message"
        );
        assert!(tpl_diags[0].message.contains("Did you mean"));
    }

    // ── UTF-16 column tests (CG-1) ────────────────────────────────────────────

    fn make_route_for_range(path: &str, path_start_col: u32) -> crate::state::RouteRecord {
        use crate::state::{Location as StateLocation, Method, ResolvedPath, RouteId};
        let uri: tower_lsp_server::ls_types::Uri = "file:///app.py".parse().unwrap();
        crate::state::RouteRecord {
            id: RouteId("test".to_owned()),
            ordinal: 0,
            name: "handler".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved(path.to_owned()),
            decorator_path: path.to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range {
                    start: tower_lsp_server::ls_types::Position::new(0, 0),
                    end: tower_lsp_server::ls_types::Position::new(0, 4),
                },
            },
            path_params: vec![],
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: Some(Range {
                start: tower_lsp_server::ls_types::Position::new(2, path_start_col),
                end: tower_lsp_server::ls_types::Position::new(
                    2,
                    path_start_col + path.encode_utf16().count() as u32 + 2,
                ),
            }),
            path_quote_width: Some(1), // single opening quote
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: false,
        }
    }

    #[test]
    fn param_segment_range_ascii_path() {
        // "/items/{id}" — all ASCII, each char = 1 UTF-16 unit
        // path_start_col=10 (column of the opening quote), quote_width=1
        // path char col 10 + 1(quote) + 7("/items/") = 18 for "{id}"
        let record = make_route_for_range("/items/{id}", 10);
        let r = param_segment_range(&record, "id");
        assert_eq!(
            r.start.character,
            10 + 1 + 7,
            "ASCII: col_start should be 18"
        );
        assert_eq!(
            r.end.character,
            10 + 1 + 7 + 4,
            "ASCII: col_end = start + len({{id}})"
        );
    }

    #[test]
    fn param_segment_range_multibyte_before_param() {
        // "/café/{id}" — "café" has é = U+00E9, 1 UTF-16 unit but 2 UTF-8 bytes
        // "/café/" = 7 UTF-16 units (/, c, a, f, é, /, 7 chars)
        // with quote_width=1 and path_start_col=5:
        // col_start = 5 + 1 + 7 = 13
        let path = "/café/{id}";
        let utf16_prefix = "/café/".encode_utf16().count() as u32; // should be 6
        let record = make_route_for_range(path, 5);
        let r = param_segment_range(&record, "id");
        let expected_start = 5 + 1 + utf16_prefix;
        assert_eq!(
            r.start.character, expected_start,
            "é is 1 UTF-16 unit — col should be {expected_start}"
        );
        assert_eq!(
            r.end.character,
            expected_start + 4,
            "{{id}} = 4 UTF-16 units"
        );
    }

    #[test]
    fn param_segment_range_emoji_before_param() {
        // "/🚀/{id}" — 🚀 = U+1F680, a surrogate pair = 2 UTF-16 units
        // "/" + 🚀 + "/{id}" → "/🚀/" = 1 + 2 + 1 = 4 UTF-16 units before "{id}"
        let path = "/🚀/{id}";
        let utf16_prefix = "/🚀/".encode_utf16().count() as u32; // should be 4
        let record = make_route_for_range(path, 3);
        let r = param_segment_range(&record, "id");
        let expected_start = 3 + 1 + utf16_prefix;
        assert_eq!(
            r.start.character, expected_start,
            "🚀 = 2 UTF-16 units — col should be {expected_start}"
        );
        assert_eq!(
            r.end.character,
            expected_start + 4,
            "{{id}} = 4 UTF-16 units"
        );
    }

    // ── Issue 5: router-not-included per-router gate ─────────────────────────

    fn make_router_not_included_state(
        router_name: &str,
        has_unresolved_route: bool,
        unresolved_include_target: Option<&str>,
    ) -> (std::sync::Arc<crate::state::WorkspaceState>, Uri) {
        use crate::config::ResolvedConfig;
        use crate::state::{
            FileFacts, IncludeCall, Linked, Location as StateLocation, Method, PrefixValue,
            ResolvedPath, RouteId, RouteRecord, RouterDecl,
        };
        use std::sync::Arc;
        use tower_lsp_server::ls_types::Position;

        let uri: Uri = "file:///myapp.py".parse().unwrap();
        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ));

        let mut facts = FileFacts::new(uri.clone());
        facts.routers.push(RouterDecl {
            name: router_name.to_owned(),
            prefix: PrefixValue::Unresolved,
            tags: vec![],
            range: Range {
                start: Position::new(1, 0),
                end: Position::new(1, 10),
            },
        });

        // Optionally add an unresolved include pointing to something OTHER than our router
        if let Some(target) = unresolved_include_target {
            facts.includes.push(IncludeCall {
                target: target.to_owned(),
                prefix: PrefixValue::Unresolved,
                app_name: "app".to_owned(),
                dependencies: vec![],
                range: Range::default(),
            });
        }

        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        if has_unresolved_route {
            let id = RouteId("unresolved:test".to_owned());
            linked.route_index.insert(
                id,
                vec![RouteRecord {
                    id: RouteId("unresolved:test".to_owned()),
                    ordinal: 0,
                    name: "some_view".to_owned(),
                    method: Method::Get,
                    resolved_path: ResolvedPath::Unresolved,
                    decorator_path: "/test".to_owned(),
                    chain: vec![],
                    handler: StateLocation {
                        uri: uri.clone(),
                        range: Range::default(),
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
                    handler_params_known: true,
                }],
            );
        }
        state.linked.store(Arc::new(linked));

        (state, uri)
    }

    #[test]
    fn router_not_included_fires_even_when_other_routes_unresolved() {
        // never_router is not included anywhere; even though there's an unresolved route,
        // the diagnostic should fire because no include targets match "never_router"
        let (state, uri) = make_router_not_included_state("never_router", true, None);
        let diags = compute(&state, &uri, &[]);
        let not_included: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "route/router-not-included".to_owned(),
                    ))
            })
            .collect();
        assert!(
            !not_included.is_empty(),
            "router-not-included must fire for never_router even when other routes are unresolved; got {:?}",
            not_included,
        );
    }

    #[test]
    fn router_not_included_suppressed_when_target_matches_unresolved_include() {
        // there's an unresolved include whose target IS "never_router" → suppress
        let (state, uri) =
            make_router_not_included_state("never_router", true, Some("never_router"));
        let diags = compute(&state, &uri, &[]);
        let not_included: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "route/router-not-included".to_owned(),
                    ))
            })
            .collect();
        assert!(
            not_included.is_empty(),
            "router-not-included must be suppressed when an unresolved include targets this router; got {:?}",
            not_included,
        );
    }

    // ── Issue 1: route/duplicate-name gated on url_for usage ─────────────────

    fn make_dup_name_state(
        name: &str,
        path_a: &str,
        path_b: &str,
        url_for_name: Option<&str>,
    ) -> (std::sync::Arc<crate::state::WorkspaceState>, Uri) {
        use crate::config::ResolvedConfig;
        use crate::state::{
            FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId,
            RouteRecord, UrlForSite,
        };
        use std::sync::Arc;
        use tower_lsp_server::ls_types::Position;

        let uri_a: Uri = "file:///a.py".parse().unwrap();
        let uri_b: Uri = "file:///b.py".parse().unwrap();

        let make_rec = |uri: &Uri, ordinal: u32, path: &str| -> (RouteId, RouteRecord) {
            let id = RouteId(format!("{}:{}:{}", uri.as_str(), path, ordinal));
            let rec = RouteRecord {
                id: id.clone(),
                ordinal,
                name: name.to_owned(),
                method: Method::Get,
                resolved_path: ResolvedPath::Resolved(path.to_owned()),
                decorator_path: path
                    .split('/')
                    .next_back()
                    .map(|s| format!("/{s}"))
                    .unwrap_or_else(|| path.to_owned()),
                chain: vec![],
                handler: StateLocation {
                    uri: uri.clone(),
                    range: Range {
                        start: Position::new(1, 0),
                        end: Position::new(1, 10),
                    },
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
                handler_params_known: true,
            };
            (id, rec)
        };

        let (rid_a, rec_a) = make_rec(&uri_a, 0, path_a);
        let (rid_b, rec_b) = make_rec(&uri_b, 1, path_b);

        let mut linked = Linked::default();
        linked.route_index.insert(rid_a, vec![rec_a]);
        linked.route_index.insert(rid_b, vec![rec_b]);

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ));

        let mut facts_a = FileFacts::new(uri_a.clone());
        if let Some(n) = url_for_name {
            facts_a.url_for_sites.push(UrlForSite {
                name: n.to_owned(),
                kwarg_names: vec![],
                has_splat_kwargs: false,
                range: Range::default(),
                name_range: None,
            });
        }
        state.file_facts.insert(uri_a.clone(), facts_a);
        state
            .file_facts
            .insert(uri_b.clone(), FileFacts::new(uri_b));
        state.linked.store(Arc::new(linked));

        (state, uri_a)
    }

    #[test]
    fn route_duplicate_name_suppressed_when_not_used_in_url_for() {
        let (state, uri) = make_dup_name_state(
            "index_view",
            "/projects/list",
            "/companies/list",
            None, // no url_for call anywhere
        );
        let diags = compute(&state, &uri, &[]);
        let dup_name: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("route/duplicate-name".to_owned())))
            .collect();
        assert!(
            dup_name.is_empty(),
            "route/duplicate-name must not fire when name is never used in url_for; got {:?}",
            dup_name,
        );
    }

    #[test]
    fn route_duplicate_name_fires_when_name_used_in_url_for() {
        let (state, uri) = make_dup_name_state(
            "index_view",
            "/projects/list",
            "/companies/list",
            Some("index_view"), // url_for('index_view') present
        );
        let diags = compute(&state, &uri, &[]);
        let dup_name: Vec<_> = diags
            .iter()
            .filter(|d| d.code == Some(NumberOrString::String("route/duplicate-name".to_owned())))
            .collect();
        assert!(
            !dup_name.is_empty(),
            "route/duplicate-name must fire when name is used in url_for",
        );
    }

    // ── Issue 2: param-missing-arg with Depends() in handler signature ────────

    fn make_state_with_dep(
        path: &str,
        handler_params: Vec<String>,
        dep_in_signature: Option<&str>, // dep fn name used in handler signature Depends()
        dep_params: Vec<String>,        // params that dep fn accepts
        nested_dep: Option<(&str, Vec<String>)>, // (nested_dep_name, its_params)
    ) -> (std::sync::Arc<crate::state::WorkspaceState>, Uri) {
        use crate::config::ResolvedConfig;
        use crate::state::AnnotatedParam;
        use crate::state::{
            FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId,
            RouteRecord,
        };
        use std::sync::Arc;

        let uri: Uri = "file:///router.py".parse().unwrap();
        let path_params = crate::util::extract_path_params(path);
        let id = RouteId(format!("app.handler:{path}:GET"));
        let rec = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "handler".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved(path.to_owned()),
            decorator_path: path.to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range::default(),
            },
            path_params,
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: None,
            path_quote_width: None,
            handler_params,
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        };

        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![rec]);
        if let Some(dep_name) = dep_in_signature {
            linked
                .dep_params
                .insert(dep_name.to_owned(), dep_params.clone());
        }
        if let Some((nested_name, nested_params)) = &nested_dep {
            linked
                .dep_params
                .insert(nested_name.to_string(), nested_params.clone());
        }

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));

        let mut facts = FileFacts::new(uri.clone());
        if let Some(dep_name) = dep_in_signature.as_ref().copied() {
            // Simulate: handler signature has `obj = Depends(dep_name)`
            facts.annotated_params.push(AnnotatedParam {
                containing_func: "handler".to_owned(),
                param_name: "obj".to_owned(),
                is_annotated: false,
                annotation_range: Range::default(),
                default_range: Some(Range::default()),
                type_text: "SomeType".to_owned(),
                depends_text: format!("Depends({dep_name})"),
                has_extra_args: false,
            });
            // If there's a nested dep, simulate that dep_name's signature also has a Depends
            if let Some((nested_name, _)) = &nested_dep {
                facts.annotated_params.push(AnnotatedParam {
                    containing_func: dep_name.to_owned(),
                    param_name: "inner".to_owned(),
                    is_annotated: false,
                    annotation_range: Range::default(),
                    default_range: Some(Range::default()),
                    type_text: "InnerType".to_owned(),
                    depends_text: format!("Depends({nested_name})"),
                    has_extra_args: false,
                });
            }
        }
        state.file_facts.insert(uri.clone(), facts);
        state.linked.store(Arc::new(linked));

        (state, uri)
    }

    #[test]
    fn param_not_unbound_when_dep_in_handler_signature_consumes_it() {
        // @router.get("/{id}")
        // async def handler(obj: Project = Depends(get_project)):
        //     pass
        // get_project(id: int) → dep_params["get_project"] = ["id"]
        let (state, uri) = make_state_with_dep(
            "/{id}",
            vec!["obj".to_owned()],
            Some("get_project"),
            vec!["id".to_owned()],
            None,
        );
        let diags = compute(&state, &uri, &[]);
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert!(
            missing.is_empty(),
            "param-missing-arg must not fire when dep in handler signature consumes the path param; got {:?}",
            missing,
        );
    }

    #[test]
    fn param_not_unbound_when_dep_is_nested_two_levels() {
        // @router.get("/{id}")
        // async def handler(obj = Depends(check_auth)):
        //     pass
        // check_auth has inner = Depends(get_project)
        // get_project(id: int)
        let (state, uri) = make_state_with_dep(
            "/{id}",
            vec!["obj".to_owned()],
            Some("check_auth"),
            vec!["inner".to_owned()], // check_auth's direct params
            Some(("get_project", vec!["id".to_owned()])),
        );
        let diags = compute(&state, &uri, &[]);
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert!(
            missing.is_empty(),
            "param-missing-arg must not fire when nested dep consumes the path param; got {:?}",
            missing,
        );
    }

    #[test]
    fn param_bound_by_type_alias_dep_suppresses_diag() {
        // Models the CurrentProject pattern:
        //   CurrentProject = Annotated[Project, Depends(fetch_project)]
        //   async def view_project(project: CurrentProject): ...  (path /{id:uuid})
        // fetch_project(id: uuid.UUID) consumes the path param — no diagnostic expected.
        let state = WorkspaceState::new(crate::config::ResolvedConfig::default_for_root(
            std::path::PathBuf::from("."),
        ));
        let uri: Uri = "file:///router.py".parse().unwrap();
        let mut facts = FileFacts::new(uri.clone());
        facts
            .dep_type_aliases
            .insert("CurrentProject".to_owned(), "fetch_project".to_owned());
        facts
            .plain_typed_params
            .push(crate::state::PlainTypedParam {
                containing_func: "handler".to_owned(),
                param_name: "project".to_owned(),
                type_name: "CurrentProject".to_owned(),
                annotation_range: Range::default(),
            });
        state.file_facts.insert(uri.clone(), facts);

        let path_params = vec![PathParam {
            name: "id".to_owned(),
            converter: PathConverter::Uuid,
        }];
        let (route_id, record) = make_route_record_with_params(
            &uri,
            "/projects/{id:uuid}",
            path_params,
            vec!["project".to_owned()],
            false,
            true,
        );
        let mut linked = Linked::default();
        linked.route_index.insert(route_id, vec![record]);
        linked
            .dep_params
            .insert("fetch_project".to_owned(), vec!["id".to_owned()]);
        state.linked.store(Arc::new(linked));

        let diags = compute(&state, &uri, &[]);
        let missing: Vec<_> = diags
            .iter()
            .filter(|d| {
                d.code == Some(NumberOrString::String("route/param-missing-arg".to_owned()))
            })
            .collect();
        assert!(
            missing.is_empty(),
            "param-missing-arg must not fire when path param is consumed via type alias dep; got {:?}",
            missing,
        );
    }

    // ── test/unknown-path ────────────────────────────────────────────────────

    #[test]
    fn test_unknown_path_diag_properties() {
        use tower_lsp_server::ls_types::Position;
        let range = Range {
            start: Position::new(5, 10),
            end: Position::new(5, 30),
        };
        let d = test_unknown_path_diag("/api/missing", range);
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("test/unknown-path".to_owned()))
        );
        assert!(d.message.contains("/api/missing"));
        assert_eq!(d.source, Some("fastapi-lsp".to_owned()));
    }
}
