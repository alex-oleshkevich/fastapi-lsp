/// Extract Depends() call sites, dependency function definitions, and
/// dependency_overrides assignments. Covers REQ-DI-01, REQ-DI-05.
use tree_sitter::{Node, Tree};

use crate::state::{DepDef, DepRef, FileFacts, NodeId, OverrideSite, range_from_node};

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let root = tree.root_node();
    walk(src, root, None, facts, enc);
}

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn walk(
    src: &[u8],
    node: Node<'_>,
    current_func: Option<(String, NodeId)>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let child_func: Option<(String, NodeId)> = match node.kind() {
        // A `decorated_definition` wraps the inner `function_definition`; handle once here.
        "decorated_definition" => {
            extract_func_def(src, node, facts, enc)
        }
        "function_definition" => {
            // Skip when already handled as part of a decorated_definition.
            if node.parent().map(|p| p.kind()) != Some("decorated_definition") {
                extract_func_def(src, node, facts, enc)
            } else {
                current_func.clone()
            }
        }
        "call" => {
            if let Some(mut dep_ref) = extract_depends_call(src, node, enc) {
                dep_ref.containing_func = current_func.as_ref().map(|(name, _)| name.clone());
                dep_ref.caller_node_id = current_func.as_ref().map(|(_, id)| id.clone());
                facts.dep_refs.push(dep_ref);
            }
            facts.override_sites.extend(extract_override_update(src, node, enc));
            current_func.clone()
        }
        "assignment" => {
            if let Some(site) = extract_override_subscript(src, node, enc) {
                facts.override_sites.push(site);
            }
            current_func.clone()
        }
        _ => current_func.clone(),
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(src, child, child_func.clone(), facts, enc);
    }
}

// ── Dependency function definitions ──────────────────────────────────────────

/// Extracts a function definition into facts and returns `Some((name, node_id))` for the walker.
fn extract_func_def(
    src: &[u8],
    node: Node<'_>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) -> Option<(String, NodeId)> {
    let func_node = if node.kind() == "decorated_definition" {
        match node.child_by_field_name("definition") {
            Some(n) if n.kind() == "function_definition" => n,
            _ => return None,
        }
    } else {
        node
    };

    let name_node = func_node.child_by_field_name("name")?;
    let name = node_text(src, name_node).to_owned();
    let range = range_from_node(name_node, src, enc);
    let node_id = NodeId { uri: facts.uri.clone(), range };
    let has_yield = body_has_yield(func_node);
    let (param_names, _) = extract_handler_params(src, func_node);

    facts.dep_defs.push(DepDef { name: name.clone(), node_id: node_id.clone(), has_yield, param_names });
    Some((name, node_id))
}

/// Extract parameter names and splat-arg presence from a function_definition node.
/// Skips `self` and `cls`. Returns (names, has_splat).
fn extract_handler_params(src: &[u8], func_node: Node<'_>) -> (Vec<String>, bool) {
    let params_node = match func_node.child_by_field_name("parameters") {
        Some(p) => p,
        None => return (vec![], false),
    };

    let mut names = vec![];
    let mut has_splat = false;

    let mut cursor = params_node.walk();
    for child in params_node.children(&mut cursor) {
        match child.kind() {
            "identifier" => {
                let name = node_text(src, child);
                if name != "self" && name != "cls" {
                    names.push(name.to_owned());
                }
            }
            "typed_parameter" | "default_parameter" | "typed_default_parameter" => {
                if let Some(first) = child.child(0)
                    && first.kind() == "identifier" {
                        let name = node_text(src, first);
                        if name != "self" && name != "cls" {
                            names.push(name.to_owned());
                        }
                    }
            }
            "list_splat_pattern" | "dictionary_splat_pattern" => {
                has_splat = true;
            }
            _ => {}
        }
    }

    (names, has_splat)
}

fn body_has_yield(func_node: Node<'_>) -> bool {
    match func_node.child_by_field_name("body") {
        Some(body) => node_contains_yield(body),
        None => false,
    }
}

fn node_contains_yield(node: Node<'_>) -> bool {
    match node.kind() {
        "yield" | "yield_statement" => return true,
        // Stop at nested function scopes — a yield there doesn't make the
        // outer function a generator.
        "function_definition" | "decorated_definition" => return false,
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if node_contains_yield(child) {
            return true;
        }
    }
    false
}

// ── Depends() call sites ──────────────────────────────────────────────────────

fn extract_depends_call(src: &[u8], node: Node<'_>, enc: crate::offset::Encoding) -> Option<DepRef> {
    let func = node.child_by_field_name("function")?;

    let func_name = match func.kind() {
        "identifier" => node_text(src, func),
        "attribute" => func
            .child_by_field_name("attribute")
            .map(|a| node_text(src, a))
            .unwrap_or(""),
        _ => return None,
    };
    if func_name != "Depends" {
        return None;
    }

    let args = node.child_by_field_name("arguments")?;
    let range = range_from_node(node, src, enc);

    let first_arg = first_positional_arg(args);
    let (name, is_called, callee_range) = match first_arg {
        None => (String::new(), false, None),
        Some(arg) => match arg.kind() {
            "identifier" => (node_text(src, arg).to_owned(), false, None),
            "attribute" => (dotted_name(src, arg), false, None),
            "call" => {
                let callable = arg
                    .child_by_field_name("function")
                    .map(|f| match f.kind() {
                        "identifier" => node_text(src, f).to_owned(),
                        "attribute" => dotted_name(src, f),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                (callable, true, Some(range_from_node(arg, src, enc)))
            }
            _ => (String::new(), false, None),
        },
    };

    Some(DepRef { name, range, is_called, callee_range, containing_func: None, caller_node_id: None })
}

fn first_positional_arg(args: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "identifier" | "attribute" | "call" => return Some(child),
            "keyword_argument" => return None,
            _ => {}
        }
    }
    None
}

fn dotted_name(src: &[u8], node: Node<'_>) -> String {
    let mut parts = Vec::new();
    collect_dotted(src, node, &mut parts);
    parts.join(".")
}

fn collect_dotted<'a>(src: &'a [u8], node: Node<'_>, parts: &mut Vec<&'a str>) {
    match node.kind() {
        "identifier" => parts.push(node_text(src, node)),
        "attribute" => {
            if let Some(obj) = node.child_by_field_name("object") {
                collect_dotted(src, obj, parts);
            }
            if let Some(attr) = node.child_by_field_name("attribute") {
                parts.push(node_text(src, attr));
            }
        }
        _ => {}
    }
}

// ── dependency_overrides ──────────────────────────────────────────────────────

fn extract_override_subscript(src: &[u8], node: Node<'_>, enc: crate::offset::Encoding) -> Option<OverrideSite> {
    // Match: <expr>.dependency_overrides[<key>] = <value>
    let lhs = node.child_by_field_name("left")?;
    if lhs.kind() != "subscript" {
        return None;
    }
    let obj = lhs.child_by_field_name("value")?;
    let attr_name = match obj.kind() {
        "attribute" => obj
            .child_by_field_name("attribute")
            .map(|a| node_text(src, a))
            .unwrap_or(""),
        _ => return None,
    };
    if attr_name != "dependency_overrides" {
        return None;
    }
    let key = lhs.child_by_field_name("subscript")?;
    let name = match key.kind() {
        "identifier" => node_text(src, key).to_owned(),
        "attribute" => dotted_name(src, key),
        _ => return None,
    };
    Some(OverrideSite { name, range: range_from_node(key, src, enc) })
}

fn extract_override_update(src: &[u8], call_node: Node<'_>, enc: crate::offset::Encoding) -> Vec<OverrideSite> {
    // Match: <expr>.dependency_overrides.update({<key>: <val>, ...})
    let func = match call_node.child_by_field_name("function") {
        Some(f) => f,
        None => return vec![],
    };
    if func.kind() != "attribute" {
        return vec![];
    }
    let update_attr = func
        .child_by_field_name("attribute")
        .map(|a| node_text(src, a))
        .unwrap_or("");
    if update_attr != "update" {
        return vec![];
    }
    let obj = match func.child_by_field_name("object") {
        Some(o) => o,
        None => return vec![],
    };
    let obj_attr = match obj.kind() {
        "attribute" => obj
            .child_by_field_name("attribute")
            .map(|a| node_text(src, a))
            .unwrap_or(""),
        _ => return vec![],
    };
    if obj_attr != "dependency_overrides" {
        return vec![];
    }

    let args = match call_node.child_by_field_name("arguments") {
        Some(a) => a,
        None => return vec![],
    };

    let mut sites = vec![];
    let mut cursor = args.walk();
    for arg in args.children(&mut cursor) {
        if arg.kind() == "dictionary" {
            let mut dc = arg.walk();
            for pair in arg.children(&mut dc) {
                if pair.kind() == "pair"
                    && let Some(key) = pair.child_by_field_name("key") {
                        let name = match key.kind() {
                            "identifier" => node_text(src, key).to_owned(),
                            "attribute" => dotted_name(src, key),
                            _ => continue,
                        };
                        sites.push(OverrideSite { name, range: range_from_node(key, src, enc) });
                    }
            }
        }
    }
    sites
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Uri;

    fn parse(src: &[u8]) -> tree_sitter::Tree {
        crate::parsing::parse_file(src)
    }

    fn facts_for(src: &str) -> FileFacts {
        let bytes = src.as_bytes();
        let tree = parse(bytes);
        let uri: Uri = "file:///test.py".parse().unwrap();
        let mut facts = FileFacts::new(uri);
        extract(bytes, &tree, &mut facts, crate::offset::Encoding::Utf8);
        facts
    }

    #[test]
    fn depends_plain_name() {
        let facts = facts_for("def ep(db = Depends(get_db)): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].name, "get_db");
        assert!(!facts.dep_refs[0].is_called);
    }

    #[test]
    fn depends_called_is_flagged() {
        let facts = facts_for("def ep(db = Depends(get_db())): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].name, "get_db");
        assert!(facts.dep_refs[0].is_called);
    }

    #[test]
    fn depends_bare() {
        let facts = facts_for("def ep(db: Session = Depends()): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].name, "");
        assert!(!facts.dep_refs[0].is_called);
    }

    #[test]
    fn depends_in_annotated() {
        let facts = facts_for("def ep(db: Annotated[Session, Depends(get_db)]): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].name, "get_db");
    }

    #[test]
    fn depends_in_dependencies_list() {
        let facts = facts_for("router = APIRouter(dependencies=[Depends(auth)])");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].name, "auth");
    }

    #[test]
    fn depends_in_include_router() {
        let facts = facts_for("app.include_router(router, dependencies=[Depends(auth)])");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].name, "auth");
    }

    #[test]
    fn dep_def_records_functions() {
        let facts = facts_for("def get_db(): yield db\ndef helper(): pass");
        assert_eq!(facts.dep_defs.len(), 2);
        let names: Vec<&str> = facts.dep_defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"get_db"));
        assert!(names.contains(&"helper"));
    }

    #[test]
    fn dep_def_has_yield_detected() {
        let facts = facts_for("def get_db():\n    yield db");
        assert_eq!(facts.dep_defs.len(), 1);
        assert!(facts.dep_defs[0].has_yield);
    }

    #[test]
    fn dep_def_no_yield() {
        let facts = facts_for("def helper():\n    return x");
        assert_eq!(facts.dep_defs.len(), 1);
        assert!(!facts.dep_defs[0].has_yield);
    }

    #[test]
    fn dep_def_nested_yield_does_not_propagate() {
        // yield inside an inner function must not mark the outer as has_yield
        let src = "def outer():\n    def inner():\n        yield 1\n    return inner()";
        let facts = facts_for(src);
        let outer = facts.dep_defs.iter().find(|d| d.name == "outer").unwrap();
        assert!(!outer.has_yield);
    }

    #[test]
    fn dep_def_decorated_not_double_counted() {
        let facts = facts_for("@router.get('/')\ndef ep(): pass");
        assert_eq!(facts.dep_defs.len(), 1);
        assert_eq!(facts.dep_defs[0].name, "ep");
    }

    #[test]
    fn override_site_subscript() {
        let facts = facts_for("app.dependency_overrides[get_db] = fake_db");
        assert_eq!(facts.override_sites.len(), 1);
        assert_eq!(facts.override_sites[0].name, "get_db");
    }

    #[test]
    fn override_site_update() {
        let facts = facts_for("app.dependency_overrides.update({get_db: fake_db})");
        assert_eq!(facts.override_sites.len(), 1);
        assert_eq!(facts.override_sites[0].name, "get_db");
    }

    #[test]
    fn depends_dotted_name() {
        let facts = facts_for("def ep(x = Depends(auth.get_user)): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].name, "auth.get_user");
    }

    #[test]
    fn depends_containing_func_captured() {
        let facts = facts_for("def endpoint(db = Depends(get_db)): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        assert_eq!(facts.dep_refs[0].containing_func.as_deref(), Some("endpoint"));
    }

    #[test]
    fn depends_module_scope_has_no_containing_func() {
        // Depends() at module scope (e.g. in a list) → containing_func is None
        let facts = facts_for("dependencies = [Depends(get_db)]");
        assert_eq!(facts.dep_refs.len(), 1);
        assert!(facts.dep_refs[0].containing_func.is_none());
    }

    #[test]
    fn depends_nested_function_uses_inner_name() {
        let src = "def outer(x = Depends(dep_a)):\n    def inner(y = Depends(dep_b)): pass";
        let facts = facts_for(src);
        let outer = facts.dep_refs.iter().find(|d| d.name == "dep_a").unwrap();
        let inner = facts.dep_refs.iter().find(|d| d.name == "dep_b").unwrap();
        assert_eq!(outer.containing_func.as_deref(), Some("outer"));
        assert_eq!(inner.containing_func.as_deref(), Some("inner"));
    }

    #[test]
    fn depends_called_stores_callee_range() {
        let facts = facts_for("def ep(db = Depends(get_db())): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        let dr = &facts.dep_refs[0];
        assert!(dr.is_called);
        assert_eq!(dr.name, "get_db");
        // callee_range covers get_db() — must be set when is_called
        assert!(dr.callee_range.is_some());
        let cr = dr.callee_range.unwrap();
        // The range start is after "Depends(" — exact col depends on parsing; just verify it's nonzero
        assert!(cr.start.character > 0);
    }

    #[test]
    fn depends_not_called_has_no_callee_range() {
        let facts = facts_for("def ep(db = Depends(get_db)): pass");
        assert_eq!(facts.dep_refs.len(), 1);
        assert!(!facts.dep_refs[0].is_called);
        assert!(facts.dep_refs[0].callee_range.is_none());
    }
}
