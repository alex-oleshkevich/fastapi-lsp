use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp_server::ls_types::DiagnosticSeverity;

use crate::cli::{CheckArgs, OutputFormat};
use crate::{config, linking, state::WorkspaceState};

/// Exit codes:
/// 0 — clean (no Warning/Error diagnostics)
/// 1 — at least one Warning or Error diagnostic emitted
/// 2 — usage or config error
pub async fn run(args: CheckArgs) -> i32 {
    if !args.path.exists() {
        eprintln!("error: path does not exist: {}", args.path.display());
        return 2;
    }

    let root = find_workspace_root(&args.path);
    let cfg = config::load(&root, None);
    let env_ignore = cfg.env_ignore.clone();
    let state = WorkspaceState::new(cfg);

    scan(&state, &root).await;
    linking::relink(&state).await;

    let only_codes: Vec<&str> = args.only.iter().map(|c| c.0.as_str()).collect();
    let ignore_codes: Vec<&str> = args.ignore.iter().map(|c| c.0.as_str()).collect();

    let mut all_diags: Vec<(String, tower_lsp_server::ls_types::Diagnostic)> = vec![];
    let target_uri_prefix = crate::uri::path_to_uri(&args.path).map(|u| u.as_str().to_owned());

    for entry in state.file_facts.iter() {
        let uri = entry.key().clone();

        // When PATH is a single file, only report diagnostics for that file.
        if let Some(ref prefix) = target_uri_prefix
            && args.path.is_file()
            && uri.as_str() != prefix.as_str()
        {
            continue;
        }

        let diags = crate::features::diagnostics::compute(&state, &uri, &env_ignore);
        for d in diags {
            let code = match &d.code {
                Some(tower_lsp_server::ls_types::NumberOrString::String(s)) => s.as_str(),
                _ => "",
            };
            if !only_codes.is_empty() && !only_codes.contains(&code) {
                continue;
            }
            if ignore_codes.contains(&code) {
                continue;
            }
            all_diags.push((uri.as_str().to_owned(), d));
        }
    }

    let has_findings = all_diags.iter().any(|(_, d)| {
        matches!(
            d.severity,
            Some(DiagnosticSeverity::ERROR) | Some(DiagnosticSeverity::WARNING)
        )
    });

    match args.format {
        OutputFormat::Json => print_json(&all_diags),
        OutputFormat::Text => print_text(&all_diags),
    }

    if has_findings { 1 } else { 0 }
}

fn print_text(diags: &[(String, tower_lsp_server::ls_types::Diagnostic)]) {
    use std::io::IsTerminal;
    let color = std::io::stdout().is_terminal();

    let mut errors: u32 = 0;
    let mut warnings: u32 = 0;

    for (uri, d) in diags {
        let path = uri_to_display_path(uri);
        let code = code_str(&d.code);
        match d.severity {
            Some(DiagnosticSeverity::ERROR) => errors += 1,
            Some(DiagnosticSeverity::WARNING) => warnings += 1,
            _ => {}
        }
        if color {
            let sev_color = severity_color(d.severity);
            println!(
                "\x1b[1m{}\x1b[0m:{}:{}: {}{}\x1b[0m {}",
                path,
                d.range.start.line + 1,
                d.range.start.character + 1,
                sev_color,
                code,
                d.message,
            );
        } else {
            println!(
                "{}:{}:{}: {} {}",
                path,
                d.range.start.line + 1,
                d.range.start.character + 1,
                code,
                d.message,
            );
        }
        if let Some(related) = &d.related_information {
            for rel in related {
                let rel_path = uri_to_display_path(rel.location.uri.as_str());
                println!(
                    "  --> {}:{}:{}",
                    rel_path,
                    rel.location.range.start.line + 1,
                    rel.location.range.start.character + 1,
                );
            }
        }
    }

    let summary = if errors == 0 && warnings == 0 && diags.is_empty() {
        "All checks passed.".to_owned()
    } else if errors > 0 && warnings > 0 {
        format!(
            "Found {} error{} and {} warning{}.",
            errors,
            if errors == 1 { "" } else { "s" },
            warnings,
            if warnings == 1 { "" } else { "s" },
        )
    } else if errors > 0 {
        format!(
            "Found {} error{}.",
            errors,
            if errors == 1 { "" } else { "s" }
        )
    } else if warnings > 0 {
        format!(
            "Found {} warning{}.",
            warnings,
            if warnings == 1 { "" } else { "s" }
        )
    } else {
        let n = diags.len();
        format!("Found {} notice{}.", n, if n == 1 { "" } else { "s" })
    };

    if color {
        let summary_color = if errors > 0 {
            "\x1b[1;31m"
        } else if warnings > 0 {
            "\x1b[1;33m"
        } else {
            "\x1b[1;32m"
        };
        eprintln!("{}{}\x1b[0m", summary_color, summary);
    } else {
        eprintln!("{}", summary);
    }
}

fn severity_color(sev: Option<DiagnosticSeverity>) -> &'static str {
    match sev {
        Some(DiagnosticSeverity::ERROR) => "\x1b[1;31m",
        Some(DiagnosticSeverity::WARNING) => "\x1b[1;33m",
        Some(DiagnosticSeverity::INFORMATION) => "\x1b[36m",
        Some(DiagnosticSeverity::HINT) => "\x1b[34m",
        _ => "",
    }
}

fn print_json(diags: &[(String, tower_lsp_server::ls_types::Diagnostic)]) {
    for (uri, d) in diags {
        let related: Vec<serde_json::Value> = d
            .related_information
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|r| {
                serde_json::json!({
                    "uri": r.location.uri.as_str(),
                    "range": {
                        "start": { "line": r.location.range.start.line, "character": r.location.range.start.character },
                        "end": { "line": r.location.range.end.line, "character": r.location.range.end.character },
                    },
                    "message": r.message,
                })
            })
            .collect();

        let obj = serde_json::json!({
            "file": uri,
            "range": {
                "start": { "line": d.range.start.line, "character": d.range.start.character },
                "end": { "line": d.range.end.line, "character": d.range.end.character },
            },
            "severity": severity_str(d.severity),
            "code": code_str(&d.code),
            "message": d.message,
            "related": related,
        });
        println!("{}", serde_json::to_string(&obj).unwrap_or_default());
    }
}

fn severity_str(sev: Option<DiagnosticSeverity>) -> &'static str {
    match sev {
        Some(DiagnosticSeverity::ERROR) => "error",
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "info",
        Some(DiagnosticSeverity::HINT) => "hint",
        _ => "note",
    }
}

fn code_str(code: &Option<tower_lsp_server::ls_types::NumberOrString>) -> &str {
    match code {
        Some(tower_lsp_server::ls_types::NumberOrString::String(s)) => s.as_str(),
        _ => "",
    }
}

pub fn uri_to_display_path(uri: &str) -> String {
    let abs = uri.strip_prefix("file://").unwrap_or(uri);
    if let Ok(cwd) = std::env::current_dir()
        && let Some(cwd_str) = cwd.to_str()
    {
        let prefix = format!("{}/", cwd_str);
        if let Some(rel) = abs.strip_prefix(prefix.as_str()) {
            return rel.to_owned();
        }
    }
    abs.to_owned()
}

/// Locate the workspace root: nearest ancestor that has pyproject.toml or .git,
/// or the path itself if it's a directory.
pub fn find_workspace_root(path: &std::path::Path) -> PathBuf {
    let dir = if path.is_file() {
        path.parent().unwrap_or(path).to_path_buf()
    } else {
        path.to_path_buf()
    };
    let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    let mut cursor = canonical.as_path();
    loop {
        if cursor.join("pyproject.toml").exists() || cursor.join(".git").exists() {
            return cursor.to_path_buf();
        }
        match cursor.parent() {
            Some(p) => cursor = p,
            None => return canonical,
        }
    }
}

pub async fn scan(state: &Arc<WorkspaceState>, root: &std::path::Path) {
    let client_fixtures = state.config.read().await.client_fixtures.clone();
    let enc = crate::offset::Encoding::Utf8;
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            !e.path().components().any(|c| {
                matches!(
                    c.as_os_str().to_str(),
                    Some(".venv") | Some("__pycache__") | Some(".git")
                )
            })
        })
    {
        let path = entry.path();
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
        let filename = path.file_name().and_then(|x| x.to_str()).unwrap_or("");

        if ext == "py" {
            if let Ok(bytes) = std::fs::read(path) {
                if !crate::server::has_indicators(&bytes) {
                    continue;
                }
                if let Some(uri) = crate::uri::path_to_uri(path) {
                    let is_test = crate::server::is_test_file(&uri);
                    let tree = crate::parsing::parse_file(&bytes);
                    let facts = crate::server::extract_all_facts(
                        &bytes,
                        &tree,
                        &uri,
                        is_test,
                        &client_fixtures,
                        enc,
                    );
                    state.file_facts.insert(uri, facts);
                    state.bump_generation();
                }
            }
        } else if crate::server::is_env_filename(filename)
            && let Ok(src) = std::fs::read_to_string(path)
                && let Some(uri) = crate::uri::path_to_uri(path)
            {
                let entries = crate::parsing::dotenv::parse(&src, &uri);
                state.env_file_entries.insert(uri, entries);
                state.bump_generation();
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::{
        Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range,
    };

    fn make_diag(sev: DiagnosticSeverity, code: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: Position::new(0, 0),
                end: Position::new(0, 10),
            },
            severity: Some(sev),
            code: Some(NumberOrString::String(code.to_owned())),
            source: Some("fastapi-lsp".to_owned()),
            message: format!("test diagnostic for {code}"),
            ..Default::default()
        }
    }

    #[test]
    fn severity_str_mapping() {
        assert_eq!(severity_str(Some(DiagnosticSeverity::ERROR)), "error");
        assert_eq!(severity_str(Some(DiagnosticSeverity::WARNING)), "warning");
        assert_eq!(severity_str(Some(DiagnosticSeverity::INFORMATION)), "info");
        assert_eq!(severity_str(Some(DiagnosticSeverity::HINT)), "hint");
        assert_eq!(severity_str(None), "note");
    }

    #[test]
    fn code_str_extracts_string_code() {
        let d = make_diag(DiagnosticSeverity::WARNING, "route/duplicate");
        assert_eq!(code_str(&d.code), "route/duplicate");
    }

    #[test]
    fn code_str_empty_for_number_code() {
        let code = Some(NumberOrString::Number(42));
        assert_eq!(code_str(&code), "");
    }

    #[test]
    fn uri_to_display_path_strips_file_prefix() {
        assert_eq!(
            uri_to_display_path("file:///project/app/main.py"),
            "/project/app/main.py"
        );
    }

    #[test]
    fn has_findings_true_for_warning() {
        let diags = [(
            "file:///a.py".to_owned(),
            make_diag(DiagnosticSeverity::WARNING, "route/duplicate"),
        )];
        let has = diags.iter().any(|(_, d)| {
            matches!(
                d.severity,
                Some(DiagnosticSeverity::ERROR) | Some(DiagnosticSeverity::WARNING)
            )
        });
        assert!(has);
    }

    #[test]
    fn has_findings_false_for_info_only() {
        let diags = [(
            "file:///a.py".to_owned(),
            make_diag(DiagnosticSeverity::INFORMATION, "env/undefined-key"),
        )];
        let has = diags.iter().any(|(_, d)| {
            matches!(
                d.severity,
                Some(DiagnosticSeverity::ERROR) | Some(DiagnosticSeverity::WARNING)
            )
        });
        assert!(!has, "Info-only diags should not set has_findings");
    }

    #[test]
    fn has_findings_false_for_hint_only() {
        let diags = [(
            "file:///a.py".to_owned(),
            make_diag(DiagnosticSeverity::HINT, "route/arg-missing-param"),
        )];
        let has = diags.iter().any(|(_, d)| {
            matches!(
                d.severity,
                Some(DiagnosticSeverity::ERROR) | Some(DiagnosticSeverity::WARNING)
            )
        });
        assert!(!has, "Hint-only diags should not set has_findings");
    }

    #[test]
    fn json_output_is_one_object_per_line() {
        // Capture is hard in unit tests; verify the structure is valid JSON.
        let diags = vec![
            (
                "file:///a.py".to_owned(),
                make_diag(DiagnosticSeverity::WARNING, "route/duplicate"),
            ),
            (
                "file:///b.py".to_owned(),
                make_diag(DiagnosticSeverity::ERROR, "di/cycle"),
            ),
        ];
        // Build the JSON manually for each entry and verify it parses.
        for (uri, d) in &diags {
            let related: Vec<serde_json::Value> = vec![];
            let obj = serde_json::json!({
                "file": uri,
                "range": {
                    "start": { "line": d.range.start.line, "character": d.range.start.character },
                    "end": { "line": d.range.end.line, "character": d.range.end.character },
                },
                "severity": severity_str(d.severity),
                "code": code_str(&d.code),
                "message": d.message,
                "related": related,
            });
            let line = serde_json::to_string(&obj).unwrap();
            // Each line is a single, compact JSON object (no newlines inside)
            assert!(
                !line.contains('\n'),
                "JSON output must be single-line per entry"
            );
            // Parses back correctly
            let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(parsed["code"].as_str().unwrap(), code_str(&d.code));
        }
    }
}
