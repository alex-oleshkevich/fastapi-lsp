use notify::{EventKind, RecursiveMode, Watcher};
use std::sync::Arc;
use tower_lsp_server::ls_types::FileChangeType;

use crate::state::WorkspaceState;

/// Start a native file watcher via the `notify` crate.
/// Called when the client doesn't support dynamic `didChangeWatchedFiles` registration.
#[allow(dead_code)]
pub fn start(
    state: Arc<WorkspaceState>,
    tx_to_server: tokio::sync::mpsc::UnboundedSender<FileEvent>,
) {
    let root = {
        // Can't .await here (sync context); state.config must be pre-loaded
        // The workspace root is read from the already-resolved config snapshot.

        state
            .config
            .try_read()
            .ok()
            .map(|c| c.workspace_root.clone())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    };

    std::thread::spawn(move || {
        let (event_tx, event_rx) = std::sync::mpsc::channel();

        let mut watcher = match notify::recommended_watcher(move |res| {
            if let Ok(event) = res {
                let _ = event_tx.send(event);
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("notify watcher init failed: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
            tracing::error!("notify watch failed on {}: {e}", root.display());
            return;
        }

        tracing::debug!("notify watcher started on {}", root.display());

        for event in event_rx {
            let typ = match event.kind {
                EventKind::Create(_) => FileChangeType::CREATED,
                EventKind::Modify(_) => FileChangeType::CHANGED,
                EventKind::Remove(_) => FileChangeType::DELETED,
                _ => continue,
            };

            for path in &event.paths {
                let ext = path.extension().and_then(|e| e.to_str());
                let name = path.file_name().and_then(|n| n.to_str());
                let relevant = matches!(ext, Some("py") | Some("toml") | Some("env"))
                    || matches!(
                        name,
                        Some(".env") | Some("fastapi-lsp.toml") | Some("pyproject.toml")
                    );
                if !relevant {
                    continue;
                }
                let _ = tx_to_server.send(FileEvent {
                    path: path.clone(),
                    typ,
                });
            }
        }
    });
}

#[allow(dead_code)]
pub struct FileEvent {
    pub path: std::path::PathBuf,
    pub typ: FileChangeType,
}
