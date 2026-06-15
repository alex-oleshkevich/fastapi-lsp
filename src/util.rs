use tower_lsp_server::ls_types::Position;
use tree_sitter::{Node, Point};

#[allow(dead_code)]
pub fn ts_point_to_position(p: Point) -> Position {
    Position::new(p.row as u32, p.column as u32)
}

#[allow(dead_code)]
pub fn position_to_ts_point(p: Position) -> Point {
    Point {
        row: p.line as usize,
        column: p.character as usize,
    }
}

/// Returns true if `pos` lies within `[start, end)` — end-exclusive, matching tree-sitter and LSP.
pub fn position_in_range(pos: Position, start: Position, end: Position) -> bool {
    (pos.line > start.line || (pos.line == start.line && pos.character >= start.character))
        && (pos.line < end.line || (pos.line == end.line && pos.character < end.character))
}

#[allow(dead_code)]
pub fn find_enclosing_call<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    let mut cur = node;
    loop {
        if kinds.contains(&cur.kind()) {
            return Some(cur);
        }
        cur = cur.parent()?;
    }
}

#[allow(dead_code)]
pub fn node_text<'a>(node: Node<'_>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

pub fn is_secret_key(key: &str) -> bool {
    let lower = key.to_lowercase();
    ["secret", "token", "password", "key", "credential"]
        .iter()
        .any(|s| lower.contains(s))
}

/// Extract `{name}` and `{name:converter}` path parameters from a path string.
pub fn extract_path_params(path: &str) -> Vec<crate::state::PathParam> {
    let mut params = vec![];
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        let end = match rest.find('}') {
            Some(e) => e,
            None => break,
        };
        let inner = &rest[..end];
        rest = &rest[end + 1..];

        let (name, converter) = if let Some(colon) = inner.find(':') {
            let name = inner[..colon].trim().to_owned();
            let conv_str = inner[colon + 1..].trim();
            let converter = match conv_str {
                "int" => crate::state::PathConverter::Int,
                "float" => crate::state::PathConverter::Float,
                "uuid" => crate::state::PathConverter::Uuid,
                "path" => crate::state::PathConverter::Path,
                _ => crate::state::PathConverter::Str,
            };
            (name, converter)
        } else {
            (inner.trim().to_owned(), crate::state::PathConverter::Str)
        };

        if !name.is_empty() {
            params.push(crate::state::PathParam { name, converter });
        }
    }
    params
}
