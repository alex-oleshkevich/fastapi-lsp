use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RawConfig {
    pub entrypoint: Option<String>,
    pub templates: Vec<String>,
    pub source_roots: Vec<String>,
    pub env_files: Vec<String>,
    /// Env file basenames checked for required BaseSettings fields (settings/env-key-missing).
    pub settings_env_files: Vec<String>,
    pub process_env: bool,
    pub client_fixtures: Vec<String>,
    #[serde(rename = "env")]
    pub env: EnvConfig,
    pub features: Option<FeatureToggles>,
    pub check: Option<CheckDefaults>,
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            entrypoint: None,
            templates: vec![],
            source_roots: vec![],
            env_files: vec![".env".into(), ".env.example".into()],
            settings_env_files: vec![".env".into(), ".env.example".into(), ".env.unittest".into()],
            process_env: false,
            client_fixtures: vec!["client".into(), "async_client".into()],
            env: EnvConfig::default(),
            features: None,
            check: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvConfig {
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureToggles {
    pub diagnostics: bool,
    pub completion: bool,
    pub hover: bool,
    pub code_actions: bool,
    pub inlay_hints: bool,
    pub code_lens: bool,
    pub symbols: bool,
    pub navigation: bool,
    pub document_links: bool,
}

impl Default for FeatureToggles {
    fn default() -> Self {
        Self {
            diagnostics: true,
            completion: true,
            hover: true,
            code_actions: true,
            inlay_hints: true,
            code_lens: true,
            symbols: true,
            navigation: true,
            document_links: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckDefaults {
    pub only: Vec<String>,
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResolvedConfig {
    pub workspace_root: PathBuf,
    pub entrypoint: Option<PathBuf>,
    pub template_roots: Vec<PathBuf>,
    pub source_roots: Vec<PathBuf>,
    pub env_files: Vec<PathBuf>,
    /// Basenames of env files that must declare required BaseSettings fields.
    pub settings_env_files: Vec<String>,
    pub process_env: bool,
    pub client_fixtures: Vec<String>,
    pub env_ignore: Vec<String>,
    pub features: FeatureToggles,
    pub check: CheckDefaults,
}

impl ResolvedConfig {
    pub fn default_for_root(root: PathBuf) -> Self {
        Self {
            workspace_root: root,
            entrypoint: None,
            template_roots: vec![],
            source_roots: vec![],
            env_files: vec![".env".into(), ".env.example".into()],
            settings_env_files: vec![".env".into(), ".env.example".into(), ".env.unittest".into()],
            process_env: false,
            client_fixtures: vec!["client".into(), "async_client".into()],
            env_ignore: vec![],
            features: FeatureToggles::default(),
            check: CheckDefaults::default(),
        }
    }
}

pub fn load(workspace_root: &Path, init_options: Option<serde_json::Value>) -> ResolvedConfig {
    let mut raw = RawConfig::default();

    // pyproject.toml — read [tool.fastapi-lsp] subsection; also harvest
    // [tool.fastapi].entrypoint as a final fallback (REQ-CFG-02).
    // Applied first so fastapi-lsp.toml (applied second) can override it.
    let pyproject_path = workspace_root.join("pyproject.toml");
    if pyproject_path.exists() {
        match std::fs::read_to_string(&pyproject_path)
            .map_err(|e| e.to_string())
            .and_then(|s| toml::from_str::<toml::Value>(&s).map_err(|e| e.to_string()))
        {
            Ok(doc) => {
                // [tool.fastapi-lsp] section
                if let Some(section) = doc.get("tool").and_then(|t| t.get("fastapi-lsp")) {
                    match toml::Value::try_into::<RawConfig>(section.clone()) {
                        Ok(file_cfg) => merge(&mut raw, file_cfg),
                        Err(e) => tracing::warn!(
                            "config parse error in [tool.fastapi-lsp] in {}: {e}",
                            pyproject_path.display()
                        ),
                    }
                }
                // [tool.fastapi].entrypoint fallback (REQ-CFG-02)
                if raw.entrypoint.is_none()
                    && let Some(ep) = doc
                        .get("tool")
                        .and_then(|t| t.get("fastapi"))
                        .and_then(|f| f.get("entrypoint"))
                        .and_then(|v| v.as_str())
                    {
                        raw.entrypoint = Some(ep.to_owned());
                    }
            }
            Err(e) => tracing::warn!("config parse error in {}: {e}", pyproject_path.display()),
        }
    }

    // fastapi-lsp.toml (flat schema) — applied after pyproject.toml so it takes precedence.
    let own_cfg_path = workspace_root.join("fastapi-lsp.toml");
    if own_cfg_path.exists() {
        match std::fs::read_to_string(&own_cfg_path)
            .map_err(|e| e.to_string())
            .and_then(|s| toml::from_str::<RawConfig>(&s).map_err(|e| e.to_string()))
        {
            Ok(file_cfg) => merge(&mut raw, file_cfg),
            Err(e) => tracing::warn!("config parse error in {}: {e}", own_cfg_path.display()),
        }
    }

    // InitializationOptions / didChangeConfiguration (highest precedence, REQ-CFG-04/06)
    if let Some(opts) = init_options {
        match serde_json::from_value::<RawConfig>(opts) {
            Ok(session_cfg) => merge(&mut raw, session_cfg),
            Err(e) => tracing::warn!("initializationOptions parse error: {e}"),
        }
    }

    resolve(workspace_root, raw)
}

fn merge(base: &mut RawConfig, over: RawConfig) {
    if over.entrypoint.is_some() { base.entrypoint = over.entrypoint; }
    if !over.templates.is_empty() { base.templates = over.templates; }
    if !over.source_roots.is_empty() { base.source_roots = over.source_roots; }
    if !over.env_files.is_empty() { base.env_files = over.env_files; }
    if !over.settings_env_files.is_empty() { base.settings_env_files = over.settings_env_files; }
    // process_env is intentionally sticky: once any source enables it, it stays on.
    // A higher-precedence source cannot disable it; use InitializationOptions to force off.
    if over.process_env { base.process_env = true; }
    if !over.client_fixtures.is_empty() { base.client_fixtures = over.client_fixtures; }
    if !over.env.ignore.is_empty() { base.env.ignore = over.env.ignore; }
    if over.features.is_some() { base.features = over.features; }
    if let Some(c) = over.check {
        let base_c = base.check.get_or_insert_with(CheckDefaults::default);
        if !c.only.is_empty() { base_c.only = c.only; }
        if !c.ignore.is_empty() { base_c.ignore = c.ignore; }
    }
}

/// Join `p` onto `root` only if `p` is relative and contains no `..` components.
/// Absolute paths and parent-directory escapes are rejected (path traversal mitigation).
pub(crate) fn safe_join(root: &Path, p: &str) -> Option<PathBuf> {
    let path = std::path::Path::new(p);
    if path.is_absolute()
        || path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        tracing::warn!("config: rejecting unsafe path: {p}");
        return None;
    }
    Some(root.join(p))
}

fn resolve(root: &Path, raw: RawConfig) -> ResolvedConfig {
    let infer_roots = {
        let mut v = vec![root.to_owned()];
        let src = root.join("src");
        if src.is_dir() { v.push(src); }
        for r in pyproject_source_roots(root) {
            if !v.contains(&r) {
                v.push(r);
            }
        }
        v
    };

    ResolvedConfig {
        workspace_root: root.to_owned(),
        entrypoint: raw.entrypoint.and_then(|p| safe_join(root, &p)),
        template_roots: if raw.templates.is_empty() {
            auto_detect_templates(root)
        } else {
            raw.templates.iter().filter_map(|p| safe_join(root, p)).collect()
        },
        source_roots: if raw.source_roots.is_empty() {
            infer_roots
        } else {
            raw.source_roots.iter().filter_map(|p| safe_join(root, p)).collect()
        },
        env_files: raw.env_files.iter().filter_map(|p| safe_join(root, p)).collect(),
        settings_env_files: raw.settings_env_files,
        process_env: raw.process_env,
        client_fixtures: raw.client_fixtures,
        env_ignore: raw.env.ignore,
        features: raw.features.unwrap_or_default(),
        check: raw.check.unwrap_or_default(),
    }
}

fn auto_detect_templates(root: &Path) -> Vec<PathBuf> {
    let t = root.join("templates");
    if t.is_dir() { vec![t] } else { vec![] }
}

/// Extract declared source roots from `pyproject.toml` packaging metadata.
/// Handles setuptools, Hatch, and PDM conventions. Returns only directories
/// that actually exist on disk (callers must still deduplicate vs. workspace root).
fn pyproject_source_roots(workspace_root: &Path) -> Vec<PathBuf> {
    let content = match std::fs::read_to_string(workspace_root.join("pyproject.toml")) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let doc: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    parse_pyproject_roots(&doc, workspace_root)
}

/// Pure parsing step (testable without a real filesystem).
fn parse_pyproject_roots(doc: &toml::Value, workspace_root: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![];

    let tool = match doc.get("tool") {
        Some(t) => t,
        None => return roots,
    };

    // [tool.setuptools.package-dir] "" = "src"
    if let Some(pkg_dir) = tool
        .get("setuptools")
        .and_then(|s| s.get("package-dir"))
        .and_then(|d| d.as_table())
        && let Some(src) = pkg_dir.get("").and_then(|v| v.as_str()) {
            let p = workspace_root.join(src);
            if p.is_dir() && !roots.contains(&p) {
                roots.push(p);
            }
        }

    // [tool.hatch.build.targets.wheel] sources = [{from = "src", ...}]
    if let Some(sources) = tool
        .get("hatch")
        .and_then(|h| h.get("build"))
        .and_then(|b| b.get("targets"))
        .and_then(|t| t.get("wheel"))
        .and_then(|w| w.get("sources"))
        .and_then(|s| s.as_array())
    {
        for item in sources {
            if let Some(from) = item.get("from").and_then(|v| v.as_str()) {
                let p = workspace_root.join(from);
                if p.is_dir() && !roots.contains(&p) {
                    roots.push(p);
                }
            }
        }
    }

    // [tool.pdm.build] package-dir = "src"
    if let Some(pkg_dir) = tool
        .get("pdm")
        .and_then(|p| p.get("build"))
        .and_then(|b| b.get("package-dir"))
        .and_then(|v| v.as_str())
    {
        let p = workspace_root.join(pkg_dir);
        if p.is_dir() && !roots.contains(&p) {
            roots.push(p);
        }
    }

    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_src_dir(f: impl FnOnce(&Path)) {
        let base = std::env::temp_dir().join(format!(
            "fastapi-lsp-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(base.join("src")).unwrap();
        f(&base);
        let _ = std::fs::remove_dir_all(&base);
    }

    fn parse(toml_str: &str, root: &Path) -> Vec<PathBuf> {
        let doc: toml::Value = toml::from_str(toml_str).unwrap();
        parse_pyproject_roots(&doc, root)
    }

    #[test]
    fn setuptools_package_dir_src() {
        with_src_dir(|dir| {
            let toml = r#"[tool.setuptools.package-dir]
"" = "src"
"#;
            let roots = parse(toml, dir);
            assert_eq!(roots, vec![dir.join("src")]);
        });
    }

    #[test]
    fn hatch_wheel_sources() {
        with_src_dir(|dir| {
            let toml = r#"[tool.hatch.build.targets.wheel]
sources = [{include = "mypackage", from = "src"}]
"#;
            let roots = parse(toml, dir);
            assert_eq!(roots, vec![dir.join("src")]);
        });
    }

    #[test]
    fn pdm_build_package_dir() {
        with_src_dir(|dir| {
            let toml = r#"[tool.pdm.build]
package-dir = "src"
"#;
            let roots = parse(toml, dir);
            assert_eq!(roots, vec![dir.join("src")]);
        });
    }

    #[test]
    fn nonexistent_dir_not_returned() {
        // temp dir without src/ subdirectory
        let base = std::env::temp_dir().join(format!(
            "fastapi-lsp-nosrc-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let toml = r#"[tool.setuptools.package-dir]
"" = "src"
"#;
        let roots = parse(toml, &base);
        let _ = std::fs::remove_dir_all(&base);
        assert!(roots.is_empty());
    }

    #[test]
    fn no_tool_section_returns_empty() {
        let base = std::env::temp_dir();
        let roots = parse("[project]\nname = \"myapp\"\n", &base);
        assert!(roots.is_empty());
    }

    // ── Config resolution tests ───────────────────────────────────────────────

    fn tmp_dir() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "fastapi-lsp-cfg-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn pyproject_tool_fastapi_lsp_section_is_read() {
        let dir = tmp_dir();
        std::fs::write(dir.join("pyproject.toml"), r#"
[project]
name = "myapp"

[tool.fastapi-lsp]
templates = ["app/templates"]
"#).unwrap();
        let cfg = load(&dir, None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(cfg.template_roots, vec![dir.join("app/templates")]);
    }

    #[test]
    fn fastapi_lsp_toml_takes_precedence_over_pyproject() {
        let dir = tmp_dir();
        std::fs::write(dir.join("fastapi-lsp.toml"), "templates = [\"own/tpl\"]\n").unwrap();
        std::fs::write(dir.join("pyproject.toml"), r#"
[tool.fastapi-lsp]
templates = ["pyproject/tpl"]
"#).unwrap();
        let cfg = load(&dir, None);
        let _ = std::fs::remove_dir_all(&dir);
        // fastapi-lsp.toml sets templates, merge overwrites pyproject value
        assert_eq!(cfg.template_roots, vec![dir.join("own/tpl")]);
    }

    #[test]
    fn tool_fastapi_entrypoint_used_as_fallback() {
        let dir = tmp_dir();
        std::fs::write(dir.join("pyproject.toml"), r#"
[tool.fastapi]
entrypoint = "app/main.py"
"#).unwrap();
        let cfg = load(&dir, None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(cfg.entrypoint, Some(dir.join("app/main.py")));
    }

    #[test]
    fn init_options_entrypoint_overrides_tool_fastapi() {
        let dir = tmp_dir();
        std::fs::write(dir.join("pyproject.toml"), r#"
[tool.fastapi]
entrypoint = "app/main.py"
"#).unwrap();
        let opts = serde_json::json!({ "entrypoint": "override/main.py" });
        let cfg = load(&dir, Some(opts));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(cfg.entrypoint, Some(dir.join("override/main.py")));
    }

    #[test]
    fn malformed_pyproject_tool_section_degrades_to_defaults() {
        let dir = tmp_dir();
        std::fs::write(dir.join("pyproject.toml"), r#"
[tool.fastapi-lsp]
templates = "not-a-list"
"#).unwrap();
        // Should not panic; templates defaults to auto-detection
        let cfg = load(&dir, None);
        let _ = std::fs::remove_dir_all(&dir);
        // Auto-detection finds no templates/ dir → empty list
        assert!(cfg.template_roots.is_empty());
    }

    #[test]
    fn fastapi_lsp_entrypoint_takes_precedence_over_tool_fastapi_entrypoint() {
        let dir = tmp_dir();
        std::fs::write(dir.join("pyproject.toml"), r#"
[tool.fastapi-lsp]
entrypoint = "lsp/entry.py"

[tool.fastapi]
entrypoint = "should/not/win.py"
"#).unwrap();
        let cfg = load(&dir, None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(cfg.entrypoint, Some(dir.join("lsp/entry.py")));
    }

    #[test]
    fn templates_fallback_to_workspace_root_templates_dir() {
        let dir = tmp_dir();
        std::fs::create_dir(dir.join("templates")).unwrap();
        let cfg = load(&dir, None);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(cfg.template_roots, vec![dir.join("templates")]);
    }

    // ── safe_join path-traversal tests ───────────────────────────────────────

    #[test]
    fn safe_join_relative_path_is_accepted() {
        let root = Path::new("/workspace");
        assert_eq!(safe_join(root, "app/templates"), Some(PathBuf::from("/workspace/app/templates")));
    }

    #[test]
    fn safe_join_absolute_path_is_rejected() {
        let root = Path::new("/workspace");
        assert!(safe_join(root, "/etc/passwd").is_none());
        assert!(safe_join(root, "/workspace/templates").is_none()); // absolute even if under root
    }

    #[test]
    fn safe_join_parent_component_is_rejected() {
        let root = Path::new("/workspace");
        assert!(safe_join(root, "../secrets/.env").is_none());
        assert!(safe_join(root, "app/../../etc/shadow").is_none());
        assert!(safe_join(root, "subdir/../../../other").is_none());
    }

    #[test]
    fn safe_join_dotdot_in_template_config_is_rejected() {
        let dir = tmp_dir();
        let opts = serde_json::json!({ "templates": ["../../malicious"] });
        let cfg = load(&dir, Some(opts));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(cfg.template_roots.is_empty(), "path escape must be rejected");
    }

    #[test]
    fn safe_join_absolute_entrypoint_in_init_options_is_rejected() {
        let dir = tmp_dir();
        let opts = serde_json::json!({ "entrypoint": "/etc/passwd" });
        let cfg = load(&dir, Some(opts));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(cfg.entrypoint.is_none(), "absolute entrypoint must be rejected");
    }
}
