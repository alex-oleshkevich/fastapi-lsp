use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionResponse, CompletionTextEdit,
    Documentation, InsertTextFormat, MarkupContent, MarkupKind, Position, Range, TextEdit, Uri,
};

use crate::check::uri_to_display_path;
use crate::state::{ResolvedPath, WorkspaceState};
use crate::util::{is_secret_key, position_in_range};

pub fn completion(state: &WorkspaceState, uri: &Uri, pos: Position) -> Option<CompletionResponse> {
    let facts = state.file_facts.get(uri)?;

    // url_for name completion: cursor inside (or at the edge of) the route-name string argument.
    // Use inclusive end check so an empty string '' (start==end) still triggers.
    if let Some(site) = facts.url_for_sites.iter().find(|s| {
        s.name_range.is_some_and(|r| {
            let after_start = pos.line > r.start.line
                || (pos.line == r.start.line && pos.character >= r.start.character);
            let before_end = pos.line < r.end.line
                || (pos.line == r.end.line && pos.character <= r.end.character);
            after_start && before_end
        })
    }) {
        return url_for_name_completions(state, site.name_range.unwrap());
    }

    // Env key completion: cursor must be inside the key string itself.
    if let Some(site) = facts
        .env_lookups
        .iter()
        .find(|s| position_in_range(pos, s.key_range.start, s.key_range.end))
    {
        return env_key_completions(state, site.replace_range);
    }

    // Client-call path completion (REQ-CPL-02): cursor inside the path string of a test client call.
    if let Some(call) = facts
        .client_calls
        .iter()
        .find(|c| position_in_range(pos, c.path_range.start, c.path_range.end))
    {
        return client_path_completions(state, &call.method, call.path_range);
    }

    // Template path completion (REQ-CPL-04): cursor inside a recognised template string.
    // TemplateRef.range uses tree-sitter's exclusive end_position(); position_in_range uses <.
    if let Some(tpl) = facts
        .templates
        .iter()
        .find(|t| position_in_range(pos, t.range.start, t.range.end))
    {
        // inner_range: content between the quotes (quotes are always ASCII — +1/-1 is safe).
        let inner_range = Range {
            start: Position::new(tpl.range.start.line, tpl.range.start.character + 1),
            end: Position::new(
                tpl.range.end.line,
                tpl.range.end.character.saturating_sub(1),
            ),
        };
        return template_path_completions(state, &tpl.path, inner_range);
    }

    None
}

// ── Env key completion (REQ-CPL-05) ──────────────────────────────────────────

fn env_key_completions(state: &WorkspaceState, replace_range: Range) -> Option<CompletionResponse> {
    let linked = state.linked.load();
    if linked.env_index.is_empty() {
        return None;
    }

    let items: Vec<CompletionItem> = linked
        .env_index
        .iter()
        .map(|(key, entry)| {
            let value_display = if is_secret_key(key) {
                "••••••".to_owned()
            } else {
                entry.value.clone()
            };

            CompletionItem {
                label: key.clone(),
                kind: Some(CompletionItemKind::CONSTANT),
                detail: Some(format!("`{value_display}`")),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!("`{key}` = `{value_display}`"),
                })),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: key.clone(),
                })),
                filter_text: Some(key.clone()),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            }
        })
        .collect();

    Some(CompletionResponse::List(CompletionList {
        is_incomplete: false,
        items,
    }))
}

// ── url_for name completion ───────────────────────────────────────────────────

fn url_for_name_completions(
    state: &WorkspaceState,
    replace_range: Range,
) -> Option<CompletionResponse> {
    let linked = state.linked.load();
    if linked.route_names.is_empty() {
        return None;
    }

    let items: Vec<CompletionItem> = linked
        .route_names
        .iter()
        .filter_map(|(name, ids)| {
            let record = ids
                .first()
                .and_then(|id| linked.route_index.get(id))
                .and_then(|v| v.first())?;
            let path = match &record.resolved_path {
                ResolvedPath::Resolved(p) => p.as_str(),
                ResolvedPath::Unresolved => return None,
            };
            Some(CompletionItem {
                label: name.clone(),
                kind: Some(CompletionItemKind::VALUE),
                detail: Some(format!("{} {}", record.method, path)),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: name.clone(),
                })),
                filter_text: Some(name.clone()),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            })
        })
        .collect();

    if items.is_empty() {
        return None;
    }

    Some(CompletionResponse::List(CompletionList {
        is_incomplete: false,
        items,
    }))
}

// ── Client-call path completion (REQ-CPL-02) ─────────────────────────────────

fn client_path_completions(
    state: &WorkspaceState,
    method: &crate::state::Method,
    replace_range: Range,
) -> Option<CompletionResponse> {
    let linked = state.linked.load();

    let items: Vec<CompletionItem> = linked
        .route_index
        .values()
        .flat_map(|records| records.iter())
        .filter(|r| &r.method == method)
        .filter_map(|r| {
            let path = match &r.resolved_path {
                ResolvedPath::Resolved(p) => p.clone(),
                ResolvedPath::Unresolved => return None,
            };
            let snippet = path_to_snippet(&path);
            let is_snippet = snippet != path;
            Some(CompletionItem {
                label: path.clone(),
                kind: Some(CompletionItemKind::REFERENCE),
                detail: Some(format!("{}", r.method)),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: format!(
                        "**{}**\n\n{}",
                        r.name,
                        uri_to_display_path(r.handler.uri.as_str()),
                    ),
                })),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: snippet,
                })),
                filter_text: Some(path),
                insert_text_format: Some(if is_snippet {
                    InsertTextFormat::SNIPPET
                } else {
                    InsertTextFormat::PLAIN_TEXT
                }),
                ..Default::default()
            })
        })
        .collect();

    if items.is_empty() {
        return None;
    }

    Some(CompletionResponse::List(CompletionList {
        is_incomplete: false,
        items,
    }))
}

/// Convert a route path to a snippet string.
/// `{param}` and `{name:path}` segments become `${N:name}` tab stops.
fn path_to_snippet(path: &str) -> String {
    let mut counter = 0u32;
    let segments: Vec<String> = path
        .split('/')
        .map(|seg| {
            if seg.starts_with('{') && seg.ends_with('}') {
                counter += 1;
                let inner = &seg[1..seg.len() - 1];
                let name = inner.split(':').next().unwrap_or(inner);
                format!("${{{counter}:{name}}}")
            } else {
                seg.to_owned()
            }
        })
        .collect();
    segments.join("/")
}

// ── Middleware kwarg completion (REQ-CPL-06) ──────────────────────────────────

// ── Template path completion (REQ-CPL-04) ─────────────────────────────────────

/// Return completions for a template path string.
///
/// `prefix` is the text the user has already typed inside the string (= `TemplateRef.path`
/// from the last parse).  `replace_range` is the range of the string content (no quotes).
///
/// Entries that start with `prefix` are returned; subdirectory entries are deduplicated to
/// the immediate next path segment, completing one level at a time.  When any directory
/// entries are returned the list is marked `isIncomplete` so the client re-triggers on `/`.
fn template_path_completions(
    state: &WorkspaceState,
    prefix: &str,
    replace_range: Range,
) -> Option<CompletionResponse> {
    let linked = state.linked.load();
    if linked.template_index.is_empty() {
        return None;
    }

    let mut seen_dirs: std::collections::HashSet<String> = Default::default();
    let mut items: Vec<CompletionItem> = vec![];
    let mut has_dirs = false;

    for key in linked.template_index.keys() {
        if !key.starts_with(prefix) {
            continue;
        }
        let rest = &key[prefix.len()..];
        if let Some(slash_pos) = rest.find('/') {
            // Entry lives in a subdirectory — emit the immediate child directory once.
            let dir_completion = format!("{}{}/", prefix, &rest[..slash_pos]);
            if seen_dirs.insert(dir_completion.clone()) {
                has_dirs = true;
                items.push(CompletionItem {
                    label: dir_completion.clone(),
                    kind: Some(CompletionItemKind::FOLDER),
                    filter_text: Some(dir_completion.clone()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: replace_range,
                        new_text: dir_completion,
                    })),
                    insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                    // Auto-reopen the completion list after inserting the directory separator.
                    command: Some(tower_lsp_server::ls_types::Command {
                        title: String::new(),
                        command: "editor.action.triggerSuggest".to_owned(),
                        arguments: None,
                    }),
                    ..Default::default()
                });
            }
        } else {
            // Entry is a file directly at the current prefix level.
            let full_path = linked
                .template_index
                .get(key)
                .map(|u| uri_to_display_path(u.as_str()));
            items.push(CompletionItem {
                label: key.clone(),
                kind: Some(CompletionItemKind::FILE),
                filter_text: Some(key.clone()),
                documentation: full_path.map(|p| {
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: p,
                    })
                }),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace_range,
                    new_text: key.clone(),
                })),
                insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
                ..Default::default()
            });
        }
    }

    if items.is_empty() {
        return None;
    }

    Some(CompletionResponse::List(CompletionList {
        is_incomplete: has_dirs,
        items,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tower_lsp_server::ls_types::Position;

    use crate::config::ResolvedConfig;
    use crate::state::{
        ClientCall, FileFacts, Linked, Location as StateLocation, Method, ResolvedPath, RouteId,
        RouteRecord, UrlForSite,
    };

    fn make_route(name: &str, path: &str, method: Method, uri_str: &str) -> (RouteId, RouteRecord) {
        let uri: Uri = uri_str.parse().unwrap();
        let id = RouteId(format!("app.{name}"));
        let record = RouteRecord {
            id: id.clone(),
            ordinal: 0,
            name: name.to_owned(),
            method,
            resolved_path: ResolvedPath::Resolved(path.to_owned()),
            decorator_path: path.to_owned(),
            chain: vec![],
            handler: StateLocation {
                uri,
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
        };
        (id, record)
    }

    #[test]
    fn path_to_snippet_no_params_is_plain() {
        assert_eq!(path_to_snippet("/items"), "/items");
        assert_eq!(path_to_snippet("/api/books"), "/api/books");
    }

    #[test]
    fn path_to_snippet_single_param() {
        assert_eq!(path_to_snippet("/items/{item_id}"), "/items/${1:item_id}");
    }

    #[test]
    fn path_to_snippet_multiple_params() {
        assert_eq!(
            path_to_snippet("/users/{user_id}/posts/{post_id}"),
            "/users/${1:user_id}/posts/${2:post_id}"
        );
    }

    #[test]
    fn path_to_snippet_path_converter_uses_name_only() {
        assert_eq!(
            path_to_snippet("/files/{filepath:path}"),
            "/files/${1:filepath}"
        );
    }

    #[test]
    fn client_path_completion_offers_matching_method_routes() {
        let uri: Uri = "file:///tests/test_routes.py".parse().unwrap();
        // path_range covers the empty string literal `""` at col 14; end is exclusive
        let path_range = Range {
            start: Position::new(3, 14),
            end: Position::new(3, 16),
        };

        let mut facts = FileFacts::new(uri.clone());
        facts.client_calls.push(ClientCall {
            fixture_name: "client".to_owned(),
            method: Method::Get,
            path: String::new(),
            is_prefix: false,
            path_depth: None,
            range: Range::default(),
            path_range,
        });

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/tmp"),
        ));
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        let (id1, r1) = make_route("list_items", "/items", Method::Get, "file:///app.py");
        let (id2, r2) = make_route("get_item", "/items/{id}", Method::Get, "file:///app.py");
        let (id3, r3) = make_route("create_item", "/items", Method::Post, "file:///app.py");
        linked.route_index.insert(id1, vec![r1]);
        linked.route_index.insert(id2, vec![r2]);
        linked.route_index.insert(id3, vec![r3]); // POST — should NOT appear
        state.linked.store(Arc::new(linked));

        let resp = completion(&state, &uri, Position::new(3, 14)).unwrap();
        let items = match resp {
            CompletionResponse::List(l) => l.items,
            CompletionResponse::Array(a) => a,
        };

        assert_eq!(items.len(), 2, "only GET routes should appear");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"/items"), "plain route should appear");
        assert!(labels.contains(&"/items/{id}"), "param route should appear");

        // Param route should have SNIPPET format
        let param_item = items.iter().find(|i| i.label == "/items/{id}").unwrap();
        assert_eq!(
            param_item.insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
        if let Some(CompletionTextEdit::Edit(edit)) = &param_item.text_edit {
            assert_eq!(edit.new_text, "/items/${1:id}");
            assert_eq!(edit.range, path_range);
        } else {
            panic!("expected a TextEdit");
        }

        // Plain route should have PLAIN_TEXT format
        let plain_item = items.iter().find(|i| i.label == "/items").unwrap();
        assert_eq!(
            plain_item.insert_text_format,
            Some(InsertTextFormat::PLAIN_TEXT)
        );
    }

    // ── Template path completion (REQ-CPL-04) ─────────────────────────────────

    fn make_template_state(prefix: &str) -> (Arc<crate::state::WorkspaceState>, Uri) {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let tpl_range = Range {
            start: Position::new(3, 40),
            end: Position::new(3, 52),
        };
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: prefix.to_owned(),
            range: tpl_range,
        });

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/project"),
        ));
        state.file_facts.insert(uri.clone(), facts);
        (state, uri)
    }

    fn store_index(state: &crate::state::WorkspaceState, entries: &[(&str, &str)]) {
        use crate::state::Linked;
        let mut linked = Linked::default();
        for (k, v) in entries {
            linked
                .template_index
                .insert(k.to_string(), v.parse().unwrap());
        }
        state.linked.store(Arc::new(linked));
    }

    #[test]
    fn flat_files_complete_with_file_kind() {
        let (state, uri) = make_template_state("");
        store_index(
            &state,
            &[
                ("index.html", "file:///project/tpl/index.html"),
                ("about.html", "file:///project/tpl/about.html"),
            ],
        );

        let resp = completion(&state, &uri, Position::new(3, 41));
        let items = match resp.unwrap() {
            CompletionResponse::List(l) => l.items,
            CompletionResponse::Array(a) => a,
        };
        assert_eq!(items.len(), 2);
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert!(kinds.iter().all(|k| *k == Some(CompletionItemKind::FILE)));
    }

    #[test]
    fn directory_items_have_folder_kind_and_incomplete_list() {
        let (state, uri) = make_template_state("");
        store_index(
            &state,
            &[
                ("admin/index.html", "file:///project/tpl/admin/index.html"),
                ("admin/users.html", "file:///project/tpl/admin/users.html"),
                ("index.html", "file:///project/tpl/index.html"),
            ],
        );

        let resp = completion(&state, &uri, Position::new(3, 41)).unwrap();
        let list = match resp {
            CompletionResponse::List(l) => l,
            _ => panic!("expected list"),
        };
        assert!(
            list.is_incomplete,
            "must be incomplete when directories present"
        );
        let folder_items: Vec<_> = list
            .items
            .iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FOLDER))
            .collect();
        assert_eq!(folder_items.len(), 1, "admin/ deduped to one entry");
        assert_eq!(folder_items[0].label, "admin/");
    }

    #[test]
    fn prefix_filters_to_next_level() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        let tpl_range = Range {
            start: Position::new(3, 40),
            end: Position::new(3, 55),
        };
        let mut facts = FileFacts::new(uri.clone());
        // User has typed "admin/" so TemplateRef.path = "admin/"
        facts.templates.push(TemplateRef {
            path: "admin/".to_owned(),
            range: tpl_range,
        });
        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/project"),
        ));
        state.file_facts.insert(uri.clone(), facts);
        store_index(
            &state,
            &[
                ("admin/index.html", "file:///project/tpl/admin/index.html"),
                ("admin/users.html", "file:///project/tpl/admin/users.html"),
                ("index.html", "file:///project/tpl/index.html"), // does NOT start with "admin/"
            ],
        );

        let resp = completion(&state, &uri, Position::new(3, 46)).unwrap();
        let list = match resp {
            CompletionResponse::List(l) => l,
            _ => panic!("expected list"),
        };
        assert_eq!(list.items.len(), 2, "only admin/ entries");
        let labels: Vec<&str> = list.items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"admin/index.html"));
        assert!(labels.contains(&"admin/users.html"));
        assert!(!list.is_incomplete, "no dirs at this level → complete");
    }

    #[test]
    fn empty_index_returns_none() {
        let (state, uri) = make_template_state("");
        use crate::state::Linked;
        state.linked.store(Arc::new(Linked::default()));

        let resp = completion(&state, &uri, Position::new(3, 41));
        assert!(resp.is_none(), "no completion when index empty");
    }

    #[test]
    fn cursor_outside_template_string_returns_none() {
        let (state, uri) = make_template_state("");
        store_index(&state, &[("index.html", "file:///project/tpl/index.html")]);

        // Cursor is on a completely different line
        let resp = completion(&state, &uri, Position::new(10, 0));
        assert!(resp.is_none());
    }

    #[test]
    fn cursor_at_exclusive_end_of_string_node_returns_none() {
        // tpl_range = (3, 40)..(3, 52): tree-sitter exclusive end is col 52 (one past closing quote).
        // A cursor at col 52 is on the character AFTER the closing quote — must NOT trigger.
        let (state, uri) = make_template_state("");
        store_index(&state, &[("index.html", "file:///project/tpl/index.html")]);

        let resp = completion(&state, &uri, Position::new(3, 52));
        assert!(
            resp.is_none(),
            "col 52 is past the closing quote — must not trigger completion"
        );
    }

    #[test]
    fn text_edit_range_excludes_quotes() {
        use crate::state::TemplateRef;
        let uri: Uri = "file:///project/app/main.py".parse().unwrap();
        // String range: `"index.html"` starts at col 40, ends at col 52
        let tpl_range = Range {
            start: Position::new(3, 40),
            end: Position::new(3, 52),
        };
        let mut facts = FileFacts::new(uri.clone());
        facts.templates.push(TemplateRef {
            path: "".to_owned(),
            range: tpl_range,
        });
        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/project"),
        ));
        state.file_facts.insert(uri.clone(), facts);
        store_index(&state, &[("index.html", "file:///project/tpl/index.html")]);

        let resp = completion(&state, &uri, Position::new(3, 41)).unwrap();
        let items = match resp {
            CompletionResponse::List(l) => l.items,
            CompletionResponse::Array(a) => a,
        };
        let edit = match &items[0].text_edit {
            Some(CompletionTextEdit::Edit(e)) => e,
            _ => panic!("expected text edit"),
        };
        // Replace range should be col 41..51 (content between quotes)
        assert_eq!(edit.range.start.character, 41, "start after opening quote");
        assert_eq!(edit.range.end.character, 51, "end before closing quote");
    }

    #[test]
    fn url_for_completion_returns_all_route_names() {
        let uri: Uri = "file:///app/views.py".parse().unwrap();
        let name_range = Range {
            start: Position::new(0, 20),
            end: Position::new(0, 20),
        };
        let mut facts = FileFacts::new(uri.clone());
        facts.url_for_sites.push(UrlForSite {
            name: String::new(),
            kwarg_names: vec![],
            has_splat_kwargs: false,
            range: Range::default(),
            name_range: Some(name_range),
        });

        let state = crate::state::WorkspaceState::new(ResolvedConfig::default_for_root(
            std::path::PathBuf::from("/project"),
        ));
        state.file_facts.insert(uri.clone(), facts);

        let mut linked = Linked::default();
        let (id1, r1) = make_route("list_items", "/items", Method::Get, "file:///app.py");
        let (id2, r2) = make_route("create_item", "/items", Method::Post, "file:///app.py");
        linked.route_index.insert(id1.clone(), vec![r1]);
        linked.route_index.insert(id2.clone(), vec![r2]);
        linked
            .route_names
            .insert("list_items".to_owned(), vec![id1]);
        linked
            .route_names
            .insert("create_item".to_owned(), vec![id2]);
        state.linked.store(Arc::new(linked));

        let resp = completion(&state, &uri, Position::new(0, 20)).unwrap();
        let items = match resp {
            CompletionResponse::List(l) => l.items,
            CompletionResponse::Array(a) => a,
        };

        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"list_items"),
            "GET route name should appear"
        );
        assert!(
            labels.contains(&"create_item"),
            "POST route name should appear"
        );

        let item = items.iter().find(|i| i.label == "list_items").unwrap();
        if let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit {
            assert_eq!(edit.range, name_range);
            assert_eq!(edit.new_text, "list_items");
        } else {
            panic!("expected a TextEdit for list_items");
        }
    }
}
