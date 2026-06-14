use tower_lsp_server::ls_types::{DocumentLink, Uri};

use crate::state::WorkspaceState;

/// Emit clickable document links for all recognised template name strings
/// in the given file (REQ-NAV-03).
pub fn document_links(state: &WorkspaceState, uri: &Uri) -> Vec<DocumentLink> {
    let facts = match state.file_facts.get(uri) {
        Some(f) => f,
        None => return vec![],
    };
    let linked = state.linked.load();

    facts
        .templates
        .iter()
        .filter_map(|tpl| {
            let target = linked.template_index.get(&tpl.path)?;
            Some(DocumentLink {
                range: tpl.range,
                target: Some(target.clone()),
                tooltip: Some(tpl.path.clone()),
                data: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tower_lsp_server::ls_types::{Position, Range};

    use crate::config::ResolvedConfig;
    use crate::state::{FileFacts, Linked, TemplateRef};

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
        Range {
            start: Position::new(sl, sc),
            end: Position::new(el, ec),
        }
    }

    #[test]
    fn emits_link_when_template_found_in_index() {
        let uri_py: Uri = "file:///app/main.py".parse().unwrap();
        let uri_tpl: Uri = "file:///app/templates/index.html".parse().unwrap();

        let mut facts = FileFacts::new(uri_py.clone());
        facts.templates.push(TemplateRef {
            path: "index.html".to_owned(),
            range: range(5, 10, 5, 22),
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_py.clone(), facts);

        let mut linked = Linked::default();
        linked.template_index.insert("index.html".to_owned(), uri_tpl.clone());
        state.linked.store(Arc::new(linked));

        let links = document_links(&state, &uri_py);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].range, range(5, 10, 5, 22));
        assert_eq!(links[0].target.as_ref().unwrap(), &uri_tpl);
        assert_eq!(links[0].tooltip.as_deref(), Some("index.html"));
    }

    #[test]
    fn no_link_when_template_not_in_index() {
        let uri_py: Uri = "file:///app/main.py".parse().unwrap();

        let mut facts = FileFacts::new(uri_py.clone());
        facts.templates.push(TemplateRef {
            path: "missing.html".to_owned(),
            range: range(3, 0, 3, 14),
        });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_py.clone(), facts);
        state.linked.store(Arc::new(Linked::default()));

        let links = document_links(&state, &uri_py);
        assert!(links.is_empty());
    }

    #[test]
    fn multiple_templates_multiple_links() {
        let uri_py: Uri = "file:///app/views.py".parse().unwrap();
        let uri_a: Uri = "file:///tpl/a.html".parse().unwrap();
        let uri_b: Uri = "file:///tpl/b.html".parse().unwrap();

        let mut facts = FileFacts::new(uri_py.clone());
        facts.templates.push(TemplateRef { path: "a.html".to_owned(), range: range(1, 0, 1, 8) });
        facts.templates.push(TemplateRef { path: "missing.html".to_owned(), range: range(2, 0, 2, 14) });
        facts.templates.push(TemplateRef { path: "b.html".to_owned(), range: range(3, 0, 3, 8) });

        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.file_facts.insert(uri_py.clone(), facts);

        let mut linked = Linked::default();
        linked.template_index.insert("a.html".to_owned(), uri_a.clone());
        linked.template_index.insert("b.html".to_owned(), uri_b.clone());
        state.linked.store(Arc::new(linked));

        let links = document_links(&state, &uri_py);
        assert_eq!(links.len(), 2);
        let targets: Vec<&Uri> = links.iter().map(|l| l.target.as_ref().unwrap()).collect();
        assert!(targets.contains(&&uri_a));
        assert!(targets.contains(&&uri_b));
    }

    #[test]
    fn no_facts_returns_empty() {
        let uri_py: Uri = "file:///app/unknown.py".parse().unwrap();
        let state = crate::state::WorkspaceState::new(
            ResolvedConfig::default_for_root(std::path::PathBuf::from("/tmp")),
        );
        state.linked.store(Arc::new(Linked::default()));

        let links = document_links(&state, &uri_py);
        assert!(links.is_empty());
    }
}
