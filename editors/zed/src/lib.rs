use zed_extension_api::{self as zed, LanguageServerId, Result};

struct FastApiLspExtension;

impl zed::Extension for FastApiLspExtension {
    fn new() -> Self {
        FastApiLspExtension
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // First try PATH (works when Zed is launched from a terminal with ~/.cargo/bin on PATH).
        // Fall back to the canonical cargo bin location for GUI launches.
        let binary = worktree
            .which("fastapi-lsp")
            .or_else(|| {
                let home = std::env::var("HOME").ok()?;
                let p = format!("{home}/.cargo/bin/fastapi-lsp");
                std::fs::metadata(&p).ok().map(|_| p)
            })
            .ok_or_else(|| {
                "fastapi-lsp not found. Install with: cargo install fastapi-lsp".to_owned()
            })?;
        Ok(zed::Command {
            command: binary,
            args: vec!["--stdio".to_owned()],
            env: Default::default(),
        })
    }
}

zed::register_extension!(FastApiLspExtension);
