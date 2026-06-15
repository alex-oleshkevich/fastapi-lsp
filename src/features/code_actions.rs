use std::path::PathBuf;

use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Command,
    OptionalVersionedTextDocumentIdentifier, Position, Range, TextDocumentEdit, TextEdit,
    WorkspaceEdit,
};

use crate::features::diagnostics::{
    arg_missing_param_diag, depends_called_diag, edit_distance, handler_param_range,
    is_env_key_ignored, is_param_segment, literal_route_shadowed_by, param_missing_arg_diag,
    param_segment_range, route_shadowed_diag, undefined_key_diag, unknown_response_model_diag,
};
use crate::state::ResolvedPath;
use crate::state::WorkspaceState;
use crate::uri::{path_to_uri, uri_to_path};
use crate::util::position_in_range;

/// Sentinel line number used for "append to end of file" text edits and
/// the matching showDocument selection. Editors clamp past-end positions.
const APPEND_LINE: u32 = u32::MAX / 2;

pub fn code_actions(
    state: &WorkspaceState,
    params: &CodeActionParams,
    workspace_root: &PathBuf,
    env_ignore: &[String],
    show_document_supported: bool,
) -> Vec<CodeActionOrCommand> {
    let uri = &params.text_document.uri;
    let range = params.range;
    let facts = match state.file_facts.get(uri) {
        Some(f) => f,
        None => return vec![],
    };
    let linked = state.linked.load();

    let dot_env_uri = path_to_uri(&workspace_root.join(".env"));
    let dot_env_example_uri = path_to_uri(&workspace_root.join(".env.example"));

    let mut actions: Vec<CodeActionOrCommand> = vec![];

    // di/depends-called: offer "Remove call" when cursor overlaps the Depends() range
    let proven_deps = &linked.proven_dep_names;
    // dep names with actual function definitions anywhere in the workspace
    let defined_deps: std::collections::HashSet<String> = state
        .file_facts
        .iter()
        .flat_map(|entry| {
            entry
                .dep_defs
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>()
        })
        .collect();
    for dep_ref in &facts.dep_refs {
        if !dep_ref.is_called {
            continue;
        }
        if !proven_deps.contains(dep_ref.name.as_str()) {
            continue;
        }
        if !position_in_range(range.start, dep_ref.range.start, dep_ref.range.end)
            && !position_in_range(range.end, dep_ref.range.start, dep_ref.range.end)
        {
            continue;
        }
        let callee_range = match dep_ref.callee_range {
            Some(r) => r,
            None => continue,
        };
        let diag = depends_called_diag(dep_ref.range);
        let action = CodeAction {
            title: format!("Remove call — pass `{}` itself", dep_ref.name),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diag]),
            edit: Some(WorkspaceEdit {
                document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                    TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: uri.clone(),
                            version: None,
                        },
                        edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                            range: callee_range,
                            new_text: dep_ref.name.clone(),
                        })],
                    },
                ])),
                ..Default::default()
            }),
            is_preferred: Some(true),
            ..Default::default()
        };
        actions.push(CodeActionOrCommand::CodeAction(action));
    }

    for site in &facts.env_lookups {
        if site.has_default {
            continue;
        }
        // Only offer actions when the cursor overlaps the key range.
        if !position_in_range(range.start, site.key_range.start, site.key_range.end)
            && !position_in_range(range.end, site.key_range.start, site.key_range.end)
        {
            continue;
        }

        let key = &site.key;
        let key_upper = key.to_uppercase();
        let in_index =
            linked.env_index.contains_key(&key_upper) || linked.env_index.contains_key(key);

        if in_index {
            continue;
        }
        if is_env_key_ignored(key, env_ignore) {
            continue;
        }

        if let Some(ref env_uri) = dot_env_uri {
            // Action: Add KEY to .env
            {
                let diag = undefined_key_diag(key, site.key_range);
                let open_cmd = show_document_supported.then(|| Command {
                    title: "Open .env at appended line".to_owned(),
                    command: "fastapi-lsp.openFileAt".to_owned(),
                    arguments: Some(vec![
                        serde_json::Value::String(env_uri.to_string()),
                        serde_json::Value::Number(serde_json::Number::from(APPEND_LINE)),
                    ]),
                });
                let action = CodeAction {
                    title: format!("Add '{key}' to .env"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag]),
                    edit: Some(WorkspaceEdit {
                        document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                            vec![TextDocumentEdit {
                                text_document: OptionalVersionedTextDocumentIdentifier {
                                    uri: env_uri.clone(),
                                    version: None,
                                },
                                edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                                    range: Range {
                                        start: Position::new(APPEND_LINE, 0),
                                        end: Position::new(APPEND_LINE, 0),
                                    },
                                    new_text: format!("{key_upper}=\n"),
                                })],
                            }],
                        )),
                        ..Default::default()
                    }),
                    command: open_cmd,
                    is_preferred: Some(true),
                    ..Default::default()
                };
                actions.push(CodeActionOrCommand::CodeAction(action));
            }

            // Action: Copy KEY from .env.example (only when key exists there)
            if let Some(example_uri) = &dot_env_example_uri {
                let example_value = state.env_file_entries.get(example_uri).and_then(|entries| {
                    entries
                        .iter()
                        .find(|e| e.key.to_uppercase() == key_upper)
                        .map(|e| format!("{}={}", e.key, e.value))
                });

                if let Some(raw_line) = example_value {
                    let action = CodeAction {
                        title: format!("Copy '{key}' from .env.example to .env"),
                        kind: Some(CodeActionKind::QUICKFIX),
                        edit: Some(WorkspaceEdit {
                            document_changes: Some(
                                tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                                    TextDocumentEdit {
                                        text_document: OptionalVersionedTextDocumentIdentifier {
                                            uri: env_uri.clone(),
                                            version: None,
                                        },
                                        edits: vec![tower_lsp_server::ls_types::OneOf::Left(
                                            TextEdit {
                                                range: Range {
                                                    start: Position::new(APPEND_LINE, 0),
                                                    end: Position::new(APPEND_LINE, 0),
                                                },
                                                new_text: format!("{}\n", raw_line.trim_end()),
                                            },
                                        )],
                                    },
                                ]),
                            ),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }
        }
    }

    // model/unknown-response-model: "Import `Name` from `module`" (single-candidate)
    for record in linked
        .route_index
        .values()
        .flat_map(|v| v.iter())
        .filter(|r| &r.handler.uri == uri)
    {
        let Some(ref model_name) = record.response_model else {
            continue;
        };
        let bare_name = model_name.rsplit('.').next().unwrap_or(model_name.as_str());

        // Only offer when there is exactly one candidate in the workspace index
        let candidates = match linked.model_index.get(bare_name) {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };
        if candidates.len() != 1 {
            continue;
        }

        // The symbol must NOT already be resolvable (else no diagnostic exists)
        let has_wildcard = facts.imported_names.contains(&"*".to_owned());
        let already_imported = facts.imported_names.iter().any(|n| n == bare_name);
        if has_wildcard || already_imported {
            continue;
        }

        let model_uri = &candidates[0].location.uri;
        let Some(module_path) = uri_to_module_path(model_uri, workspace_root) else {
            continue;
        };

        // Only offer if the cursor overlaps the response_model range
        let rm_range = record.response_model_range.unwrap_or(record.handler.range);
        if !position_in_range(range.start, rm_range.start, rm_range.end)
            && !position_in_range(range.end, rm_range.start, rm_range.end)
        {
            continue;
        }

        let import_text = format!("from {module_path} import {bare_name}\n");
        let insert_line = state
            .file_sources
            .get(uri)
            .map(|s| import_insert_line(s.as_str()))
            .unwrap_or(0);
        let insert_range = Range {
            start: Position::new(insert_line, 0),
            end: Position::new(insert_line, 0),
        };
        let diag = unknown_response_model_diag(bare_name, rm_range);
        let action = CodeAction {
            title: format!("Import `{bare_name}` from `{module_path}`"),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diag]),
            edit: Some(WorkspaceEdit {
                document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                    TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: uri.clone(),
                            version: None,
                        },
                        edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                            range: insert_range,
                            new_text: import_text,
                        })],
                    },
                ])),
                ..Default::default()
            }),
            is_preferred: Some(true),
            ..Default::default()
        };
        actions.push(CodeActionOrCommand::CodeAction(action));
    }

    // route/param-missing-arg: "Add parameter `X: str`"
    // route/arg-missing-param: "Rename parameter to `X`"
    {
        let mut dep_params: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for fe in state.file_facts.iter() {
            for d in &fe.dep_defs {
                dep_params
                    .entry(d.name.clone())
                    .or_insert_with(|| d.param_names.clone());
            }
        }

        for record in linked
            .route_index
            .values()
            .flat_map(|v| v.iter())
            .filter(|r| &r.handler.uri == uri)
            .filter(|r| matches!(r.resolved_path, ResolvedPath::Resolved(_)))
        {
            if !record.handler_params_known || record.handler_has_splat_args {
                continue;
            }
            if record.path_params.is_empty() {
                continue;
            }

            let mut bound: std::collections::HashSet<String> =
                record.handler_params.iter().cloned().collect();
            for dep_name in &record.dependencies {
                if let Some(params) = dep_params.get(dep_name) {
                    bound.extend(params.iter().cloned());
                }
            }

            // route/param-missing-arg: offer "Add parameter `X: str`"
            if let Some(insert_pos) = record.params_insert_pos {
                for path_param in &record.path_params {
                    if bound.contains(&path_param.name) {
                        continue;
                    }
                    let seg_range = param_segment_range(record, &path_param.name);
                    if !position_in_range(range.start, seg_range.start, seg_range.end)
                        && !position_in_range(range.end, seg_range.start, seg_range.end)
                    {
                        continue;
                    }
                    let new_text = if record.handler_params.is_empty() {
                        format!("{}: str", path_param.name)
                    } else {
                        format!(", {}: str", path_param.name)
                    };
                    let insert_range = Range {
                        start: insert_pos,
                        end: insert_pos,
                    };
                    let diag = param_missing_arg_diag(&path_param.name, seg_range);
                    let action = CodeAction {
                        title: format!("Add parameter `{}: str`", path_param.name),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag]),
                        edit: Some(WorkspaceEdit {
                            document_changes: Some(
                                tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                                    TextDocumentEdit {
                                        text_document: OptionalVersionedTextDocumentIdentifier {
                                            uri: uri.clone(),
                                            version: None,
                                        },
                                        edits: vec![tower_lsp_server::ls_types::OneOf::Left(
                                            TextEdit {
                                                range: insert_range,
                                                new_text,
                                            },
                                        )],
                                    },
                                ]),
                            ),
                            ..Default::default()
                        }),
                        is_preferred: Some(true),
                        ..Default::default()
                    };
                    actions.push(CodeActionOrCommand::CodeAction(action));
                }
            }

            // route/arg-missing-param: offer "Rename parameter to `X`" (gate: exactly one unbound path param)
            let unbound_path_params: Vec<&str> = record
                .path_params
                .iter()
                .filter(|p| !bound.contains(&p.name))
                .map(|p| p.name.as_str())
                .collect();
            if unbound_path_params.len() != 1 {
                continue;
            }
            let target_param = unbound_path_params[0];

            // Guard: mismatched ranges would produce a destructive edit to the wrong span.
            if record.handler_param_ranges.len() != record.handler_params.len() {
                continue;
            }

            let dep_contributed: std::collections::HashSet<&str> = record
                .dependencies
                .iter()
                .flat_map(|dep_name| {
                    dep_params
                        .get(dep_name)
                        .into_iter()
                        .flat_map(|v| v.iter().map(|s| s.as_str()))
                })
                .collect();
            let path_param_names: std::collections::HashSet<&str> =
                record.path_params.iter().map(|p| p.name.as_str()).collect();

            for (idx, handler_param) in record.handler_params.iter().enumerate() {
                if path_param_names.contains(handler_param.as_str())
                    || dep_contributed.contains(handler_param.as_str())
                {
                    continue;
                }
                if edit_distance(handler_param, target_param) > 2 {
                    continue;
                }
                let hp_range = record
                    .handler_param_ranges
                    .get(idx)
                    .copied()
                    .unwrap_or_else(|| handler_param_range(record, handler_param));
                if !position_in_range(range.start, hp_range.start, hp_range.end)
                    && !position_in_range(range.end, hp_range.start, hp_range.end)
                {
                    continue;
                }
                let diag = arg_missing_param_diag(handler_param, target_param, hp_range);
                // Fix 1: rename handler param to match path param
                let rename_action = CodeAction {
                    title: format!("Rename parameter to `{target_param}`"),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag.clone()]),
                    edit: Some(WorkspaceEdit {
                        document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                            vec![TextDocumentEdit {
                                text_document: OptionalVersionedTextDocumentIdentifier {
                                    uri: uri.clone(),
                                    version: None,
                                },
                                edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                                    range: hp_range,
                                    new_text: target_param.to_owned(),
                                })],
                            }],
                        )),
                        ..Default::default()
                    }),
                    is_preferred: Some(true),
                    ..Default::default()
                };
                actions.push(CodeActionOrCommand::CodeAction(rename_action));

                // Fix 2: add `/{handler_param}` segment to path (gate: literal path string)
                if let Some(pr) = record.path_range {
                    let insert_col = pr.end.character.saturating_sub(1);
                    let insert_pos = Position::new(pr.end.line, insert_col);
                    let insert_range = Range {
                        start: insert_pos,
                        end: insert_pos,
                    };
                    let seg_action = CodeAction {
                        title: format!("Add `/{{{}}}` segment to path", handler_param),
                        kind: Some(CodeActionKind::QUICKFIX),
                        diagnostics: Some(vec![diag]),
                        edit: Some(WorkspaceEdit {
                            document_changes: Some(
                                tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                                    TextDocumentEdit {
                                        text_document: OptionalVersionedTextDocumentIdentifier {
                                            uri: uri.clone(),
                                            version: None,
                                        },
                                        edits: vec![tower_lsp_server::ls_types::OneOf::Left(
                                            TextEdit {
                                                range: insert_range,
                                                new_text: format!("/{{{handler_param}}}"),
                                            },
                                        )],
                                    },
                                ]),
                            ),
                            ..Default::default()
                        }),
                        ..Default::default()
                    };
                    actions.push(CodeActionOrCommand::CodeAction(seg_action));
                }
                break;
            }
        }
    }

    // route/shadowed: "Move route above `{handler_name}`"
    // Only offered when both handlers are in the same file and file source is available.
    if let Some(source) = state.file_sources.get(uri) {
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let all_records: Vec<&crate::state::RouteRecord> =
            linked.route_index.values().flat_map(|v| v.iter()).collect();
        let file_records: Vec<&crate::state::RouteRecord> = all_records
            .iter()
            .copied()
            .filter(|r| &r.handler.uri == uri)
            .filter(|r| matches!(r.resolved_path, ResolvedPath::Resolved(_)))
            .collect();

        for record in &file_records {
            let rec_path = match &record.resolved_path {
                ResolvedPath::Resolved(p) => p.as_str(),
                ResolvedPath::Unresolved => continue,
            };
            // Only fire on literal-path (no param segments) routes
            if rec_path.split('/').any(is_param_segment) {
                continue;
            }
            if !position_in_range(
                range.start,
                record.handler.range.start,
                record.handler.range.end,
            ) && !position_in_range(
                range.end,
                record.handler.range.start,
                record.handler.range.end,
            ) {
                continue;
            }
            // Find the shadowing route (same method, lower ordinal, same file, shadows this literal path)
            let shadower = all_records.iter().copied().find(|other| {
                other.ordinal < record.ordinal
                    && other.method == record.method
                    && other.handler.uri == record.handler.uri
                    && matches!(&other.resolved_path, ResolvedPath::Resolved(p) if
                        literal_route_shadowed_by(rec_path, p.as_str()))
            });
            let Some(shadower) = shadower else { continue };

            let shadowed_block = extract_handler_block(&lines, record.handler.range);
            let shadower_block = extract_handler_block(&lines, shadower.handler.range);
            let (Some((sb_start, sb_end, sb_text)), Some((shb_start, shb_end, shb_text))) =
                (shadowed_block, shadower_block)
            else {
                continue;
            };

            // Swap by replacing in reverse source order (second block first, then first block).
            let (earlier_start, earlier_end, earlier_new, later_start, later_end, later_new) =
                if shb_start < sb_start {
                    (shb_start, shb_end, sb_text, sb_start, sb_end, shb_text)
                } else {
                    (sb_start, sb_end, shb_text, shb_start, shb_end, sb_text)
                };
            let earlier_range = Range {
                start: Position::new(earlier_start as u32, 0),
                end: earlier_end,
            };
            let later_range = Range {
                start: Position::new(later_start as u32, 0),
                end: later_end,
            };

            let shadower_path = match &shadower.resolved_path {
                ResolvedPath::Resolved(p) => p.as_str(),
                ResolvedPath::Unresolved => continue,
            };
            let diag = route_shadowed_diag(
                record.handler.range,
                rec_path,
                shadower_path,
                tower_lsp_server::ls_types::Location {
                    uri: shadower.handler.uri.clone(),
                    range: shadower.handler.range,
                },
            );
            let action = CodeAction {
                title: format!("Move route above `{}`", shadower.name),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag]),
                edit: Some(WorkspaceEdit {
                    document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                        vec![TextDocumentEdit {
                            text_document: OptionalVersionedTextDocumentIdentifier {
                                uri: uri.clone(),
                                version: None,
                            },
                            // Apply later edit first, then earlier — LSP applies edits in array order,
                            // so we put the one with higher line numbers first to avoid offset shifts.
                            edits: vec![
                                tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                                    range: later_range,
                                    new_text: later_new,
                                }),
                                tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                                    range: earlier_range,
                                    new_text: earlier_new,
                                }),
                            ],
                        }],
                    )),
                    ..Default::default()
                }),
                is_preferred: Some(true),
                ..Default::default()
            };
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
    }

    // convert/annotated: "Convert to Annotated style" / "Convert to inline style"
    // Offered for any parameter using Depends(...) in either style. Purely syntactic — always safe.
    for ap in &facts.annotated_params {
        let (action_range_start, action_range_end) = if ap.is_annotated {
            (ap.annotation_range.start, ap.annotation_range.end)
        } else {
            // Inline style: the editable span covers type annotation + `= Depends(...)`
            let default_end = ap
                .default_range
                .map(|r| r.end)
                .unwrap_or(ap.annotation_range.end);
            (ap.annotation_range.start, default_end)
        };
        if !position_in_range(range.start, action_range_start, action_range_end)
            && !position_in_range(range.end, action_range_start, action_range_end)
        {
            continue;
        }

        if ap.is_annotated {
            // Annotated → Inline: only safe when Annotated has no extra args beyond [T, Depends(fn)]
            if ap.has_extra_args {
                continue;
            }
            // Replace Annotated[T, Depends(fn)] with T = Depends(fn)
            let action = CodeAction {
                title: format!("Convert `{}` to inline style", ap.param_name),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                edit: Some(WorkspaceEdit {
                    document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                        vec![TextDocumentEdit {
                            text_document: OptionalVersionedTextDocumentIdentifier {
                                uri: uri.clone(),
                                version: None,
                            },
                            edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                                range: ap.annotation_range,
                                new_text: format!("{} = {}", ap.type_text, ap.depends_text),
                            })],
                        }],
                    )),
                    ..Default::default()
                }),
                ..Default::default()
            };
            actions.push(CodeActionOrCommand::CodeAction(action));
        } else if let Some(default_range) = ap.default_range {
            // Inline → Annotated: replace `T = Depends(fn)` with `Annotated[T, Depends(fn)]`
            // Also add `from typing import Annotated` if not already imported.
            let combined_range = Range {
                start: ap.annotation_range.start,
                end: default_range.end,
            };
            let needs_annotated_import = !facts
                .imported_names
                .iter()
                .any(|n| n == "Annotated" || n == "*");
            let mut edits = vec![];
            if needs_annotated_import {
                let ins_line = state
                    .file_sources
                    .get(uri)
                    .map(|s| import_insert_line(s.as_str()))
                    .unwrap_or(0);
                edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                    range: Range {
                        start: Position::new(ins_line, 0),
                        end: Position::new(ins_line, 0),
                    },
                    new_text: "from typing import Annotated\n".to_owned(),
                }));
            }
            edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                range: combined_range,
                new_text: format!("Annotated[{}, {}]", ap.type_text, ap.depends_text),
            }));
            let action = CodeAction {
                title: format!("Convert `{}` to Annotated style", ap.param_name),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                edit: Some(WorkspaceEdit {
                    document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                        vec![TextDocumentEdit {
                            text_document: OptionalVersionedTextDocumentIdentifier {
                                uri: uri.clone(),
                                version: None,
                            },
                            edits,
                        }],
                    )),
                    ..Default::default()
                }),
                ..Default::default()
            };
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
    }

    // extract/dependency: "Extract to named dependency `{T}Dep`"
    // Offered for cursor on is_annotated=true params. Inserts a module-level alias and rewrites
    // all textually identical Annotated[T, Depends(fn)] annotations in the file (same-file),
    // or across the workspace placing the alias in deps.py/dependencies.py (workspace variant).
    if let Some(source) = state.file_sources.get(uri).map(|s| s.clone()) {
        for ap in &facts.annotated_params {
            if !ap.is_annotated {
                continue;
            }
            if !position_in_range(
                range.start,
                ap.annotation_range.start,
                ap.annotation_range.end,
            ) && !position_in_range(
                range.end,
                ap.annotation_range.start,
                ap.annotation_range.end,
            ) {
                continue;
            }
            // Gate: dep name is actually defined somewhere in the workspace.
            // Extract fn name from "Depends(fn)" or "fastapi.Depends(fn)" by splitting on "(".
            let dep_name_in_depends = ap
                .depends_text
                .split_once('(')
                .map(|x| x.1)
                .and_then(|s| s.strip_suffix(')'))
                .unwrap_or("");
            if dep_name_in_depends.is_empty() || !defined_deps.contains(dep_name_in_depends) {
                continue;
            }
            let alias_name = format!("{}Dep", ap.type_text);
            let canonical = format!("Annotated[{}, {}]", ap.type_text, ap.depends_text);

            // Gate: proposed alias name must not already be bound in this file.
            // Check top-level assignment, type annotation, or import.
            let alias_bound = source.lines().any(|line| {
                line.starts_with(&format!("{alias_name} ="))
                    || line.starts_with(&format!("{alias_name}: "))
                    || line.contains(&format!("import {alias_name}"))
            });
            if alias_bound {
                continue;
            }

            // Find all matching annotated params in this file
            let file_matches: Vec<&crate::state::AnnotatedParam> = facts
                .annotated_params
                .iter()
                .filter(|p| {
                    p.is_annotated
                        && p.type_text == ap.type_text
                        && p.depends_text == ap.depends_text
                })
                .collect();

            // Find insertion point: first decorator/def line of the first handler using this annotation
            let first_line = file_matches
                .iter()
                .map(|p| p.annotation_range.start.line)
                .min()
                .unwrap_or(0);
            let first_func = file_matches
                .iter()
                .find(|p| p.annotation_range.start.line == first_line)
                .map(|p| p.containing_func.as_str());
            let handler_def_line = first_func
                .and_then(|name| facts.routes.iter().find(|rf| rf.handler_name == name))
                .map(|rf| rf.handler_range.start.line)
                .unwrap_or(first_line);
            let src_lines: Vec<&str> = source.lines().collect();
            let insert_line = (0..handler_def_line as usize)
                .rev()
                .find(|&i| {
                    src_lines
                        .get(i)
                        .is_some_and(|l| l.trim_start().starts_with('@'))
                })
                .unwrap_or(handler_def_line as usize);

            // Build replacement edits (highest line first)
            let mut repl_edits: Vec<tower_lsp_server::ls_types::OneOf<TextEdit, _>> = file_matches
                .iter()
                .map(|p| {
                    tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                        range: p.annotation_range,
                        new_text: alias_name.clone(),
                    })
                })
                .collect();
            repl_edits.sort_by(|a, b| {
                let la = if let tower_lsp_server::ls_types::OneOf::Left(te) = a {
                    te.range.start.line
                } else {
                    0
                };
                let lb = if let tower_lsp_server::ls_types::OneOf::Left(te) = b {
                    te.range.start.line
                } else {
                    0
                };
                lb.cmp(&la)
            });

            // Alias insert goes at the start of insert_line (text editors apply highest-line first,
            // so the insert at insert_line won't shift the replacement lines above it)
            let alias_text = format!("{alias_name} = {canonical}\n\n");
            let alias_edit = tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                range: Range {
                    start: Position::new(insert_line as u32, 0),
                    end: Position::new(insert_line as u32, 0),
                },
                new_text: alias_text,
            });
            let mut all_edits = repl_edits;
            all_edits.push(alias_edit);

            let same_file_action = CodeAction {
                title: format!("Extract to named dependency `{alias_name}`"),
                kind: Some(CodeActionKind::REFACTOR_EXTRACT),
                edit: Some(WorkspaceEdit {
                    document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                        vec![TextDocumentEdit {
                            text_document: OptionalVersionedTextDocumentIdentifier {
                                uri: uri.clone(),
                                version: None,
                            },
                            edits: all_edits,
                        }],
                    )),
                    ..Default::default()
                }),
                ..Default::default()
            };
            actions.push(CodeActionOrCommand::CodeAction(same_file_action));

            // Workspace variant: find all files with matching annotation text + place alias in deps.py
            // Use `canonical` for text matching across files
            let mut workspace_doc_edits: Vec<TextDocumentEdit> = vec![];

            // Determine alias target file (deps.py/dependencies.py in same dir, or current file)
            let alias_target_uri = find_dependency_file(uri, state).unwrap_or_else(|| uri.clone());
            let alias_in_external = alias_target_uri != *uri;

            // Collect per-file annotation replacements across the workspace
            let mut alias_file_uris: Vec<tower_lsp_server::ls_types::Uri> = vec![];
            let mut sorted_file_keys: Vec<tower_lsp_server::ls_types::Uri> =
                state.file_facts.iter().map(|e| e.key().clone()).collect();
            sorted_file_keys.sort_by(|a, b| a.as_str().cmp(b.as_str()));

            for file_uri in &sorted_file_keys {
                let Some(file_source) = state.file_sources.get(file_uri).map(|s| s.clone()) else {
                    continue;
                };
                let Some(file_facts) = state.file_facts.get(file_uri) else {
                    continue;
                };

                let ws_matches: Vec<&crate::state::AnnotatedParam> = file_facts
                    .annotated_params
                    .iter()
                    .filter(|p| {
                        p.is_annotated
                            && p.type_text == ap.type_text
                            && p.depends_text == ap.depends_text
                    })
                    .collect();
                if ws_matches.is_empty() {
                    continue;
                }
                if !file_source.contains(&canonical) {
                    continue;
                }

                alias_file_uris.push(file_uri.clone());

                let mut file_repl: Vec<tower_lsp_server::ls_types::OneOf<TextEdit, _>> = ws_matches
                    .iter()
                    .map(|p| {
                        tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                            range: p.annotation_range,
                            new_text: alias_name.clone(),
                        })
                    })
                    .collect();
                file_repl.sort_by(|a, b| {
                    let la = if let tower_lsp_server::ls_types::OneOf::Left(te) = a {
                        te.range.start.line
                    } else {
                        0
                    };
                    let lb = if let tower_lsp_server::ls_types::OneOf::Left(te) = b {
                        te.range.start.line
                    } else {
                        0
                    };
                    lb.cmp(&la)
                });

                // Add import when the alias lives in an external file
                if alias_in_external
                    && file_uri != &alias_target_uri
                    && let Some(module_path) = uri_to_module_path(&alias_target_uri, workspace_root)
                {
                    let import_text = format!("from {module_path} import {alias_name}\n");
                    file_repl.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                        range: Range {
                            start: Position::new(0, 0),
                            end: Position::new(0, 0),
                        },
                        new_text: import_text,
                    }));
                }
                workspace_doc_edits.push(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: file_uri.clone(),
                        version: None,
                    },
                    edits: file_repl,
                });
            }

            // Alias definition goes into alias_target_uri
            if alias_in_external {
                // Append alias definition to the external deps file
                workspace_doc_edits.push(TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: alias_target_uri.clone(),
                        version: None,
                    },
                    edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                        range: Range {
                            start: Position::new(APPEND_LINE, 0),
                            end: Position::new(APPEND_LINE, 0),
                        },
                        new_text: format!("\n{alias_name} = {canonical}\n"),
                    })],
                });
            } else {
                // Insert alias into the current file (uri), not whichever file sorted first
                if let Some(doc) = workspace_doc_edits
                    .iter_mut()
                    .find(|d| &d.text_document.uri == uri)
                {
                    doc.edits
                        .push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                            range: Range {
                                start: Position::new(insert_line as u32, 0),
                                end: Position::new(insert_line as u32, 0),
                            },
                            new_text: format!("{alias_name} = {canonical}\n\n"),
                        }));
                }
            }

            if !workspace_doc_edits.is_empty() {
                let workspace_action = CodeAction {
                    title: format!("Extract to named dependency `{alias_name}` (workspace)"),
                    kind: Some(CodeActionKind::REFACTOR_EXTRACT),
                    edit: Some(WorkspaceEdit {
                        document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                            workspace_doc_edits,
                        )),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                actions.push(CodeActionOrCommand::CodeAction(workspace_action));
            }
        }
    }

    // model/create: "Create model `Name`" — response_model is CamelCase, not in model_index,
    // not imported. Imports-first target: a workspace module already imported by this file that
    // already contains Pydantic models. Fallback: schemas.py in the same package directory.
    for record in linked
        .route_index
        .values()
        .flat_map(|v| v.iter())
        .filter(|r| &r.handler.uri == uri)
    {
        let Some(ref model_name) = record.response_model else {
            continue;
        };
        let bare_name = model_name.rsplit('.').next().unwrap_or(model_name.as_str());

        if !is_camel_case(bare_name) {
            continue;
        }
        // Candidates exist → the "import" action above handles it
        if linked.model_index.contains_key(bare_name) {
            continue;
        }
        if facts.imported_names.iter().any(|n| n == bare_name)
            || facts.imported_names.contains(&"*".to_owned())
        {
            continue;
        }

        let rm_range = record.response_model_range.unwrap_or(record.handler.range);
        if !position_in_range(range.start, rm_range.start, rm_range.end)
            && !position_in_range(range.end, rm_range.start, rm_range.end)
        {
            continue;
        }

        let Some(target_uri) =
            resolve_create_model_target(&facts.imported_from, uri, workspace_root, state)
        else {
            continue;
        };

        // Use file_sources (not file_facts) as the single source of truth: only "existing" when
        // we also have the source text, so build_create_model_action always has content to diff.
        let target_exists = state.file_sources.contains_key(&target_uri);

        // Creating a new file requires the client to advertise ResourceOperationKind::Create
        if !target_exists
            && !state
                .can_create_files
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            continue;
        }

        if let Some(action) =
            build_create_model_action(bare_name, &target_uri, workspace_root, state, target_exists)
        {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
    }

    // create/missing-dependency: "Create dependency `{name}`"
    // Gate: dep name has no dep_def in the workspace (i.e., no function is defined for it).
    // proven_dependency_names includes dep_refs, so we use dep_defs exclusively here.
    for dep_ref in &facts.dep_refs {
        if !position_in_range(range.start, dep_ref.range.start, dep_ref.range.end)
            && !position_in_range(range.end, dep_ref.range.start, dep_ref.range.end)
        {
            continue;
        }
        if defined_deps.contains(dep_ref.name.as_str()) {
            continue;
        }
        if let Some(action) = build_create_dependency_action(
            &dep_ref.name,
            dep_ref.containing_func.as_deref(),
            uri,
            &facts,
            workspace_root,
            state,
        ) {
            actions.push(CodeActionOrCommand::CodeAction(action));
        }
    }

    // extract/router: "Extract router with prefix `{prefix}`"
    // Gate: cursor on a handler whose resolved path shares a literal prefix with ≥1 other handler
    // on the same object in the same file; all must be direct (chain.is_empty()) routes.
    // URL-preservation proof: for direct routes, resolved_path == decorator_path, so
    // stripping the prefix and emitting prefix + stripped_path == original is always valid.
    if let Some(source) = state.file_sources.get(uri).map(|s| s.clone())
        && let Some(action) =
            extract_router_action(uri, range, &facts, &linked, state, workspace_root, &source)
    {
        actions.push(CodeActionOrCommand::CodeAction(action));
    }

    // tpl/missing-template quick fixes (REQ-TPL-05):
    //   "Change to <near-miss>" and "Create <path>" (gated on can_create_files).
    let can_create = state
        .can_create_files
        .load(std::sync::atomic::Ordering::Relaxed);
    let cfg_guard = state.config.try_read();
    let template_roots: Vec<std::path::PathBuf> = cfg_guard
        .as_ref()
        .map(|c| c.template_roots.clone())
        .unwrap_or_default();
    let source_for_tpl = state
        .file_sources
        .get(uri)
        .map(|s| s.clone())
        .unwrap_or_default();
    let source_lines: Vec<&str> = source_for_tpl.lines().collect();
    for tpl in &facts.templates {
        if !position_in_range(range.start, tpl.range.start, tpl.range.end)
            && !position_in_range(range.end, tpl.range.start, tpl.range.end)
        {
            continue;
        }
        if linked.template_index.contains_key(&tpl.path) {
            continue;
        }

        // Gate actions to plain single-quoted/double-quoted literals only.
        // Prefixed (r"", f"") and triple-quoted ("""...""") strings have a quote
        // offset other than 1, so our +1/-1 inner_range calculation would corrupt them.
        let opening_char = source_lines
            .get(tpl.range.start.line as usize)
            .and_then(|line| line.chars().nth(tpl.range.start.character as usize));
        let is_simple_quote = matches!(opening_char, Some('"') | Some('\''));
        if !is_simple_quote {
            continue;
        }

        let inner_range = Range {
            start: Position::new(tpl.range.start.line, tpl.range.start.character + 1),
            end: Position::new(
                tpl.range.end.line,
                tpl.range.end.character.saturating_sub(1),
            ),
        };

        let index_keys: Vec<&str> = linked.template_index.keys().map(|k| k.as_str()).collect();

        // "Change to <near-miss>": replace the string content with the suggestion.
        if let Some(suggestion) = index_keys
            .iter()
            .copied()
            .filter(|k| crate::features::diagnostics::edit_distance(k, &tpl.path) <= 2)
            .min_by_key(|k| crate::features::diagnostics::edit_distance(k, &tpl.path))
        {
            let diag = crate::features::diagnostics::missing_template_diag(
                &tpl.path,
                Some(suggestion),
                tpl.range,
            );
            let action = CodeAction {
                title: format!("Change to '{suggestion}'"),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diag]),
                edit: Some(WorkspaceEdit {
                    document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(
                        vec![TextDocumentEdit {
                            text_document: OptionalVersionedTextDocumentIdentifier {
                                uri: uri.clone(),
                                version: None,
                            },
                            edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                                range: inner_range,
                                new_text: suggestion.to_owned(),
                            })],
                        }],
                    )),
                    ..Default::default()
                }),
                is_preferred: Some(true),
                ..Default::default()
            };
            actions.push(CodeActionOrCommand::CodeAction(action));
        }

        // "Create <path>": create the file under the highest-precedence template root.
        if can_create && let Some(root) = template_roots.first() {
            let Some(target_path) = crate::config::safe_join(root, &tpl.path) else {
                // Reject path traversal attempts embedded in template string literals
                tracing::warn!("code_action: rejecting unsafe template path: {}", tpl.path);
                continue;
            };
            if let Some(target_uri) = path_to_uri(&target_path) {
                use tower_lsp_server::ls_types::{
                    CreateFile, CreateFileOptions, DocumentChangeOperation, ResourceOp,
                };
                let diag =
                    crate::features::diagnostics::missing_template_diag(&tpl.path, None, tpl.range);
                let action = CodeAction {
                    title: format!("Create '{}'", tpl.path),
                    kind: Some(CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diag]),
                    edit: Some(WorkspaceEdit {
                        document_changes: Some(
                            tower_lsp_server::ls_types::DocumentChanges::Operations(vec![
                                DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                                    uri: target_uri,
                                    options: Some(CreateFileOptions {
                                        overwrite: Some(false),
                                        ignore_if_exists: Some(true),
                                    }),
                                    annotation_id: None,
                                })),
                            ]),
                        ),
                        ..Default::default()
                    }),
                    ..Default::default()
                };
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }
    }

    actions
}

/// Extract the full handler block (decorators + function body) from source lines.
/// Returns `(block_start_line, block_end_line, text)` or `None` if out of bounds.
/// Scans backward from the `def` line to collect leading `@decorator` lines.
/// Return the line number after which a new `import` statement can be safely inserted.
/// Skips the module-level docstring (triple-quoted) and any `from __future__ import` lines.
fn import_insert_line(source: &str) -> u32 {
    let lines: Vec<&str> = source.lines().collect();
    let n = lines.len();
    let mut i = 0usize;

    // Skip leading blank/comment lines before any docstring.
    while i < n && (lines[i].trim().is_empty() || lines[i].trim().starts_with('#')) {
        i += 1;
    }

    // Skip the module docstring if the first real line is a triple-quoted string.
    if i < n {
        let trimmed = lines[i].trim();
        let (is_triple, delim) = if trimmed.starts_with("\"\"\"")
            || trimmed.starts_with("r\"\"\"")
            || trimmed.starts_with("u\"\"\"")
        {
            (true, "\"\"\"")
        } else if trimmed.starts_with("'''")
            || trimmed.starts_with("r'''")
            || trimmed.starts_with("u'''")
        {
            (true, "'''")
        } else {
            (false, "")
        };
        if is_triple {
            // Locate the content after the opening delimiter.
            let open_idx = trimmed.find(delim).unwrap_or(0);
            let rest = &trimmed[open_idx + 3..];
            if rest.contains(delim) {
                i += 1; // closing delimiter on the same line
            } else {
                i += 1;
                while i < n {
                    if lines[i].contains(delim) {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
        }
    }

    // Skip `from __future__ import ...` lines (and blanks/comments between them).
    while i < n {
        let trimmed = lines[i].trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("from __future__ import")
        {
            i += 1;
        } else {
            break;
        }
    }

    i as u32
}

fn extract_handler_block(
    lines: &[&str],
    handler_range: Range,
) -> Option<(usize, Position, String)> {
    let def_line = handler_range.start.line as usize;
    let end_line = handler_range.end.line as usize;

    if end_line >= lines.len() {
        return None;
    }

    let mut block_start = def_line;
    while block_start > 0 {
        let prev_trimmed = lines[block_start - 1].trim();
        if prev_trimmed.starts_with('@') || prev_trimmed.starts_with('#') {
            block_start -= 1;
        } else {
            break;
        }
    }

    // Lines from split_inclusive keep their original terminators (\n or \r\n),
    // so concat() reproduces the exact source bytes without normalisation.
    let block_text: String = lines[block_start..=end_line].concat();
    // Clamp the end position to avoid referencing a line past the document.
    let block_end = if end_line + 1 < lines.len() {
        Position::new(end_line as u32 + 1, 0)
    } else {
        Position::new(end_line as u32, lines[end_line].len() as u32)
    };
    Some((block_start, block_end, block_text))
}

/// Find a `deps.py` or `dependencies.py` file in the same directory as the given file.
/// Used to locate the target module for workspace-scoped alias extraction.
fn find_dependency_file(
    handler_uri: &tower_lsp_server::ls_types::Uri,
    state: &WorkspaceState,
) -> Option<tower_lsp_server::ls_types::Uri> {
    let handler_path = uri_to_path(handler_uri)?;
    let handler_dir = handler_path.parent()?;
    for name in &["deps.py", "dependencies.py"] {
        let candidate = path_to_uri(&handler_dir.join(name))?;
        if state.file_sources.contains_key(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Find a `deps.py` file in the same directory as the handler file.
fn find_deps_file(
    handler_uri: &tower_lsp_server::ls_types::Uri,
    state: &WorkspaceState,
) -> Option<tower_lsp_server::ls_types::Uri> {
    let handler_path = uri_to_path(handler_uri)?;
    let handler_dir = handler_path.parent()?;
    let deps_uri = path_to_uri(&handler_dir.join("deps.py"))?;
    if state.file_sources.contains_key(&deps_uri) {
        Some(deps_uri)
    } else {
        None
    }
}

/// Build the "Create dependency `{name}`" code action.
/// Appends a stub to `deps.py` (with an import) when one exists in the same package,
/// or inserts the stub inline above the enclosing handler when no deps.py is present.
fn build_create_dependency_action(
    dep_name: &str,
    containing_func: Option<&str>,
    handler_uri: &tower_lsp_server::ls_types::Uri,
    facts: &crate::state::FileFacts,
    workspace_root: &PathBuf,
    state: &WorkspaceState,
) -> Option<CodeAction> {
    let stub = format!("\ndef {dep_name}():\n    ...\n");

    if let Some(deps_uri) = find_deps_file(handler_uri, state) {
        // deps.py exists — append stub there and add import to current file
        let module_path =
            uri_to_module_path(&deps_uri, workspace_root).unwrap_or_else(|| "deps".to_owned());
        let import_text = format!("from {module_path} import {dep_name}\n");

        return Some(CodeAction {
            title: format!("Create dependency `{dep_name}` in {module_path}"),
            kind: Some(CodeActionKind::QUICKFIX),
            edit: Some(WorkspaceEdit {
                document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                    TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: deps_uri.clone(),
                            version: None,
                        },
                        edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                            range: Range {
                                start: Position::new(APPEND_LINE, 0),
                                end: Position::new(APPEND_LINE, 0),
                            },
                            new_text: stub,
                        })],
                    },
                    TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier {
                            uri: handler_uri.clone(),
                            version: None,
                        },
                        edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                            range: {
                                let ins = state
                                    .file_sources
                                    .get(handler_uri)
                                    .map(|s| import_insert_line(s.as_str()))
                                    .unwrap_or(0);
                                Range {
                                    start: Position::new(ins, 0),
                                    end: Position::new(ins, 0),
                                }
                            },
                            new_text: import_text,
                        })],
                    },
                ])),
                ..Default::default()
            }),
            is_preferred: Some(true),
            ..Default::default()
        });
    }

    // No deps.py — insert stub inline above the enclosing handler
    let func_name = containing_func?;
    let handler_line = facts
        .routes
        .iter()
        .find(|rf| rf.handler_name == func_name)
        .map(|rf| rf.handler_range.start.line)?;

    let source = state.file_sources.get(handler_uri)?;
    let lines: Vec<&str> = source.lines().collect();
    let insert_line = (0..handler_line as usize)
        .rev()
        .find(|&i| {
            lines
                .get(i)
                .is_some_and(|l| l.trim_start().starts_with('@'))
        })
        .unwrap_or(handler_line as usize);

    Some(CodeAction {
        title: format!("Create dependency `{dep_name}` above handler"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: handler_uri.clone(),
                        version: None,
                    },
                    edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                        range: Range {
                            start: Position::new(insert_line as u32, 0),
                            end: Position::new(insert_line as u32, 0),
                        },
                        new_text: format!("\n\ndef {dep_name}():\n    ...\n\n\n"),
                    })],
                },
            ])),
            ..Default::default()
        }),
        is_preferred: Some(true),
        ..Default::default()
    })
}

/// Convert a workspace file URI to a dotted Python module path relative to the workspace root.
/// `file:///project/app/models.py` with root `/project` → `app.models`.
fn uri_to_module_path(uri: &tower_lsp_server::ls_types::Uri, root: &PathBuf) -> Option<String> {
    let file_path = uri_to_path(uri)?;
    let rel = file_path.strip_prefix(root).ok()?;
    let without_ext = rel.with_extension("");
    let module = without_ext
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter(|s| *s != "__init__")
        .collect::<Vec<_>>()
        .join(".");
    if module.is_empty() {
        None
    } else {
        Some(module)
    }
}

fn module_path_to_uri(
    module_path: &str,
    root: &std::path::Path,
) -> Option<tower_lsp_server::ls_types::Uri> {
    // Relative imports (starting with '.') can't be resolved to an absolute path
    if module_path.starts_with('.') {
        return None;
    }
    let rel: PathBuf = module_path.split('.').collect();
    path_to_uri(&root.join(rel).with_extension("py"))
}

fn is_camel_case(s: &str) -> bool {
    // Reject generic types (Optional[X], List[X], etc.) — they contain brackets/commas
    if s.contains(['[', ']', ',', ' ']) {
        return false;
    }
    s.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// Resolve the target file for "Create model": imports-first, then schemas.py fallback.
/// Imports-first: if the current file already imports from a workspace module that has Pydantic
/// models, use that module as the target. Sorted for determinism when multiple candidates match.
/// Otherwise use `schemas.py` in the same directory.
fn resolve_create_model_target(
    imported_from: &std::collections::HashMap<String, String>,
    current_uri: &tower_lsp_server::ls_types::Uri,
    workspace_root: &std::path::Path,
    state: &WorkspaceState,
) -> Option<tower_lsp_server::ls_types::Uri> {
    let mut candidates: Vec<_> = imported_from
        .values()
        .filter_map(|module_path| {
            let candidate_uri = module_path_to_uri(module_path, workspace_root)?;
            let facts = state.file_facts.get(&candidate_uri)?;
            if facts.models.is_empty() {
                return None;
            }
            Some((module_path.clone(), candidate_uri))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some((_, uri)) = candidates.into_iter().next() {
        return Some(uri);
    }
    let current_path = uri_to_path(current_uri)?;
    path_to_uri(&current_path.parent()?.join("schemas.py"))
}

fn build_create_model_action(
    model_name: &str,
    target_uri: &tower_lsp_server::ls_types::Uri,
    workspace_root: &PathBuf,
    state: &WorkspaceState,
    target_exists: bool,
) -> Option<CodeAction> {
    use tower_lsp_server::ls_types::{
        CreateFile, CreateFileOptions, DocumentChangeOperation, ResourceOp,
    };

    let target_module = uri_to_module_path(target_uri, workspace_root)?;

    let document_changes = if target_exists {
        let source = state
            .file_sources
            .get(target_uri)
            .map(|s| s.clone())
            .unwrap_or_default();
        // Only suppress import prepend when a pydantic import line already brings in BaseModel
        let needs_import = !source.lines().any(|l| {
            let l = l.trim();
            l == "from pydantic import *"
                || (l.starts_with("from pydantic import") && l.contains("BaseModel"))
        });

        let mut edits = vec![];
        if needs_import {
            edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                range: Range {
                    start: Position::new(0, 0),
                    end: Position::new(0, 0),
                },
                new_text: "from pydantic import BaseModel\n".to_owned(),
            }));
        }
        edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
            range: Range {
                start: Position::new(APPEND_LINE, 0),
                end: Position::new(APPEND_LINE, 0),
            },
            new_text: format!("\nclass {model_name}(BaseModel):\n    pass\n"),
        }));
        tower_lsp_server::ls_types::DocumentChanges::Edits(vec![TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: target_uri.clone(),
                version: None,
            },
            edits,
        }])
    } else {
        let file_content =
            format!("from pydantic import BaseModel\n\nclass {model_name}(BaseModel):\n    pass\n");
        tower_lsp_server::ls_types::DocumentChanges::Operations(vec![
            DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                uri: target_uri.clone(),
                options: Some(CreateFileOptions {
                    overwrite: Some(false),
                    ignore_if_exists: Some(true),
                }),
                annotation_id: None,
            })),
            DocumentChangeOperation::Edit(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: target_uri.clone(),
                    version: None,
                },
                edits: vec![tower_lsp_server::ls_types::OneOf::Left(TextEdit {
                    range: Range {
                        start: Position::new(0, 0),
                        end: Position::new(0, 0),
                    },
                    new_text: file_content,
                })],
            }),
        ])
    };

    Some(CodeAction {
        title: format!("Create model `{model_name}` in {target_module}"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            document_changes: Some(document_changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// Extract the longest common path prefix (at segment boundaries, literal segments only).
/// Stops at the first parameterized segment `{...}`. Returns "" when no common prefix exists.
fn longest_common_literal_path_prefix(paths: &[&str]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let segments_per_path: Vec<Vec<&str>> = paths
        .iter()
        .map(|p| p.split('/').filter(|s| !s.is_empty()).collect())
        .collect();
    let min_len = segments_per_path.iter().map(|s| s.len()).min().unwrap_or(0);
    let mut common: Vec<&str> = Vec::new();
    for i in 0..min_len {
        let seg = segments_per_path[0][i];
        if seg.contains('{') {
            break;
        }
        if segments_per_path.iter().all(|s| s.get(i) == Some(&seg)) {
            common.push(seg);
        } else {
            break;
        }
    }
    if common.is_empty() {
        return String::new();
    }
    format!("/{}", common.join("/"))
}

/// Find the first decorator line (`@...`) before the handler def line, scanning backward.
fn find_decorator_line(lines: &[&str], record: &crate::state::RouteRecord) -> Option<usize> {
    let def_line = record.handler.range.start.line as usize;
    (0..def_line).rev().find(|&i| {
        lines
            .get(i)
            .is_some_and(|l| l.trim_start().starts_with('@'))
    })
}

/// Find the decorator line that contains the path string at `path_range`.
/// For single-line decorators the path line IS the decorator line; for multi-line, scan backward.
fn find_decorator_line_for_path(lines: &[&str], path_range: Range) -> Option<usize> {
    let path_line = path_range.start.line as usize;
    if lines
        .get(path_line)
        .is_some_and(|l| l.trim_start().starts_with('@'))
    {
        return Some(path_line);
    }
    (0..path_line).rev().find(|&i| {
        lines
            .get(i)
            .is_some_and(|l| l.trim_start().starts_with('@'))
    })
}

/// Return the range of `object_name` in a decorator line `@{object_name}.method(...)`.
fn find_object_range_in_decorator(line: &str, object_name: &str, line_num: u32) -> Option<Range> {
    let at_pos = line.find('@')?;
    let after_at = &line[at_pos + 1..];
    if !after_at.starts_with(object_name) {
        return None;
    }
    let next_idx = at_pos + 1 + object_name.len();
    if !line
        .get(next_idx..)
        .is_some_and(|rest| rest.starts_with('.'))
    {
        return None;
    }
    Some(Range {
        start: Position::new(line_num, (at_pos + 1) as u32),
        end: Position::new(line_num, next_idx as u32),
    })
}

/// Build the "Extract router with prefix" code action, or None when the gate conditions are not met.
fn extract_router_action(
    uri: &tower_lsp_server::ls_types::Uri,
    cursor_range: Range,
    facts: &crate::state::FileFacts,
    linked: &crate::state::Linked,
    state: &WorkspaceState,
    _workspace_root: &PathBuf,
    source: &str,
) -> Option<CodeAction> {
    let lines: Vec<&str> = source.lines().collect();

    // Build object_name lookup from RouteFact (not carried through to RouteRecord)
    let object_name_map: std::collections::HashMap<&str, &str> = facts
        .routes
        .iter()
        .map(|rf| (rf.handler_name.as_str(), rf.object_name.as_str()))
        .collect();

    // Collect all include_router targets across the workspace so we can gate out routes
    // that are already included (replacing the always-empty `chain` check from the old design)
    let included_targets: std::collections::HashSet<String> = state
        .file_facts
        .iter()
        .flat_map(|e| {
            e.value()
                .includes
                .iter()
                .flat_map(|inc| {
                    let mut keys = vec![inc.target.clone()];
                    // Also index the last dotted component (e.g. "books.router" → "router")
                    if let Some(suffix) = inc.target.rsplit('.').next()
                        && suffix != inc.target
                    {
                        keys.push(suffix.to_owned());
                    }
                    keys
                })
                .collect::<Vec<_>>()
        })
        .collect();

    // Candidate routes: same file, NOT already included elsewhere, resolved, with a path_range
    let candidates: Vec<&crate::state::RouteRecord> = linked
        .route_index
        .values()
        .flat_map(|v| v.iter())
        .filter(|r| &r.handler.uri == uri)
        .filter(|r| {
            // RouteId format: "{uri}:{handler_name}:{method}" — uri may contain colons,
            // so split from the right to get handler_name
            let handler_name = r.id.0.rsplit(':').nth(1).unwrap_or("");
            let obj_name = object_name_map
                .get(handler_name)
                .copied()
                .unwrap_or(handler_name);
            !included_targets.contains(obj_name)
        })
        .filter(|r| matches!(r.resolved_path, ResolvedPath::Resolved(_)))
        .filter(|r| r.path_range.is_some())
        .collect();

    if candidates.len() < 2 {
        return None;
    }

    // Find the handler the cursor is on
    let cursor_record = candidates.iter().copied().find(|r| {
        position_in_range(
            cursor_range.start,
            r.handler.range.start,
            r.handler.range.end,
        ) || position_in_range(cursor_range.end, r.handler.range.start, r.handler.range.end)
    })?;

    let cursor_obj = *object_name_map.get(cursor_record.name.as_str())?;

    // Restrict to routes on the same object
    let same_obj: Vec<&crate::state::RouteRecord> = candidates
        .iter()
        .copied()
        .filter(|r| object_name_map.get(r.name.as_str()) == Some(&cursor_obj))
        .collect();

    if same_obj.len() < 2 {
        return None;
    }

    // Compute LCP of all paths on this object
    let all_paths: Vec<&str> = same_obj
        .iter()
        .filter_map(|r| match &r.resolved_path {
            ResolvedPath::Resolved(p) => Some(p.as_str()),
            _ => None,
        })
        .collect();

    let prefix = longest_common_literal_path_prefix(&all_paths);
    if prefix.is_empty() {
        return None;
    }

    // Group: routes whose resolved_path starts with prefix at a segment boundary.
    // Use p.get() to avoid a panic when prefix.len() is not a valid char boundary.
    let group: Vec<&crate::state::RouteRecord> = same_obj
        .iter()
        .copied()
        .filter(|r| match &r.resolved_path {
            ResolvedPath::Resolved(p) => {
                p == &prefix
                    || p.get(prefix.len()..)
                        .is_some_and(|rest| rest.starts_with('/'))
            }
            _ => false,
        })
        .collect();

    if group.len() < 2 {
        return None;
    }

    // Pick router variable name (avoid "router" if already declared in this file)
    let router_name: String = if facts.routers.iter().any(|r| r.name == "router") {
        let last_seg = prefix
            .rsplit('/')
            .find(|s| !s.is_empty() && !s.contains('{'))
            .unwrap_or("sub");
        format!("{last_seg}_router")
    } else {
        "router".to_owned()
    };

    let needs_apirouter_import = !facts
        .imported_names
        .iter()
        .any(|n| n == "APIRouter" || n == "*");

    // Find first handler's decorator line for the insert position
    let first_record = group.iter().min_by_key(|r| r.handler.range.start.line)?;
    let first_deco_line = find_decorator_line(&lines, first_record)?;

    // Build edits, highest line first (defensive ordering for non-compliant LSP clients)
    let mut edits: Vec<
        tower_lsp_server::ls_types::OneOf<TextEdit, tower_lsp_server::ls_types::AnnotatedTextEdit>,
    > = Vec::new();

    // Append include_router at end (highest effective line)
    edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
        range: Range {
            start: Position::new(APPEND_LINE, 0),
            end: Position::new(APPEND_LINE, 0),
        },
        new_text: format!("\n{cursor_obj}.include_router({router_name})\n"),
    }));

    // Per-handler edits in reverse line order; deduplicate on path_range
    let mut sorted_group = group.clone();
    sorted_group.sort_by_key(|r| std::cmp::Reverse(r.handler.range.start.line));
    let mut seen_path_ranges = std::collections::HashSet::<Range>::new();

    for record in &sorted_group {
        let path_range = record.path_range.unwrap();
        if !seen_path_ranges.insert(path_range) {
            continue;
        }

        let resolved = match &record.resolved_path {
            ResolvedPath::Resolved(p) => p.as_str(),
            _ => continue,
        };
        let stripped = resolved.get(prefix.len()..).unwrap_or("");
        let new_path = format!("\"{stripped}\"");

        // Edit: replace path string
        edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
            range: path_range,
            new_text: new_path,
        }));

        // Edit: replace object_name with router_name in decorator line.
        // If the decorator line can't be found, the action would produce broken Python
        // (path stripped but object not renamed) — abort the whole action.
        let deco_line = find_decorator_line_for_path(&lines, path_range)?;
        let deco_str = lines.get(deco_line)?;
        let obj_range = find_object_range_in_decorator(deco_str, cursor_obj, deco_line as u32)?;
        edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
            range: obj_range,
            new_text: router_name.clone(),
        }));
    }

    // Insert router declaration + optional APIRouter import before first decorator
    let mut router_decl = String::new();
    if needs_apirouter_import {
        router_decl.push_str("from fastapi import APIRouter\n");
    }
    router_decl.push_str(&format!(
        "{router_name} = APIRouter(prefix=\"{prefix}\")\n\n"
    ));
    edits.push(tower_lsp_server::ls_types::OneOf::Left(TextEdit {
        range: Range {
            start: Position::new(first_deco_line as u32, 0),
            end: Position::new(first_deco_line as u32, 0),
        },
        new_text: router_decl,
    }));

    Some(CodeAction {
        title: format!("Extract router with prefix `{prefix}`"),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(vec![
                TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version: None,
                    },
                    edits,
                },
            ])),
            ..Default::default()
        }),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResolvedConfig;
    use crate::state::{
        FileFacts, Linked, Location as StateLocation, Method, PathConverter, PathParam,
        PrefixValue, ResolvedPath, RouteFact, RouteId, RouteRecord,
    };
    use std::sync::Arc;
    use tower_lsp_server::ls_types::{
        CodeActionContext, PartialResultParams, TextDocumentIdentifier, Uri, WorkDoneProgressParams,
    };

    fn make_params(uri: &Uri, cursor: Position) -> CodeActionParams {
        CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range {
                start: cursor,
                end: cursor,
            },
            context: CodeActionContext {
                diagnostics: vec![],
                only: None,
                trigger_kind: None,
            },
            work_done_progress_params: WorkDoneProgressParams {
                work_done_token: None,
            },
            partial_result_params: PartialResultParams {
                partial_result_token: None,
            },
        }
    }

    fn make_route(
        uri: &Uri,
        path: &str,
        path_params: Vec<PathParam>,
        handler_params: Vec<String>,
        handler_param_ranges: Vec<Range>,
        params_insert_pos: Option<Position>,
        has_splat: bool,
    ) -> (RouteId, RouteRecord) {
        let id = RouteId(format!("app.handler:{}:GET", path));
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
            path_range: Some(Range {
                start: Position::new(2, 0),
                end: Position::new(2, (path.len() + 2) as u32),
            }),
            path_quote_width: None,
            handler_params,
            handler_param_ranges,
            params_insert_pos,
            handler_has_splat_args: has_splat,
            handler_params_known: true,
        };
        (id, record)
    }

    fn make_state(uri: &Uri, record: RouteRecord) -> Arc<WorkspaceState> {
        let state = WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/tmp")));
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let mut linked = Linked::default();
        let id = record.id.clone();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));
        state
    }

    #[test]
    fn param_missing_arg_action_offered_on_path_segment() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        };
        let (_, record) = make_route(
            &uri,
            "/items/{book_id}",
            vec![path_param],
            vec![],
            vec![],
            Some(Position::new(5, 15)),
            false,
        );
        let state = make_state(&uri, record);
        // `{book_id}` is at path offset 7 (0-based), length 9
        // path_range starts at (2, 0), content starts at col 1 (skip quote)
        // so segment col_start = 0 + 1 + 7 = 8, col_end = 8 + 9 = 17
        let cursor = Position::new(2, 8);
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let titles: Vec<&str> = actions
            .iter()
            .filter_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    Some(ca.title.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            titles.iter().any(|t| t.contains("book_id")),
            "expected add-param action, got: {:?}",
            titles
        );
    }

    #[test]
    fn param_missing_arg_action_not_offered_when_cursor_misses_segment() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        };
        let (_, record) = make_route(
            &uri,
            "/items/{book_id}",
            vec![path_param],
            vec![],
            vec![],
            Some(Position::new(5, 15)),
            false,
        );
        let state = make_state(&uri, record);
        let cursor = Position::new(10, 0); // far from the path
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let param_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("book_id")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            param_actions.is_empty(),
            "action should not appear when cursor misses segment"
        );
    }

    #[test]
    fn param_missing_arg_no_comma_for_empty_params() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "x".to_owned(),
            converter: PathConverter::Str,
        };
        let (_, record) = make_route(
            &uri,
            "/{x}",
            vec![path_param],
            vec![],
            vec![],
            Some(Position::new(5, 10)),
            false,
        );
        let state = make_state(&uri, record);
        // `{x}` is at path offset 1, length 3; col_start = 0 + 1 + 1 = 2, col_end = 5
        let params = make_params(&uri, Position::new(2, 2));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let action = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    Some(ca)
                } else {
                    None
                }
            })
            .expect("action should exist");
        let edit = action.edit.as_ref().unwrap();
        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) =
            &edit.document_changes
        {
            let text = &edits[0].edits[0];
            if let tower_lsp_server::ls_types::OneOf::Left(te) = text {
                assert!(
                    !te.new_text.starts_with(','),
                    "empty params should not start with comma"
                );
                assert!(te.new_text.contains("x: str"));
            }
        }
    }

    #[test]
    fn param_missing_arg_adds_comma_when_params_exist() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "x".to_owned(),
            converter: PathConverter::Str,
        };
        let (_, record) = make_route(
            &uri,
            "/{x}",
            vec![path_param],
            vec!["request".to_owned()],
            vec![Range::default()],
            Some(Position::new(5, 20)),
            false,
        );
        let state = make_state(&uri, record);
        let params = make_params(&uri, Position::new(2, 2));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let action = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    Some(ca)
                } else {
                    None
                }
            })
            .expect("action should exist");
        let edit = action.edit.as_ref().unwrap();
        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) =
            &edit.document_changes
            && let tower_lsp_server::ls_types::OneOf::Left(te) = &edits[0].edits[0] {
                assert!(
                    te.new_text.starts_with(", "),
                    "should prepend comma when params exist"
                );
            }
    }

    #[test]
    fn param_missing_arg_not_offered_when_splat_args() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        };
        let (_, record) = make_route(
            &uri,
            "/items/{book_id}",
            vec![path_param],
            vec![],
            vec![],
            Some(Position::new(5, 15)),
            true, // has_splat = true
        );
        let state = make_state(&uri, record);
        let params = make_params(&uri, Position::new(2, 8));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let param_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("book_id")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            param_actions.is_empty(),
            "splat args should suppress add-param action"
        );
    }

    #[test]
    fn arg_missing_param_rename_offered_for_near_miss() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        };
        // handler has "boook_id" (edit distance 1 from "book_id" — one extra 'o')
        let hp_range = Range {
            start: Position::new(6, 8),
            end: Position::new(6, 16),
        };
        let (_, record) = make_route(
            &uri,
            "/items/{book_id}",
            vec![path_param],
            vec!["boook_id".to_owned()],
            vec![hp_range],
            Some(Position::new(6, 17)),
            false,
        );
        let state = make_state(&uri, record);
        let params = make_params(&uri, Position::new(6, 8)); // cursor on "boook_id"
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let rename = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.contains("Rename") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            rename.is_some(),
            "rename action should be offered for near-miss"
        );
        let action = rename.unwrap();
        assert!(
            action.title.contains("book_id"),
            "title should mention target name"
        );

        // Verify the text edit replaces exactly the handler param range with the path param name
        if let Some(WorkspaceEdit {
            document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)),
            ..
        }) = &action.edit
        {
            assert_eq!(edits.len(), 1);
            if let tower_lsp_server::ls_types::OneOf::Left(te) = &edits[0].edits[0] {
                assert_eq!(
                    te.range, hp_range,
                    "edit must cover the handler param token"
                );
                assert_eq!(te.new_text, "book_id", "must rename to path param name");
            }
        } else {
            panic!("expected document_changes with one text edit");
        }
    }

    #[test]
    fn arg_missing_param_rename_not_offered_when_multiple_unbound_params() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        // Two unbound path params — gate should prevent action
        let p1 = PathParam {
            name: "a".to_owned(),
            converter: PathConverter::Str,
        };
        let p2 = PathParam {
            name: "b".to_owned(),
            converter: PathConverter::Str,
        };
        let hp_range = Range {
            start: Position::new(6, 8),
            end: Position::new(6, 11),
        };
        let (_, record) = make_route(
            &uri,
            "/{a}/{b}",
            vec![p1, p2],
            vec!["aa".to_owned()], // near-miss for "a", but "b" also unbound → two unbound
            vec![hp_range],
            Some(Position::new(6, 12)),
            false,
        );
        let state = make_state(&uri, record);
        let params = make_params(&uri, Position::new(6, 8));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let rename_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Rename")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            rename_actions.is_empty(),
            "rename should not appear when multiple params unbound"
        );
    }

    #[test]
    fn arg_missing_param_rename_not_offered_when_ranges_mismatch_handler_params() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        };
        let hp_range = Range {
            start: Position::new(6, 8),
            end: Position::new(6, 16),
        };
        // Deliberately pass empty handler_param_ranges (len=0) with a non-empty handler_params (len=1)
        let id = RouteId("app.handler:/items/{book_id}:GET".to_owned());
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "handler".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/items/{book_id}".to_owned()),
            decorator_path: "/items/{book_id}".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: hp_range,
            },
            path_params: vec![path_param],
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: Some(Range {
                start: Position::new(2, 0),
                end: Position::new(2, 18),
            }),
            path_quote_width: None,
            handler_params: vec!["boook_id".to_owned()],
            handler_param_ranges: vec![], // intentionally mismatched
            params_insert_pos: Some(Position::new(6, 17)),
            handler_has_splat_args: false,
            handler_params_known: true,
        };
        let state = make_state(&uri, record);
        let params = make_params(&uri, Position::new(6, 8));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let rename_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Rename")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            rename_actions.is_empty(),
            "mismatched ranges must suppress rename to prevent destructive edit"
        );
    }

    #[test]
    fn arg_missing_param_add_segment_offered_with_rename() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let path_param = PathParam {
            name: "book_id".to_owned(),
            converter: PathConverter::Str,
        };
        let hp_range = Range {
            start: Position::new(6, 8),
            end: Position::new(6, 16),
        };
        let (_, record) = make_route(
            &uri,
            "/items/{book_id}",
            vec![path_param],
            vec!["boook_id".to_owned()],
            vec![hp_range],
            Some(Position::new(6, 17)),
            false,
        );
        let state = make_state(&uri, record);
        let params = make_params(&uri, Position::new(6, 8));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let add_seg = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.contains("segment") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            add_seg.is_some(),
            "add-segment action should appear alongside rename"
        );
        let action = add_seg.unwrap();
        assert!(
            action.title.contains("boook_id"),
            "title uses handler param name"
        );
        if let Some(WorkspaceEdit {
            document_changes: Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)),
            ..
        }) = &action.edit
            && let tower_lsp_server::ls_types::OneOf::Left(te) = &edits[0].edits[0] {
                assert_eq!(
                    te.new_text, "/{boook_id}",
                    "segment text uses handler param name"
                );
            }
    }

    fn make_state_with_annotated_param(
        uri: &Uri,
        ap: crate::state::AnnotatedParam,
    ) -> Arc<WorkspaceState> {
        let state = WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/tmp")));
        let mut facts = FileFacts::new(uri.clone());
        facts.annotated_params.push(ap);
        state.file_facts.insert(uri.clone(), facts);
        state.linked.store(Arc::new(Linked::default()));
        state
    }

    #[test]
    fn convert_inline_to_annotated_offered_on_cursor() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let type_range = Range {
            start: Position::new(3, 17),
            end: Position::new(3, 24),
        }; // "Session"
        let default_range = Range {
            start: Position::new(3, 27),
            end: Position::new(3, 42),
        }; // "Depends(get_db)"
        let ap = crate::state::AnnotatedParam {
            param_name: "db".to_owned(),
            containing_func: "get_book".to_owned(),
            is_annotated: false,
            annotation_range: type_range,
            default_range: Some(default_range),
            type_text: "Session".to_owned(),
            depends_text: "Depends(get_db)".to_owned(),
            has_extra_args: false,
        };
        let state = make_state_with_annotated_param(&uri, ap);

        let cursor = Position::new(3, 18); // inside "Session"
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let convert = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.contains("Annotated style") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            convert.is_some(),
            "should offer convert-to-Annotated action"
        );
        let action = convert.unwrap();
        assert!(action.title.contains("db"), "title should name the param");

        // Verify the edit replaces the combined range with Annotated[Session, Depends(get_db)]
        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = action
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let all_edits: Vec<_> = edits.iter().flat_map(|e| e.edits.iter()).collect();
            let annotated_edit = all_edits.iter().find(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text.contains("Annotated[Session")
                } else {
                    false
                }
            });
            assert!(
                annotated_edit.is_some(),
                "edit should contain Annotated[Session, Depends(get_db)]"
            );
        } else {
            panic!("expected document_changes");
        }
    }

    #[test]
    fn convert_inline_to_annotated_adds_import_when_missing() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let type_range = Range {
            start: Position::new(3, 17),
            end: Position::new(3, 24),
        };
        let default_range = Range {
            start: Position::new(3, 27),
            end: Position::new(3, 42),
        };
        let ap = crate::state::AnnotatedParam {
            param_name: "db".to_owned(),
            containing_func: "get_book".to_owned(),
            is_annotated: false,
            annotation_range: type_range,
            default_range: Some(default_range),
            type_text: "Session".to_owned(),
            depends_text: "Depends(get_db)".to_owned(),
            has_extra_args: false,
        };
        let state = make_state_with_annotated_param(&uri, ap);

        let params = make_params(&uri, Position::new(3, 18));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let convert = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("Annotated style") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap();

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = convert
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let all_edits: Vec<_> = edits.iter().flat_map(|e| e.edits.iter()).collect();
            let import_edit = all_edits.iter().find(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text.contains("from typing import Annotated")
                } else {
                    false
                }
            });
            assert!(
                import_edit.is_some(),
                "should add Annotated import when missing"
            );
        }
    }

    #[test]
    fn convert_annotated_to_inline_offered_on_cursor() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let annotation_range = Range {
            start: Position::new(3, 17),
            end: Position::new(3, 50),
        }; // "Annotated[Session, Depends(get_db)]"
        let ap = crate::state::AnnotatedParam {
            param_name: "db".to_owned(),
            containing_func: "get_book".to_owned(),
            is_annotated: true,
            annotation_range,
            default_range: None,
            type_text: "Session".to_owned(),
            depends_text: "Depends(get_db)".to_owned(),
            has_extra_args: false,
        };
        let state = make_state_with_annotated_param(&uri, ap);

        let cursor = Position::new(3, 25); // inside the Annotated[...] span
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let convert = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.contains("inline style") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(convert.is_some(), "should offer convert-to-inline action");

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = convert
            .unwrap()
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let all_edits: Vec<_> = edits.iter().flat_map(|e| e.edits.iter()).collect();
            assert_eq!(
                all_edits.len(),
                1,
                "inline conversion should produce exactly one edit"
            );
            if let tower_lsp_server::ls_types::OneOf::Left(te) = all_edits[0] {
                assert_eq!(te.range, annotation_range);
                assert_eq!(te.new_text, "Session = Depends(get_db)");
            }
        }
    }

    #[test]
    fn convert_inline_to_annotated_no_import_when_already_imported() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let type_range = Range {
            start: Position::new(3, 17),
            end: Position::new(3, 24),
        };
        let default_range = Range {
            start: Position::new(3, 27),
            end: Position::new(3, 42),
        };
        let ap = crate::state::AnnotatedParam {
            containing_func: "get_book".to_owned(),
            param_name: "db".to_owned(),
            is_annotated: false,
            annotation_range: type_range,
            default_range: Some(default_range),
            type_text: "Session".to_owned(),
            depends_text: "Depends(get_db)".to_owned(),
            has_extra_args: false,
        };
        let state = WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/tmp")));
        let mut facts = FileFacts::new(uri.clone());
        facts.imported_names.push("Annotated".to_owned()); // already imported
        facts.annotated_params.push(ap);
        state.file_facts.insert(uri.clone(), facts);
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(3, 18));
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let convert = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("Annotated style") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .unwrap();

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = convert
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let has_import_edit = edits.iter().flat_map(|e| e.edits.iter()).any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text.contains("from typing import Annotated")
                } else {
                    false
                }
            });
            assert!(
                !has_import_edit,
                "should NOT add import when Annotated is already imported"
            );
        }
    }

    #[test]
    fn convert_inline_to_annotated_offered_for_cursor_on_depends() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let type_range = Range {
            start: Position::new(3, 17),
            end: Position::new(3, 24),
        };
        let default_range = Range {
            start: Position::new(3, 27),
            end: Position::new(3, 42),
        }; // Depends(...)
        let ap = crate::state::AnnotatedParam {
            containing_func: "get_book".to_owned(),
            param_name: "db".to_owned(),
            is_annotated: false,
            annotation_range: type_range,
            default_range: Some(default_range),
            type_text: "Session".to_owned(),
            depends_text: "Depends(get_db)".to_owned(),
            has_extra_args: false,
        };
        let state = make_state_with_annotated_param(&uri, ap);

        // Cursor on the Depends(...) side — still within the action span
        let cursor = Position::new(3, 30);
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let convert_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Annotated style")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            !convert_actions.is_empty(),
            "action should be offered when cursor is on Depends side"
        );
    }

    #[test]
    fn convert_annotated_to_inline_not_offered_when_extra_args() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let annotation_range = Range {
            start: Position::new(3, 17),
            end: Position::new(3, 60),
        };
        let ap = crate::state::AnnotatedParam {
            containing_func: "handler".to_owned(),
            param_name: "x".to_owned(),
            is_annotated: true,
            annotation_range,
            default_range: None,
            type_text: "int".to_owned(),
            depends_text: "Depends(fn)".to_owned(),
            has_extra_args: true, // Annotated[int, Depends(fn), Field()]
        };
        let state = make_state_with_annotated_param(&uri, ap);

        let cursor = Position::new(3, 30);
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let inline_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("inline style")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            inline_actions.is_empty(),
            "inline conversion must be suppressed when extra Annotated args present"
        );
    }

    #[test]
    fn convert_not_offered_when_cursor_misses_param() {
        let uri: Uri = "file:///app.py".parse().unwrap();
        let type_range = Range {
            start: Position::new(3, 17),
            end: Position::new(3, 24),
        };
        let default_range = Range {
            start: Position::new(3, 27),
            end: Position::new(3, 42),
        };
        let ap = crate::state::AnnotatedParam {
            param_name: "db".to_owned(),
            containing_func: "get_book".to_owned(),
            is_annotated: false,
            annotation_range: type_range,
            default_range: Some(default_range),
            type_text: "Session".to_owned(),
            depends_text: "Depends(get_db)".to_owned(),
            has_extra_args: false,
        };
        let state = make_state_with_annotated_param(&uri, ap);

        let cursor = Position::new(10, 0); // far from param
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let convert_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Annotated style") || ca.title.contains("inline style")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            convert_actions.is_empty(),
            "should not offer when cursor misses the param span"
        );
    }

    fn make_route_with_response_model(
        uri: &Uri,
        response_model: &str,
        rm_range: Range,
    ) -> (crate::state::RouteId, RouteRecord) {
        let id = crate::state::RouteId("app.handler:GET".to_string());
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: "handler".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/items".to_owned()),
            decorator_path: "/items".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: Range {
                    start: Position::new(1, 0),
                    end: Position::new(1, 20),
                },
            },
            path_params: vec![],
            response_model: Some(response_model.to_owned()),
            response_model_range: Some(rm_range),
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
    fn create_model_offered_for_camel_case_not_in_index() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let rm_range = Range {
            start: Position::new(0, 20),
            end: Position::new(0, 30),
        };
        let (id, record) = make_route_with_response_model(&uri, "BookCreate", rm_range);

        // schemas.py exists in the same package (so no file creation needed — target_exists=true)
        let schemas_uri: Uri = "file:///project/schemas.py".parse().unwrap();

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        state
            .file_facts
            .insert(schemas_uri.clone(), FileFacts::new(schemas_uri.clone()));
        // file_sources must also have schemas.py so target_exists=true (desync safety)
        state
            .file_sources
            .insert(schemas_uri.clone(), String::new());
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(0, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let create_action = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.contains("Create model") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            create_action.is_some(),
            "create model action should be offered"
        );
        let action = create_action.unwrap();
        assert!(
            action.title.contains("BookCreate"),
            "title must name the model"
        );
        assert!(
            action.title.contains("schemas"),
            "title must name target module"
        );
    }

    #[test]
    fn create_model_not_offered_when_model_in_index() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let rm_range = Range {
            start: Position::new(0, 20),
            end: Position::new(0, 30),
        };
        let (id, record) = make_route_with_response_model(&uri, "BookCreate", rm_range);

        let schemas_uri: Uri = "file:///project/schemas.py".parse().unwrap();
        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        state
            .file_facts
            .insert(schemas_uri.clone(), FileFacts::new(schemas_uri.clone()));
        let mut linked = Linked::default();
        // BookCreate IS in the model_index — "import" action handles it, not "create"
        linked.model_index.insert(
            "BookCreate".to_owned(),
            vec![crate::state::ModelRecord {
                name: "BookCreate".to_owned(),
                location: StateLocation {
                    uri: schemas_uri.clone(),
                    range: Range::default(),
                },
                is_settings: false,
            }],
        );
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(0, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Create model")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "create action must not appear when model is already in the index"
        );
    }

    #[test]
    fn create_model_not_offered_when_already_imported() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let rm_range = Range {
            start: Position::new(0, 20),
            end: Position::new(0, 30),
        };
        let (id, record) = make_route_with_response_model(&uri, "BookCreate", rm_range);

        let schemas_uri: Uri = "file:///project/schemas.py".parse().unwrap();
        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.imported_names.push("BookCreate".to_owned());
        state.file_facts.insert(uri.clone(), facts);
        state
            .file_facts
            .insert(schemas_uri.clone(), FileFacts::new(schemas_uri.clone()));
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(0, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Create model")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "create action must not appear when name is already imported"
        );
    }

    #[test]
    fn create_model_uses_imports_first_when_imported_module_has_models() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let rm_range = Range {
            start: Position::new(0, 20),
            end: Position::new(0, 30),
        };
        let (id, record) = make_route_with_response_model(&uri, "BookCreate", rm_range);

        // The current file imports from "app.models", which has Pydantic models
        let models_uri: Uri = "file:///project/app/models.py".parse().unwrap();
        let schemas_uri: Uri = "file:///project/schemas.py".parse().unwrap();

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts
            .imported_from
            .insert("Book".to_owned(), "app.models".to_owned());
        state.file_facts.insert(uri.clone(), facts);

        // app/models.py tracked and has models
        let mut models_facts = FileFacts::new(models_uri.clone());
        models_facts.models.push(crate::state::ModelFact {
            name: "Book".to_owned(),
            range: Range::default(),
            is_settings: false,
        });
        state.file_facts.insert(models_uri.clone(), models_facts);
        state.file_sources.insert(
            models_uri.clone(),
            "from pydantic import BaseModel\nclass Book(BaseModel): pass\n".to_owned(),
        );

        // schemas.py also tracked (would be fallback)
        state
            .file_facts
            .insert(schemas_uri.clone(), FileFacts::new(schemas_uri.clone()));
        state
            .file_sources
            .insert(schemas_uri.clone(), "".to_owned());

        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(0, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let create_action = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.contains("Create model") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            create_action.is_some(),
            "create model action should be offered"
        );
        let action = create_action.unwrap();
        assert!(
            action.title.contains("app.models"),
            "imports-first: action should target app.models, got: {}",
            action.title
        );
        assert!(
            !action.title.contains("schemas"),
            "schemas.py fallback must NOT be used when an imported module with models exists"
        );
    }

    #[test]
    fn create_model_new_file_requires_create_capability() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let rm_range = Range {
            start: Position::new(0, 20),
            end: Position::new(0, 30),
        };
        let (id, record) = make_route_with_response_model(&uri, "BookCreate", rm_range);

        // schemas.py is NOT in file_facts — it would need to be created
        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        let mut linked = Linked::default();
        linked.route_index.insert(id, vec![record]);
        state.linked.store(Arc::new(linked));

        // can_create_files is false by default
        let params = make_params(&uri, Position::new(0, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);
        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Create model")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "must not offer create action when can_create_files=false"
        );

        // Now enable the capability
        state
            .can_create_files
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);
        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Create model")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            !create_actions.is_empty(),
            "must offer create action when can_create_files=true"
        );
    }

    #[test]
    fn route_shadowed_move_above_offered_same_file() {
        let uri: Uri = "file:///app.py".parse().unwrap();

        // Provide source text:
        // Line 0: @app.get('/items/{id}')
        // Line 1: def get_book(id: str): ...
        // Line 2: (blank)
        // Line 3: @app.get('/items/featured')
        // Line 4: def get_featured(): ...
        let source = "@app.get('/items/{id}')\ndef get_book(id: str): ...\n\n@app.get('/items/featured')\ndef get_featured(): ...\n";
        // Line 0: @app.get('/items/{id}')
        // Line 1: def get_book(id: str): ...
        // Line 2: (blank)
        // Line 3: @app.get('/items/featured')
        // Line 4: def get_featured(): ...

        // Adjust ranges to match actual line numbers
        let shadower_range_adj = Range {
            start: Position::new(1, 0),
            end: Position::new(1, 26),
        };
        let shadowed_range_adj = Range {
            start: Position::new(4, 0),
            end: Position::new(4, 22),
        };

        let id_sh = RouteId("app.shadower2".to_owned());
        let shadower2 = RouteRecord {
            id: id_sh.clone(),
            ordinal: 0,
            name: "get_book".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/items/{id}".to_owned()),
            decorator_path: "/items/{id}".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: shadower_range_adj,
            },
            path_params: vec![PathParam {
                name: "id".to_owned(),
                converter: PathConverter::Str,
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
            handler_params_known: true,
        };
        let id_sd = RouteId("app.shadowed2".to_owned());
        let shadowed2 = RouteRecord {
            id: id_sd.clone(),
            ordinal: 1,
            name: "get_featured".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/items/featured".to_owned()),
            decorator_path: "/items/featured".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: shadowed_range_adj,
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

        let state = WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/tmp")));
        state
            .file_facts
            .insert(uri.clone(), FileFacts::new(uri.clone()));
        state.file_sources.insert(uri.clone(), source.to_owned());
        let mut linked = Linked::default();
        linked.route_index.insert(id_sh, vec![shadower2]);
        linked.route_index.insert(id_sd, vec![shadowed2]);
        state.linked.store(Arc::new(linked));

        let cursor = Position::new(4, 0); // cursor on shadowed handler
        let params = make_params(&uri, cursor);
        let actions = code_actions(&state, &params, &PathBuf::from("/tmp"), &[], false);

        let move_action = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.starts_with("Move route above") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(
            move_action.is_some(),
            "Move route above action should be offered"
        );
        let action = move_action.unwrap();
        assert!(
            action.title.contains("get_book"),
            "title names the shadowing handler"
        );
    }

    // ── extract/router tests ─────────────────────────────────────────────────

    /// Build a RouteFact for the extract-router tests.
    fn make_route_fact(
        handler_name: &str,
        object_name: &str,
        path: &str,
        path_range: Range,
        handler_range: Range,
    ) -> RouteFact {
        RouteFact {
            handler_name: handler_name.to_owned(),
            handler_range,
            object_name: object_name.to_owned(),
            methods: vec![Method::Get],
            path: PrefixValue::Literal(path.to_owned()),
            path_range: Some(path_range),
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
        }
    }

    /// Build a RouteRecord for the extract-router tests (chain.is_empty()).
    fn make_direct_route(
        uri: &Uri,
        handler_name: &str,
        path: &str,
        path_range: Range,
        handler_range: Range,
    ) -> RouteRecord {
        RouteRecord {
            id: RouteId::new(uri, handler_name, &Method::Get),
            ordinal: 0,
            name: handler_name.to_owned(),
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
            path_range: Some(path_range),
            path_quote_width: None,
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        }
    }

    #[test]
    fn extract_router_offered_for_routes_sharing_common_prefix() {
        // Source:
        // Line 0: @app.get("/books")
        // Line 1: def list_books(): ...
        // Line 2: @app.get("/books/{book_id}")
        // Line 3: def get_book(book_id: int): ...
        let source = "@app.get(\"/books\")\ndef list_books(): ...\n@app.get(\"/books/{book_id}\")\ndef get_book(book_id: int): ...\n";
        let uri: Uri = "file:///project/app.py".parse().unwrap();

        let pr1 = Range {
            start: Position::new(0, 9),
            end: Position::new(0, 17),
        }; // "/books"
        let hr1 = Range {
            start: Position::new(1, 0),
            end: Position::new(1, 20),
        };
        let pr2 = Range {
            start: Position::new(2, 9),
            end: Position::new(2, 27),
        }; // "/books/{book_id}"
        let hr2 = Range {
            start: Position::new(3, 0),
            end: Position::new(3, 30),
        };

        let rf1 = make_route_fact("list_books", "app", "/books", pr1, hr1);
        let rf2 = make_route_fact("get_book", "app", "/books/{book_id}", pr2, hr2);
        let rr1 = make_direct_route(&uri, "list_books", "/books", pr1, hr1);
        let rr2 = make_direct_route(&uri, "get_book", "/books/{book_id}", pr2, hr2);

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.routes.push(rf1);
        facts.routes.push(rf2);
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        let mut linked = Linked::default();
        linked.route_index.insert(rr1.id.clone(), vec![rr1]);
        linked.route_index.insert(rr2.id.clone(), vec![rr2]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(1, 0)); // cursor on list_books def
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let extract = actions.iter().find_map(|a| {
            if let CodeActionOrCommand::CodeAction(ca) = a {
                if ca.title.contains("Extract router") {
                    Some(ca)
                } else {
                    None
                }
            } else {
                None
            }
        });
        assert!(extract.is_some(), "extract router action should be offered");
        let action = extract.unwrap();
        assert!(
            action.title.contains("/books"),
            "title must include the prefix"
        );
    }

    #[test]
    fn extract_router_rewrites_path_and_object_name() {
        let source = "@app.get(\"/books\")\ndef list_books(): ...\n@app.get(\"/books/{book_id}\")\ndef get_book(book_id: int): ...\n";
        let uri: Uri = "file:///project/app.py".parse().unwrap();

        let pr1 = Range {
            start: Position::new(0, 9),
            end: Position::new(0, 17),
        };
        let hr1 = Range {
            start: Position::new(1, 0),
            end: Position::new(1, 20),
        };
        let pr2 = Range {
            start: Position::new(2, 9),
            end: Position::new(2, 27),
        };
        let hr2 = Range {
            start: Position::new(3, 0),
            end: Position::new(3, 30),
        };

        let rf1 = make_route_fact("list_books", "app", "/books", pr1, hr1);
        let rf2 = make_route_fact("get_book", "app", "/books/{book_id}", pr2, hr2);
        let rr1 = make_direct_route(&uri, "list_books", "/books", pr1, hr1);
        let rr2 = make_direct_route(&uri, "get_book", "/books/{book_id}", pr2, hr2);

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.routes.push(rf1);
        facts.routes.push(rf2);
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        let mut linked = Linked::default();
        linked.route_index.insert(rr1.id.clone(), vec![rr1]);
        linked.route_index.insert(rr2.id.clone(), vec![rr2]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(1, 0));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let extract = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("Extract router") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .expect("action should exist");

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = extract
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let all_edits: Vec<_> = edits.iter().flat_map(|e| e.edits.iter()).collect();
            // Path /books → "" (empty, stripped prefix)
            let has_empty_path = all_edits.iter().any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text == "\"\""
                } else {
                    false
                }
            });
            assert!(has_empty_path, "should strip prefix from /books → \"\"");
            // Path /books/{book_id} → /{book_id}
            let has_stripped_path = all_edits.iter().any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text == "\"/{book_id}\""
                } else {
                    false
                }
            });
            assert!(
                has_stripped_path,
                "should strip prefix from /books/{{book_id}} → /{{book_id}}"
            );
            // Object name `app` → `router`
            let has_obj_rename = all_edits.iter().any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text == "router"
                } else {
                    false
                }
            });
            assert!(
                has_obj_rename,
                "should rename object_name from app to router"
            );
            // Router declaration insert
            let has_router_decl = all_edits.iter().any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text.contains("APIRouter(prefix=\"/books\")")
                } else {
                    false
                }
            });
            assert!(has_router_decl, "should insert APIRouter declaration");
            // include_router append
            let has_include = all_edits.iter().any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text.contains("app.include_router(router)")
                } else {
                    false
                }
            });
            assert!(has_include, "should append include_router call");
        } else {
            panic!("expected document_changes with edits");
        }
    }

    #[test]
    fn extract_router_not_offered_with_single_route() {
        let source = "@app.get(\"/books/{book_id}\")\ndef get_book(book_id: int): ...\n";
        let uri: Uri = "file:///project/app.py".parse().unwrap();

        let pr = Range {
            start: Position::new(0, 9),
            end: Position::new(0, 27),
        };
        let hr = Range {
            start: Position::new(1, 0),
            end: Position::new(1, 30),
        };

        let rf = make_route_fact("get_book", "app", "/books/{book_id}", pr, hr);
        let rr = make_direct_route(&uri, "get_book", "/books/{book_id}", pr, hr);

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.routes.push(rf);
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        let mut linked = Linked::default();
        linked.route_index.insert(rr.id.clone(), vec![rr]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(1, 0));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let extract_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Extract router")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            extract_actions.is_empty(),
            "must not offer extract router with only one candidate"
        );
    }

    // ── extract-dependency tests ─────────────────────────────────────────────

    fn make_annotated_dep_param(
        param_name: &str,
        func: &str,
        line: u32,
        col_start: u32,
        col_end: u32,
        type_text: &str,
        depends_text: &str,
    ) -> crate::state::AnnotatedParam {
        crate::state::AnnotatedParam {
            param_name: param_name.to_owned(),
            containing_func: func.to_owned(),
            is_annotated: true,
            annotation_range: Range {
                start: Position::new(line, col_start),
                end: Position::new(line, col_end),
            },
            default_range: None,
            type_text: type_text.to_owned(),
            depends_text: depends_text.to_owned(),
            has_extra_args: false,
        }
    }

    #[test]
    fn extract_dependency_same_file_offered_when_dep_defined() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        // "db: Annotated[Session, Depends(get_db)]" at line 1, cols 4-40
        let ap = make_annotated_dep_param("db", "get_book", 1, 4, 40, "Session", "Depends(get_db)");
        let source = "@app.get(\"/books\")\ndef get_book(book_id: int, db: Annotated[Session, Depends(get_db)]): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.annotated_params.push(ap);
        facts.routes.push(make_route_fact_simple("get_book", 1));
        // dep is defined → gate passes
        facts.dep_defs.push(crate::state::DepDef {
            name: "get_db".to_owned(),
            node_id: crate::state::NodeId {
                uri: uri.clone(),
                range: Range::default(),
            },
            has_yield: false,
            param_names: vec![],
        });
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(1, 20));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let action = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("SessionDep") && !ca.title.contains("workspace") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .expect("same-file extract action should be offered");

        assert_eq!(action.kind, Some(CodeActionKind::REFACTOR_EXTRACT));

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = action
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let all_edits: Vec<_> = edits.iter().flat_map(|e| e.edits.iter()).collect();
            // Should have: alias insert + annotation replacement
            assert!(
                all_edits.len() >= 2,
                "should have alias insert and annotation replacement"
            );

            let has_alias = all_edits.iter().any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text
                        .contains("SessionDep = Annotated[Session, Depends(get_db)]")
                } else {
                    false
                }
            });
            assert!(has_alias, "should insert alias definition");

            let has_replacement = all_edits.iter().any(|e| {
                if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                    te.new_text == "SessionDep"
                } else {
                    false
                }
            });
            assert!(has_replacement, "should replace annotation with alias name");
        } else {
            panic!("expected document_changes");
        }
    }

    #[test]
    fn extract_dependency_not_offered_when_dep_undefined() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let ap = make_annotated_dep_param("db", "get_book", 1, 4, 40, "Session", "Depends(get_db)");
        let source = "@app.get(\"/books\")\ndef get_book(book_id: int, db: Annotated[Session, Depends(get_db)]): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.annotated_params.push(ap);
        facts.routes.push(make_route_fact_simple("get_book", 1));
        // No dep_defs → gate fails
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(1, 20));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let extract_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("SessionDep")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            extract_actions.is_empty(),
            "should not offer extract when dep is undefined"
        );
    }

    #[test]
    fn extract_dependency_not_offered_when_alias_already_bound() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let ap = make_annotated_dep_param("db", "get_book", 2, 4, 40, "Session", "Depends(get_db)");
        // SessionDep is already defined at line 0
        let source = "SessionDep = Annotated[Session, Depends(get_db)]\n@app.get(\"/books\")\ndef get_book(book_id: int, db: Annotated[Session, Depends(get_db)]): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.annotated_params.push(ap);
        facts.routes.push(make_route_fact_simple("get_book", 2));
        facts.dep_defs.push(crate::state::DepDef {
            name: "get_db".to_owned(),
            node_id: crate::state::NodeId {
                uri: uri.clone(),
                range: Range::default(),
            },
            has_yield: false,
            param_names: vec![],
        });
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(2, 20));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let extract_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Extract to named dependency")
                        && ca.title.contains("SessionDep")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            extract_actions.is_empty(),
            "should not offer extract when alias name is already bound"
        );
    }

    #[test]
    fn extract_dependency_replaces_all_occurrences_in_file() {
        let uri: Uri = "file:///project/app.py".parse().unwrap();
        let source = "@app.get(\"/a\")\ndef handler_a(db: Annotated[Session, Depends(get_db)]): ...\n@app.get(\"/b\")\ndef handler_b(db: Annotated[Session, Depends(get_db)]): ...\n";
        // Two occurrences of the same annotation
        let ap1 =
            make_annotated_dep_param("db", "handler_a", 1, 17, 53, "Session", "Depends(get_db)");
        let ap2 =
            make_annotated_dep_param("db", "handler_b", 3, 17, 53, "Session", "Depends(get_db)");

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.annotated_params.push(ap1);
        facts.annotated_params.push(ap2);
        facts.routes.push(make_route_fact_simple("handler_a", 1));
        facts.routes.push(make_route_fact_simple("handler_b", 3));
        facts.dep_defs.push(crate::state::DepDef {
            name: "get_db".to_owned(),
            node_id: crate::state::NodeId {
                uri: uri.clone(),
                range: Range::default(),
            },
            has_yield: false,
            param_names: vec![],
        });
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(1, 30));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let action = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("SessionDep") && !ca.title.contains("workspace") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .expect("extract action should be offered");

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = action
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let replacement_count = edits
                .iter()
                .flat_map(|e| e.edits.iter())
                .filter(|e| {
                    if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                        te.new_text == "SessionDep"
                    } else {
                        false
                    }
                })
                .count();
            assert_eq!(
                replacement_count, 2,
                "should replace both occurrences of the annotation"
            );
        } else {
            panic!("expected document_changes");
        }
    }

    #[test]
    fn extract_dependency_workspace_uses_deps_py() {
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let deps_uri: Uri = "file:///project/app/deps.py".parse().unwrap();
        let ap = make_annotated_dep_param("db", "get_book", 1, 4, 40, "Session", "Depends(get_db)");
        let source = "@app.get(\"/books\")\ndef get_book(book_id: int, db: Annotated[Session, Depends(get_db)]): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.annotated_params.push(ap);
        facts.routes.push(make_route_fact_simple("get_book", 1));
        facts.dep_defs.push(crate::state::DepDef {
            name: "get_db".to_owned(),
            node_id: crate::state::NodeId {
                uri: uri.clone(),
                range: Range::default(),
            },
            has_yield: false,
            param_names: vec![],
        });
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        // deps.py is present
        state.file_sources.insert(deps_uri.clone(), String::new());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(1, 20));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let ws_action = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("SessionDep") && ca.title.contains("workspace") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .expect("workspace extract action should be offered");

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = ws_action
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            let uris: Vec<_> = edits.iter().map(|e| e.text_document.uri.as_str()).collect();
            assert!(
                uris.iter().any(|u| u.contains("deps.py")),
                "workspace action should target deps.py for alias; got {uris:?}",
            );
        } else {
            panic!("expected document_changes");
        }
    }

    // ── create-dependency helpers ────────────────────────────────────────────
    fn make_dep_ref(
        name: &str,
        line: u32,
        col_start: u32,
        col_end: u32,
        func: Option<&str>,
    ) -> crate::state::DepRef {
        crate::state::DepRef {
            name: name.to_owned(),
            range: Range {
                start: Position::new(line, col_start),
                end: Position::new(line, col_end),
            },
            is_called: false,
            callee_range: None,
            containing_func: func.map(str::to_owned),
            caller_node_id: None,
        }
    }

    fn make_route_fact_simple(handler_name: &str, handler_line: u32) -> RouteFact {
        let hr = Range {
            start: Position::new(handler_line, 0),
            end: Position::new(handler_line, 20),
        };
        RouteFact {
            handler_name: handler_name.to_owned(),
            handler_range: hr,
            object_name: "app".to_owned(),
            path: crate::state::PrefixValue::Literal("/items".to_owned()),
            path_range: Some(Range {
                start: Position::new(handler_line - 1, 9),
                end: Position::new(handler_line - 1, 17),
            }),
            path_quote_width: None,
            methods: vec![crate::state::Method::Get],
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
        }
    }

    #[test]
    fn create_dependency_offered_when_name_unproven() {
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let source = "@app.get(\"/items\")\ndef get_items(pg = Depends(get_pagination)): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        // dep_ref at the Depends(get_pagination) position — col 19..35 on line 1
        facts
            .dep_refs
            .push(make_dep_ref("get_pagination", 1, 19, 35, Some("get_items")));
        facts.routes.push(make_route_fact_simple("get_items", 1));
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        // cursor inside the Depends(...) range
        let params = make_params(&uri, Position::new(1, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let titles: Vec<&str> = actions
            .iter()
            .filter_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    Some(ca.title.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            titles.iter().any(|t| t.contains("get_pagination")),
            "expected create-dependency action, got: {titles:?}",
        );
    }

    #[test]
    fn create_dependency_not_offered_when_defined() {
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let source = "@app.get(\"/items\")\ndef get_items(pg = Depends(get_pagination)): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts
            .dep_refs
            .push(make_dep_ref("get_pagination", 1, 19, 35, Some("get_items")));
        // defined: dep_def for get_pagination exists (it has a function definition)
        facts.dep_defs.push(crate::state::DepDef {
            name: "get_pagination".to_owned(),
            node_id: crate::state::NodeId {
                uri: uri.clone(),
                range: Range::default(),
            },
            has_yield: false,
            param_names: vec![],
        });
        facts.routes.push(make_route_fact_simple("get_items", 1));
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(1, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("get_pagination") && ca.title.contains("Create dependency")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "must not offer create-dependency when dep is defined"
        );
    }

    #[test]
    fn create_dependency_targets_deps_py_when_present() {
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let deps_uri: Uri = "file:///project/app/deps.py".parse().unwrap();
        let source = "@app.get(\"/items\")\ndef get_items(pg = Depends(get_pagination)): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts
            .dep_refs
            .push(make_dep_ref("get_pagination", 1, 19, 35, Some("get_items")));
        facts.routes.push(make_route_fact_simple("get_items", 1));
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        // deps.py is present
        state.file_sources.insert(deps_uri.clone(), String::new());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(1, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let action = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("get_pagination") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .expect("action should exist");

        // Should reference deps.py in title
        assert!(
            action.title.contains("deps"),
            "title should reference deps module: {}",
            action.title
        );

        // Should have two document edits: one to deps.py and one to main.py
        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = action
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            assert_eq!(
                edits.len(),
                2,
                "expected 2 document edits (deps.py + main.py)"
            );
            let uris: Vec<_> = edits.iter().map(|e| e.text_document.uri.as_str()).collect();
            assert!(
                uris.iter().any(|u| u.contains("deps.py")),
                "missing deps.py edit"
            );
            assert!(
                uris.iter().any(|u| u.contains("main.py")),
                "missing main.py import edit"
            );

            // The main.py edit should be an import at line 0
            let import_edit = edits
                .iter()
                .find(|e| e.text_document.uri.as_str().contains("main.py"))
                .and_then(|e| e.edits.first())
                .and_then(|e| {
                    if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                        Some(te)
                    } else {
                        None
                    }
                })
                .expect("missing import edit");
            assert_eq!(
                import_edit.range.start.line, 0,
                "import should be inserted at line 0"
            );
            assert!(
                import_edit.new_text.contains("get_pagination"),
                "import text should mention get_pagination"
            );
        } else {
            panic!("expected document_changes with edits");
        }
    }

    #[test]
    fn create_dependency_inserts_inline_when_no_deps_py() {
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let source = "@app.get(\"/items\")\ndef get_items(pg = Depends(get_pagination)): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts
            .dep_refs
            .push(make_dep_ref("get_pagination", 1, 19, 35, Some("get_items")));
        facts.routes.push(make_route_fact_simple("get_items", 1));
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        // No deps.py
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(1, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let action = actions
            .iter()
            .find_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    if ca.title.contains("get_pagination") {
                        Some(ca)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .expect("action should exist");

        assert!(
            action.title.contains("above handler"),
            "title should say 'above handler': {}",
            action.title
        );

        if let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(edits)) = action
            .edit
            .as_ref()
            .and_then(|e| e.document_changes.as_ref())
        {
            assert_eq!(edits.len(), 1, "expected 1 document edit (inline only)");
            let edit = edits[0]
                .edits
                .first()
                .and_then(|e| {
                    if let tower_lsp_server::ls_types::OneOf::Left(te) = e {
                        Some(te)
                    } else {
                        None
                    }
                })
                .expect("missing text edit");
            assert!(
                edit.new_text.contains("def get_pagination"),
                "stub should contain function def"
            );
            assert!(
                edit.new_text.contains("..."),
                "stub should contain ellipsis body"
            );
            // Should be inserted at the decorator line (line 0), not at the handler line
            assert_eq!(edit.range.start.line, 0, "should insert at decorator line");
        } else {
            panic!("expected document_changes with edits");
        }
    }

    #[test]
    fn create_dependency_not_offered_when_containing_func_missing() {
        // When dep_ref has no containing_func and no deps.py, build_create_dependency_action
        // returns None (can't locate handler to insert above).
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let source = "pg = Depends(get_pagination)\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        // containing_func = None (module-scope dep_ref)
        facts
            .dep_refs
            .push(make_dep_ref("get_pagination", 0, 5, 28, None));
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        let params = make_params(&uri, Position::new(0, 15));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("get_pagination")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "must not offer action when containing_func is None and no deps.py"
        );
    }

    #[test]
    fn create_dependency_only_offered_for_dep_ref_under_cursor() {
        // Two dep_refs in the same handler; cursor is only on get_pagination.
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let source = "@app.get(\"/items\")\ndef get_items(a = Depends(get_pagination), b = Depends(get_db)): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts
            .dep_refs
            .push(make_dep_ref("get_pagination", 1, 18, 34, Some("get_items")));
        facts
            .dep_refs
            .push(make_dep_ref("get_db", 1, 44, 54, Some("get_items")));
        facts.routes.push(make_route_fact_simple("get_items", 1));
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        // cursor only over get_pagination (col 25, inside col 18-34)
        let params = make_params(&uri, Position::new(1, 25));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let titles: Vec<&str> = actions
            .iter()
            .filter_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    Some(ca.title.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            titles.iter().any(|t| t.contains("get_pagination")),
            "should offer for get_pagination"
        );
        assert!(
            !titles.iter().any(|t| t.contains("get_db")),
            "must NOT offer for get_db when cursor misses its range"
        );
    }

    #[test]
    fn create_dependency_not_offered_when_cursor_misses_range() {
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let source = "@app.get(\"/items\")\ndef get_items(pg = Depends(get_pagination)): ...\n";

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts
            .dep_refs
            .push(make_dep_ref("get_pagination", 1, 19, 35, Some("get_items")));
        facts.routes.push(make_route_fact_simple("get_items", 1));
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), source.to_owned());
        state.linked.store(Arc::new(Linked::default()));

        // cursor far from the Depends range
        let params = make_params(&uri, Position::new(0, 0));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("get_pagination")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "must not offer action when cursor misses range"
        );
    }

    #[test]
    fn extract_router_not_offered_when_router_already_included() {
        // Routes are on `router` which is already included via include_router in main.py.
        // The action should NOT be offered.
        let source = "@router.get(\"/books\")\ndef list_books(): ...\n@router.get(\"/books/{book_id}\")\ndef get_book(book_id: int): ...\n";
        let uri: Uri = "file:///project/books.py".parse().unwrap();
        let main_uri: Uri = "file:///project/main.py".parse().unwrap();

        let pr1 = Range {
            start: Position::new(0, 12),
            end: Position::new(0, 20),
        };
        let hr1 = Range {
            start: Position::new(1, 0),
            end: Position::new(1, 20),
        };
        let pr2 = Range {
            start: Position::new(2, 12),
            end: Position::new(2, 30),
        };
        let hr2 = Range {
            start: Position::new(3, 0),
            end: Position::new(3, 30),
        };

        let rr1 = RouteRecord {
            id: RouteId::new(&uri, "list_books", &Method::Get),
            ordinal: 0,
            name: "list_books".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/api/books".to_owned()),
            decorator_path: "/books".to_owned(),
            chain: vec![], // chain is always empty in production
            handler: StateLocation {
                uri: uri.clone(),
                range: hr1,
            },
            path_params: vec![],
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: Some(pr1),
            path_quote_width: None,
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        };
        let rr2 = RouteRecord {
            id: RouteId::new(&uri, "get_book", &Method::Get),
            ordinal: 1,
            name: "get_book".to_owned(),
            method: Method::Get,
            resolved_path: ResolvedPath::Resolved("/api/books/{book_id}".to_owned()),
            decorator_path: "/books/{book_id}".to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri: uri.clone(),
                range: hr2,
            },
            path_params: vec![],
            response_model: None,
            response_model_range: None,
            return_annotation: None,
            dependencies: vec![],
            middleware: vec![],
            path_range: Some(pr2),
            path_quote_width: None,
            handler_params: vec![],
            handler_param_ranges: vec![],
            params_insert_pos: None,
            handler_has_splat_args: false,
            handler_params_known: true,
        };

        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut books_facts = FileFacts::new(uri.clone());
        books_facts
            .routes
            .push(make_route_fact("list_books", "router", "/books", pr1, hr1));
        books_facts.routes.push(make_route_fact(
            "get_book",
            "router",
            "/books/{book_id}",
            pr2,
            hr2,
        ));
        state.file_facts.insert(uri.clone(), books_facts);
        state.file_sources.insert(uri.clone(), source.to_owned());

        // main.py includes the router — this is what gates the action
        let mut main_facts = FileFacts::new(main_uri.clone());
        main_facts.includes.push(crate::state::IncludeCall {
            target: "router".to_owned(),
            prefix: crate::state::PrefixValue::Literal("/api".to_owned()),
            app_name: "app".to_owned(),
            dependencies: vec![],
            range: Range::default(),
        });
        state.file_facts.insert(main_uri, main_facts);

        let mut linked = Linked::default();
        linked.route_index.insert(rr1.id.clone(), vec![rr1]);
        linked.route_index.insert(rr2.id.clone(), vec![rr2]);
        state.linked.store(Arc::new(linked));

        let params = make_params(&uri, Position::new(1, 0));
        let actions = code_actions(&state, &params, &PathBuf::from("/project"), &[], false);

        let extract_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Extract router")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            extract_actions.is_empty(),
            "must not offer extract router when router is already included"
        );
    }

    // ── tpl/missing-template quick fixes ─────────────────────────────────────

    fn tpl_ref_range() -> Range {
        Range {
            start: Position::new(2, 40),
            end: Position::new(2, 55),
        }
    }

    /// Source that puts a `"` at column 40 of line 2, matching tpl_ref_range.
    fn tpl_ref_source() -> String {
        "# line 0\n# line 1\n".to_owned() + &" ".repeat(40) + "\"some_template.html\"\n"
    }

    #[test]
    fn change_to_near_miss_offered_when_suggestion_exists() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        // typo: "book_lst.html" instead of "book_list.html"
        facts.templates.push(TemplateRef {
            path: "book_lst.html".to_owned(),
            range: tpl_ref_range(),
        });
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), tpl_ref_source());

        let mut linked = Linked::default();
        linked.template_index.insert(
            "book_list.html".to_owned(),
            "file:///project/tpl/book_list.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let cursor = Position::new(2, 45);
        let actions = code_actions(
            &state,
            &make_params(&uri, cursor),
            &PathBuf::from("/project"),
            &[],
            false,
        );
        let titles: Vec<&str> = actions
            .iter()
            .filter_map(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    Some(ca.title.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(
            titles.iter().any(|t| t.contains("book_list.html")),
            "change-to action should be offered"
        );
    }

    #[test]
    fn create_template_not_offered_without_can_create_files() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "new.html".to_owned(),
            range: tpl_ref_range(),
        });
        state.file_facts.insert(uri.clone(), facts);
        state
            .can_create_files
            .store(false, std::sync::atomic::Ordering::Relaxed);

        let mut linked = Linked::default();
        linked.template_index.insert(
            "other.html".to_owned(),
            "file:///project/tpl/other.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let cursor = Position::new(2, 45);
        let actions = code_actions(
            &state,
            &make_params(&uri, cursor),
            &PathBuf::from("/project"),
            &[],
            false,
        );
        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Create") && ca.title.contains("new.html")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "create action must not appear without can_create_files"
        );
    }

    #[test]
    fn create_template_offered_when_can_create_files_and_root_exists() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let mut cfg = ResolvedConfig::default_for_root(PathBuf::from("/project"));
        cfg.template_roots = vec![PathBuf::from("/project/templates")];
        let state = WorkspaceState::new(cfg);
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "new.html".to_owned(),
            range: tpl_ref_range(),
        });
        state.file_facts.insert(uri.clone(), facts);
        state.file_sources.insert(uri.clone(), tpl_ref_source());
        state
            .can_create_files
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let mut linked = Linked::default();
        linked.template_index.insert(
            "other.html".to_owned(),
            "file:///project/templates/other.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let cursor = Position::new(2, 45);
        let actions = code_actions(
            &state,
            &make_params(&uri, cursor),
            &PathBuf::from("/project"),
            &[],
            false,
        );
        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("Create") && ca.title.contains("new.html")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            !create_actions.is_empty(),
            "create action must appear when can_create_files=true and root exists"
        );
    }

    #[test]
    fn no_template_actions_when_template_found_in_index() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "index.html".to_owned(),
            range: tpl_ref_range(),
        });
        state.file_facts.insert(uri.clone(), facts);
        state
            .can_create_files
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let mut linked = Linked::default();
        linked.template_index.insert(
            "index.html".to_owned(),
            "file:///project/tpl/index.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let cursor = Position::new(2, 45);
        let actions = code_actions(
            &state,
            &make_params(&uri, cursor),
            &PathBuf::from("/project"),
            &[],
            false,
        );
        let tpl_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("index.html")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            tpl_actions.is_empty(),
            "no template actions when template is found"
        );
    }

    #[test]
    fn create_template_path_traversal_is_rejected() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let mut cfg = ResolvedConfig::default_for_root(PathBuf::from("/project"));
        cfg.template_roots = vec![PathBuf::from("/project/templates")];
        let state = WorkspaceState::new(cfg);
        state
            .can_create_files
            .store(true, std::sync::atomic::Ordering::Relaxed);

        // Simulate a template string containing path traversal: "../../etc/passwd"
        let evil_path = "../../etc/passwd".to_owned();
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: evil_path.clone(),
            range: tpl_ref_range(),
        });
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        linked.template_index.insert(
            "other.html".to_owned(),
            "file:///project/templates/other.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        let cursor = Position::new(2, 45);
        let actions = code_actions(
            &state,
            &make_params(&uri, cursor),
            &PathBuf::from("/project"),
            &[],
            false,
        );
        // Must NOT offer a "Create" action that would write outside the template root
        let create_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.starts_with("Create") && ca.title.contains(&evil_path)
                } else {
                    false
                }
            })
            .collect();
        assert!(
            create_actions.is_empty(),
            "path traversal in template name must not produce Create action"
        );
    }

    #[test]
    fn no_template_actions_when_cursor_outside_string_range() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let state =
            WorkspaceState::new(ResolvedConfig::default_for_root(PathBuf::from("/project")));
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "missing.html".to_owned(),
            range: tpl_ref_range(),
        });
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        linked.template_index.insert(
            "other.html".to_owned(),
            "file:///project/tpl/other.html".parse().unwrap(),
        );
        state.linked.store(Arc::new(linked));

        // Cursor is on a completely different line/column — outside tpl_ref_range().
        let cursor = Position::new(10, 0);
        let actions = code_actions(
            &state,
            &make_params(&uri, cursor),
            &PathBuf::from("/project"),
            &[],
            false,
        );
        let tpl_actions: Vec<_> = actions
            .iter()
            .filter(|a| {
                if let CodeActionOrCommand::CodeAction(ca) = a {
                    ca.title.contains("missing.html")
                } else {
                    false
                }
            })
            .collect();
        assert!(
            tpl_actions.is_empty(),
            "no template actions when cursor outside string range"
        );
    }

    #[test]
    fn import_insert_line_empty_file() {
        assert_eq!(import_insert_line(""), 0);
    }

    #[test]
    fn import_insert_line_no_header() {
        assert_eq!(import_insert_line("import fastapi\n"), 0);
    }

    #[test]
    fn import_insert_line_after_future() {
        let src = "from __future__ import annotations\nimport fastapi\n";
        assert_eq!(import_insert_line(src), 1);
    }

    #[test]
    fn import_insert_line_after_docstring_and_future() {
        let src =
            "\"\"\"Module docstring.\"\"\"\nfrom __future__ import annotations\nimport fastapi\n";
        assert_eq!(import_insert_line(src), 2);
    }

    #[test]
    fn import_insert_line_after_multiline_docstring() {
        let src = "\"\"\"\nMulti-line docstring.\n\"\"\"\nfrom __future__ import annotations\n";
        assert_eq!(import_insert_line(src), 4);
    }

    #[test]
    fn extract_handler_block_preserves_crlf() {
        // CRLF source must round-trip: extracted text keeps \r\n, not converted to \n.
        let source = "@app.get(\"/\")\r\nasync def root():\r\n    return {}\r\n";
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let handler_range = Range {
            start: Position::new(0, 0),
            end: Position::new(2, 14),
        };
        let (start, _end, text) = extract_handler_block(&lines, handler_range).unwrap();
        assert_eq!(start, 0);
        assert!(
            text.contains("\r\n"),
            "CRLF must be preserved, got: {:?}",
            text
        );
        assert_eq!(text, source);
    }

    #[test]
    fn extract_handler_block_no_trailing_newline_clamped() {
        // Last line with no trailing newline: end position must point to end of last line
        // rather than line_count+1, 0 which would be past the document.
        let source = "@app.get(\"/\")\nasync def root():\n    return {}";
        let lines: Vec<&str> = source.split_inclusive('\n').collect();
        let handler_range = Range {
            start: Position::new(0, 0),
            end: Position::new(2, 14),
        };
        let (_, end_pos, _) = extract_handler_block(&lines, handler_range).unwrap();
        // Last line is index 2; since it has no terminator, end should be (2, len_of_last_line)
        assert_eq!(end_pos.line, 2, "end line must not exceed last line");
        assert!(
            end_pos.character > 0,
            "end character must point inside last line"
        );
    }
}
