use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tower_lsp_server::ls_types::{Position, Range};

use crate::features::diagnostics::{edit_distance, handler_param_range};
use crate::state::{ResolvedPath, WorkspaceState};
use crate::uri::uri_to_path;

/// A single deterministic fix for a diagnostic.
pub struct FileFix {
    /// URI of the file containing the diagnostic.
    pub uri: String,
    /// Range of the diagnostic this fix resolves (used to match diag → fix).
    pub diag_range: Range,
    /// Filesystem path for applying the edit.
    pub path: PathBuf,
    /// Range in the file to replace.
    pub edit_range: Range,
    /// Replacement text.
    pub new_text: String,
}

/// Collect all auto-fixable diagnostics from workspace state.
pub fn collect_fixes(state: &WorkspaceState) -> Vec<FileFix> {
    let mut fixes: Vec<FileFix> = Vec::new();
    let linked = state.linked.load();

    // dep_params: dep_name → param_names (needed for route/arg-missing-param bound-set)
    let mut dep_params: HashMap<String, Vec<String>> = HashMap::new();
    for fe in state.file_facts.iter() {
        for d in &fe.dep_defs {
            dep_params
                .entry(d.name.clone())
                .or_insert_with(|| d.param_names.clone());
        }
    }

    for entry in state.file_facts.iter() {
        let uri = entry.key();
        let facts = entry.value();
        let Some(path) = uri_to_path(uri) else {
            continue;
        };
        let uri_str = uri.as_str().to_owned();

        // di/depends-called: replace callee `fn()` with `fn` inside Depends(...)
        for dep_ref in &facts.dep_refs {
            if !dep_ref.is_called {
                continue;
            }
            if !linked.proven_dep_names.contains(dep_ref.name.as_str()) {
                continue;
            }
            let Some(callee_range) = dep_ref.callee_range else {
                continue;
            };
            fixes.push(FileFix {
                uri: uri_str.clone(),
                diag_range: dep_ref.range,
                path: path.clone(),
                edit_range: callee_range,
                new_text: dep_ref.name.clone(),
            });
        }

        // route/arg-missing-param: rename handler param to match path param (is_preferred fix)
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
            if record.handler_param_ranges.len() != record.handler_params.len() {
                continue;
            }

            let mut bound: HashSet<String> = record.handler_params.iter().cloned().collect();
            for dep_name in &record.dependencies {
                if let Some(params) = dep_params.get(dep_name) {
                    bound.extend(params.iter().cloned());
                }
            }

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

            let dep_contributed: HashSet<&str> = record
                .dependencies
                .iter()
                .flat_map(|dep_name| {
                    dep_params
                        .get(dep_name)
                        .into_iter()
                        .flat_map(|v| v.iter().map(|s| s.as_str()))
                })
                .collect();
            let path_param_names: HashSet<&str> =
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
                fixes.push(FileFix {
                    uri: uri_str.clone(),
                    diag_range: hp_range,
                    path: path.clone(),
                    edit_range: hp_range,
                    new_text: target_param.to_owned(),
                });
                break;
            }
        }
    }

    fixes
}

/// Apply a list of (range, new_text) edits to a file in-place.
/// Edits are applied bottom-up so earlier byte offsets are not shifted by later ones.
pub fn apply_fixes_to_file(path: &PathBuf, edits: &[(Range, String)]) -> std::io::Result<()> {
    let mut content = std::fs::read(path)?;

    let mut sorted: Vec<&(Range, String)> = edits.iter().collect();
    sorted.sort_by(|a, b| {
        b.0.start
            .line
            .cmp(&a.0.start.line)
            .then_with(|| b.0.start.character.cmp(&a.0.start.character))
    });

    for (range, new_text) in sorted {
        let start = position_to_offset(&content, range.start);
        let end = position_to_offset(&content, range.end);
        if start > content.len() || end > content.len() || start > end {
            continue;
        }
        let mut new_content = content[..start].to_vec();
        new_content.extend_from_slice(new_text.as_bytes());
        new_content.extend_from_slice(&content[end..]);
        content = new_content;
    }

    std::fs::write(path, &content)
}

/// Convert an LSP Position (UTF-8 character = byte) to a byte offset in `content`.
fn position_to_offset(content: &[u8], pos: Position) -> usize {
    let mut current_line = 0u32;
    let mut i = 0;
    while i < content.len() {
        if current_line == pos.line {
            return (i + pos.character as usize).min(content.len());
        }
        if content[i] == b'\n' {
            current_line += 1;
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Position;

    #[test]
    fn position_to_offset_first_line() {
        let content = b"hello\nworld\n";
        assert_eq!(position_to_offset(content, Position::new(0, 0)), 0);
        assert_eq!(position_to_offset(content, Position::new(0, 5)), 5);
    }

    #[test]
    fn position_to_offset_second_line() {
        let content = b"hello\nworld\n";
        assert_eq!(position_to_offset(content, Position::new(1, 0)), 6);
        assert_eq!(position_to_offset(content, Position::new(1, 3)), 9);
    }

    #[test]
    fn position_to_offset_clamps_past_end() {
        let content = b"hi";
        assert_eq!(position_to_offset(content, Position::new(0, 100)), 2);
    }

    #[test]
    fn apply_fixes_replaces_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.py");
        std::fs::write(&path, b"def foo(bar):\n    pass\n").unwrap();

        // Replace "bar" (line 0, chars 8-11) with "baz"
        let range = Range {
            start: Position::new(0, 8),
            end: Position::new(0, 11),
        };
        apply_fixes_to_file(&path, &[(range, "baz".to_owned())]).unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert_eq!(result, "def foo(baz):\n    pass\n");
    }

    #[test]
    fn apply_fixes_multiple_edits_bottom_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.py");
        std::fs::write(&path, b"ab\ncd\n").unwrap();

        let r1 = Range {
            start: Position::new(0, 0),
            end: Position::new(0, 2),
        };
        let r2 = Range {
            start: Position::new(1, 0),
            end: Position::new(1, 2),
        };
        // Apply both; order in input is top-down but should be applied bottom-up
        apply_fixes_to_file(&path, &[(r1, "XX".to_owned()), (r2, "YY".to_owned())]).unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert_eq!(result, "XX\nYY\n");
    }
}
