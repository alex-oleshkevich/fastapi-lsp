use crate::check::scan;
use crate::cli::{OutputFormat, RoutesArgs};
use crate::state::{ResolvedPath, RouteRecord, WorkspaceState};
use crate::{config, linking};

/// Exit codes:
/// 0 — success
/// 2 — usage or config error
pub async fn run(args: RoutesArgs) -> i32 {
    if !args.path.exists() {
        eprintln!("error: path does not exist: {}", args.path.display());
        return 2;
    }

    let root = crate::check::find_workspace_root(&args.path);
    let cfg = config::load(&root, None);
    let state = WorkspaceState::new(cfg);

    scan(&state, &root).await;
    linking::relink(&state).await;

    let linked = state.linked.load();
    let mut records: Vec<&RouteRecord> = linked
        .route_index
        .values()
        .flat_map(|v| v.iter())
        .collect();

    records.sort_by(|a, b| a.ordinal.cmp(&b.ordinal).then_with(|| a.name.cmp(&b.name)));

    match args.format {
        OutputFormat::Text => print_text(&records),
        OutputFormat::Json => print_json(&records),
    }

    0
}

fn display_path(record: &RouteRecord) -> &str {
    match &record.resolved_path {
        ResolvedPath::Resolved(p) => p.as_str(),
        ResolvedPath::Unresolved => &record.decorator_path,
    }
}

fn handler_location(record: &RouteRecord) -> String {
    let path = record
        .handler
        .uri
        .as_str()
        .strip_prefix("file://")
        .unwrap_or(record.handler.uri.as_str());
    format!("{}:{}", path, record.handler.range.start.line + 1)
}

fn print_text(records: &[&RouteRecord]) {
    for r in records {
        println!(
            "{:<9} {:<40} {:<30} {}",
            format!("{}", r.method),
            display_path(r),
            r.name,
            handler_location(r),
        );
    }
}

fn print_json(records: &[&RouteRecord]) {
    for r in records {
        let obj = serde_json::json!({
            "method": format!("{}", r.method),
            "path": display_path(r),
            "name": r.name,
            "handler": r.handler.uri.as_str(),
            "line": r.handler.range.start.line + 1,
        });
        println!("{obj}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Location, Method, ResolvedPath, RouteId, RouteRecord};
    use tower_lsp_server::ls_types::{Position, Range, Uri};

    fn make_record(
        ordinal: u32,
        method: Method,
        path: &str,
        name: &str,
        uri_str: &str,
        line: u32,
    ) -> RouteRecord {
        let uri: Uri = format!("file://{uri_str}").parse().unwrap();
        RouteRecord {
            id: RouteId(format!("{uri_str}:{name}:{method}")),
            ordinal,
            name: name.to_owned(),
            method,
            resolved_path: ResolvedPath::Resolved(path.to_owned()),
            decorator_path: path.to_owned(),
            chain: vec![],
            handler: Location {
                uri: uri.clone(),
                range: Range {
                    start: Position::new(line, 0),
                    end: Position::new(line, 20),
                },
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
            handler_params_known: true,
        }
    }

    #[test]
    fn display_path_resolved() {
        let r = make_record(0, Method::Get, "/items", "list_items", "/app/main.py", 10);
        assert_eq!(display_path(&r), "/items");
    }

    #[test]
    fn display_path_unresolved_falls_back_to_decorator() {
        let mut r = make_record(0, Method::Get, "/items", "list_items", "/app/main.py", 10);
        r.resolved_path = ResolvedPath::Unresolved;
        r.decorator_path = "/fallback".to_owned();
        assert_eq!(display_path(&r), "/fallback");
    }

    #[test]
    fn handler_location_strips_file_prefix() {
        let r = make_record(0, Method::Get, "/items", "list_items", "/app/main.py", 9);
        assert_eq!(handler_location(&r), "/app/main.py:10");
    }

    #[test]
    fn handler_location_line_is_one_based() {
        let r = make_record(0, Method::Post, "/users", "create_user", "/app/users.py", 0);
        assert_eq!(handler_location(&r), "/app/users.py:1");
    }

    #[test]
    fn json_output_has_required_fields() {
        let r = make_record(0, Method::Delete, "/items/{id}", "delete_item", "/app/main.py", 5);
        let obj = serde_json::json!({
            "method": format!("{}", r.method),
            "path": display_path(&r),
            "name": r.name,
            "handler": r.handler.uri.as_str(),
            "line": r.handler.range.start.line + 1,
        });
        assert_eq!(obj["method"], "DELETE");
        assert_eq!(obj["path"], "/items/{id}");
        assert_eq!(obj["name"], "delete_item");
        assert_eq!(obj["line"], 6);
        assert!(obj["handler"].as_str().unwrap().contains("main.py"));
    }
}
