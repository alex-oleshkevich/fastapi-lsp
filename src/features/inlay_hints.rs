use std::collections::HashMap;

use tower_lsp_server::ls_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range, Uri};

use crate::state::{ResolvedPath, WorkspaceState};

// (start_line, start_char, end_line, end_char) — Range isn't Hash
type RangeKey = (u32, u32, u32, u32);

pub fn inlay_hints(state: &WorkspaceState, uri: &Uri, range: Range) -> Vec<InlayHint> {
    let linked = state.linked.load();

    // group by (decorator_path, handler_range_key) → (hint_pos, resolved_paths)
    let mut groups: HashMap<(String, RangeKey), (Position, Vec<String>)> = HashMap::new();

    for records in linked.route_index.values() {
        for r in records {
            if &r.handler.uri != uri {
                continue;
            }
            if !ranges_overlap(r.handler.range, range) {
                continue;
            }
            // Only hint when a prefix was applied
            let resolved_str = match &r.resolved_path {
                ResolvedPath::Resolved(p) if p != &r.decorator_path => p.clone(),
                _ => continue,
            };

            let key = (r.decorator_path.clone(), range_key(r.handler.range));
            groups
                .entry(key)
                .or_insert_with(|| (r.handler.range.end, vec![]))
                .1
                .push(resolved_str);
        }
    }

    groups
        .into_values()
        .map(|(pos, paths)| {
            let label = if paths.len() == 1 {
                format!("→ {}", paths[0])
            } else {
                format!("→ {} mounts (hover for paths)", paths.len())
            };
            InlayHint {
                position: pos,
                label: InlayHintLabel::String(label),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: Some(true),
                padding_right: None,
                data: None,
            }
        })
        .collect()
}

fn ranges_overlap(a: Range, b: Range) -> bool {
    a.start.line <= b.end.line && b.start.line <= a.end.line
}

fn range_key(r: Range) -> RangeKey {
    (r.start.line, r.start.character, r.end.line, r.end.character)
}
