use zed_extension_api::{
    self as zed, serde_json, settings::LspSettings, LanguageServerId, Result,
};

const SERVER_NAME: &str = "fastapi-lsp";

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
        let env = worktree.shell_env();

        if let Ok(lsp_settings) = LspSettings::for_worktree(SERVER_NAME, worktree) {
            if let Some(binary) = lsp_settings.binary {
                if let Some(path) = binary.path {
                    let args = binary.arguments.unwrap_or_else(|| vec!["lsp".to_string()]);
                    return Ok(zed::Command { command: path, args, env });
                }
            }
        }

        let binary = worktree
            .which(SERVER_NAME)
            .ok_or_else(|| {
                format!("{SERVER_NAME} not found. Download from: https://github.com/alex-oleshkevich/fastapi-lsp/releases")
            })?;
        Ok(zed::Command {
            command: binary,
            args: vec!["lsp".to_owned()],
            env,
        })
    }

    fn language_server_initialization_options(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|s| s.initialization_options.clone())
            .unwrap_or_default();
        Ok(Some(settings))
    }

    fn language_server_workspace_configuration(
        &mut self,
        language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        let settings = LspSettings::for_worktree(language_server_id.as_ref(), worktree)
            .ok()
            .and_then(|s| s.settings.clone())
            .unwrap_or_default();
        Ok(Some(settings))
    }
}

zed::register_extension!(FastApiLspExtension);
