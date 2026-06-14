use tower_lsp_server::ls_types::{Location as LspLocation, Position, Uri};

use crate::state::WorkspaceState;

// Delegates to goto module which owns the shared edge_at + references logic
#[allow(dead_code)]
pub fn references(
    state: &WorkspaceState,
    uri: &Uri,
    pos: Position,
    include_declaration: bool,
) -> Vec<LspLocation> {
    super::goto::references(state, uri, pos, include_declaration)
}
