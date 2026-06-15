use tree_sitter::{Node, Tree};

use crate::state::{AnnotatedParam, FileFacts, PlainTypedParam, range_from_node};

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("")
}

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    walk(src, tree.root_node(), facts, enc, false);
}

/// `nested`: true when inside a function or class body — module-level alias extraction is skipped.
fn walk(
    src: &[u8],
    node: Node<'_>,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
    nested: bool,
) {
    match node.kind() {
        "function_definition" => {
            let func_name = node
                .child_by_field_name("name")
                .map(|n| node_text(src, n).to_owned())
                .unwrap_or_default();
            if let Some(params) = node.child_by_field_name("parameters") {
                extract_params(src, params, &func_name, facts, enc);
            }
            // Recurse into nested body — functions defined inside functions still have params.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk(src, child, facts, enc, true);
            }
            return;
        }
        "class_definition" => {
            // Class bodies may contain method definitions; recurse but mark as nested
            // so class-level assignments are not captured as module dep-type aliases.
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk(src, child, facts, enc, true);
            }
            return;
        }
        "assignment" if !nested => {
            if let Some((alias, dep_fn)) = try_extract_dep_type_alias(src, node) {
                let range = range_from_node(node, src, enc);
                facts.dep_type_alias_ranges.insert(alias.clone(), range);
                facts.dep_type_aliases.insert(alias, dep_fn);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(src, child, facts, enc, nested);
    }
}

fn extract_params(
    src: &[u8],
    params_node: Node<'_>,
    func_name: &str,
    facts: &mut FileFacts,
    enc: crate::offset::Encoding,
) {
    let mut cursor = params_node.walk();
    for param in params_node.children(&mut cursor) {
        match param.kind() {
            "typed_default_parameter" => {
                let Some(name_node) = param.child_by_field_name("name") else {
                    continue;
                };
                let Some(type_node) = param.child_by_field_name("type") else {
                    continue;
                };
                let Some(value_node) = param.child_by_field_name("value") else {
                    continue;
                };
                if !is_depends_call(src, value_node) {
                    continue;
                }
                facts.annotated_params.push(AnnotatedParam {
                    containing_func: func_name.to_owned(),
                    param_name: node_text(src, name_node).to_owned(),
                    is_annotated: false,
                    annotation_range: range_from_node(type_node, src, enc),
                    default_range: Some(range_from_node(value_node, src, enc)),
                    type_text: node_text(src, type_node).to_owned(),
                    depends_text: node_text(src, value_node).to_owned(),
                    has_extra_args: false,
                });
            }
            "typed_parameter" => {
                let Some(name_node) = first_identifier(param) else {
                    continue;
                };
                // child_by_field_name("type") returns a "type" wrapper node; the actual
                // generic_type is one level inside it.
                let Some(type_wrapper) = param.child_by_field_name("type") else {
                    continue;
                };
                let mut c = type_wrapper.walk();
                let type_children: Vec<Node> = type_wrapper.children(&mut c).collect();
                if let Some(generic_node) =
                    type_children.iter().find(|n| n.kind() == "generic_type")
                {
                    let Some((type_text, depends_text, has_extra_args)) =
                        extract_annotated_type(src, *generic_node)
                    else {
                        continue;
                    };
                    facts.annotated_params.push(AnnotatedParam {
                        containing_func: func_name.to_owned(),
                        param_name: node_text(src, name_node).to_owned(),
                        is_annotated: true,
                        annotation_range: range_from_node(type_wrapper, src, enc),
                        default_range: None,
                        type_text,
                        depends_text,
                        has_extra_args,
                    });
                } else if let Some(ident_node) =
                    type_children.iter().find(|n| n.kind() == "identifier")
                {
                    let type_name = node_text(src, *ident_node);
                    // Skip Python built-in types — they will never appear in dep_type_aliases.
                    if !matches!(
                        type_name,
                        "int"
                            | "str"
                            | "bool"
                            | "float"
                            | "bytes"
                            | "list"
                            | "dict"
                            | "set"
                            | "tuple"
                            | "None"
                            | "type"
                    ) {
                        facts.plain_typed_params.push(PlainTypedParam {
                            containing_func: func_name.to_owned(),
                            param_name: node_text(src, name_node).to_owned(),
                            type_name: type_name.to_owned(),
                            annotation_range: range_from_node(*ident_node, src, enc),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn first_identifier(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .find(|c| c.kind() == "identifier")
}

fn is_depends_call(src: &[u8], node: Node<'_>) -> bool {
    if node.kind() != "call" {
        return false;
    }
    let Some(func) = node.child_by_field_name("function") else {
        return false;
    };
    match func.kind() {
        "identifier" => node_text(src, func) == "Depends",
        "attribute" => func
            .child_by_field_name("attribute")
            .map(|a| node_text(src, a) == "Depends")
            .unwrap_or(false),
        _ => false,
    }
}

/// For a `generic_type` node that is `Annotated[T, Depends(fn), ...]`, extract
/// `(type_text, depends_text, has_extra_args)`.
/// In Python type-annotation position, `X[T, ...]` is a `generic_type` node, not `subscript`.
fn extract_annotated_type(src: &[u8], node: Node<'_>) -> Option<(String, String, bool)> {
    if node.kind() != "generic_type" {
        return None;
    }
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();

    // First child: identifier "Annotated"
    let name = children.first()?;
    if name.kind() != "identifier" || node_text(src, *name) != "Annotated" {
        return None;
    }

    // Second child: type_parameter containing the bracketed arguments
    let type_params = children.get(1)?;
    if type_params.kind() != "type_parameter" {
        return None;
    }

    // Collect the "type" children within type_parameter
    let mut c = type_params.walk();
    let type_args: Vec<Node> = type_params
        .children(&mut c)
        .filter(|n| n.kind() == "type")
        .collect();
    if type_args.len() < 2 {
        return None;
    }

    // Second type arg must wrap a Depends(...) call
    // (In annotation position each arg is wrapped in a "type" node)
    let depends_arg = type_args[1];
    let has_depends = {
        let mut c = depends_arg.walk();
        depends_arg
            .children(&mut c)
            .any(|n| is_depends_call(src, n))
    };
    if !has_depends {
        return None;
    }

    let has_extra_args = type_args.len() > 2;
    Some((
        node_text(src, type_args[0]).to_owned(),
        node_text(src, depends_arg).to_owned(),
        has_extra_args,
    ))
}

/// For a module-level `X = Annotated[T, Depends(fn)]` assignment, extract `("X", "fn")`.
/// RHS must be a `subscript` whose value is `Annotated` (or `X.Annotated`), and the
/// subscript args must contain a `Depends(fn)` call anywhere in the subtree.
fn try_extract_dep_type_alias(src: &[u8], node: Node<'_>) -> Option<(String, String)> {
    if node.kind() != "assignment" {
        return None;
    }
    let lhs = node.child_by_field_name("left")?;
    if lhs.kind() != "identifier" {
        return None;
    }
    let alias_name = node_text(src, lhs).to_owned();

    let rhs = node.child_by_field_name("right")?;
    if rhs.kind() != "subscript" {
        return None;
    }
    let value = rhs.child_by_field_name("value")?;
    let is_annotated = match value.kind() {
        "identifier" => node_text(src, value) == "Annotated",
        "attribute" => value
            .child_by_field_name("attribute")
            .map(|a| node_text(src, a) == "Annotated")
            .unwrap_or(false),
        _ => false,
    };
    if !is_annotated {
        return None;
    }

    let dep_fn = find_depends_arg_in_subtree(src, rhs)?;
    Some((alias_name, dep_fn))
}

/// Walk all descendants of `node` looking for a `Depends(fn)` call.
/// Returns the first positional argument text when found.
fn find_depends_arg_in_subtree(src: &[u8], node: Node<'_>) -> Option<String> {
    if is_depends_call(src, node) {
        return extract_depends_first_arg(src, node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_depends_arg_in_subtree(src, child) {
            return Some(found);
        }
    }
    None
}

/// Extract the first positional argument from a `Depends(fn)` call node.
fn extract_depends_first_arg(src: &[u8], depends_call: Node<'_>) -> Option<String> {
    let args = depends_call.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "identifier" | "attribute" => return Some(node_text(src, child).to_owned()),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_file;
    use tower_lsp_server::ls_types::Uri;

    fn extract_from(src: &str) -> FileFacts {
        let uri: Uri = "file:///a.py".parse().unwrap();
        let mut facts = FileFacts::new(uri);
        let tree = parse_file(src.as_bytes());
        extract(
            src.as_bytes(),
            &tree,
            &mut facts,
            crate::offset::Encoding::Utf8,
        );
        facts
    }

    #[test]
    fn detects_inline_depends_param() {
        let facts = extract_from("def get_book(db: Session = Depends(get_db)): pass\n");
        assert_eq!(facts.annotated_params.len(), 1);
        let p = &facts.annotated_params[0];
        assert_eq!(p.param_name, "db");
        assert_eq!(p.containing_func, "get_book");
        assert!(!p.is_annotated);
        assert_eq!(p.type_text, "Session");
        assert_eq!(p.depends_text, "Depends(get_db)");
        assert!(p.default_range.is_some());
    }

    #[test]
    fn detects_annotated_depends_param() {
        let facts = extract_from("def get_book(db: Annotated[Session, Depends(get_db)]): pass\n");
        assert_eq!(facts.annotated_params.len(), 1);
        let p = &facts.annotated_params[0];
        assert_eq!(p.param_name, "db");
        assert_eq!(p.containing_func, "get_book");
        assert!(p.is_annotated);
        assert_eq!(p.type_text, "Session");
        assert_eq!(p.depends_text, "Depends(get_db)");
        assert!(p.default_range.is_none());
    }

    #[test]
    fn ignores_non_depends_default() {
        let facts = extract_from("def get_book(db: Session = None): pass\n");
        assert!(facts.annotated_params.is_empty());
    }

    #[test]
    fn ignores_plain_param() {
        let facts = extract_from("def get_book(book_id: int): pass\n");
        assert!(facts.annotated_params.is_empty());
    }

    #[test]
    fn collects_multiple_params() {
        let facts = extract_from(
            "def handler(db: Session = Depends(get_db), auth: Auth = Depends(get_auth)): pass\n",
        );
        assert_eq!(facts.annotated_params.len(), 2);
    }

    #[test]
    fn handles_annotated_with_extra_args() {
        // Annotated with more than 2 args — only first two matter, second must be Depends
        let facts = extract_from("def f(x: Annotated[int, Depends(fn), Field()]): pass\n");
        // Second subscript arg is Depends(fn) — should still be detected
        assert_eq!(facts.annotated_params.len(), 1);
    }

    #[test]
    fn ignores_annotated_without_depends_second_arg() {
        let facts = extract_from("def f(x: Annotated[int, Query()]): pass\n");
        assert!(facts.annotated_params.is_empty());
    }

    #[test]
    fn module_level_annotated_alias_captured_in_dep_type_aliases() {
        let facts = extract_from("CurrentProject = Annotated[Project, Depends(fetch_project)]\n");
        assert_eq!(
            facts
                .dep_type_aliases
                .get("CurrentProject")
                .map(|s| s.as_str()),
            Some("fetch_project"),
            "module-level Annotated alias must be captured in dep_type_aliases"
        );
    }

    #[test]
    fn typing_qualified_annotated_alias_captured() {
        let facts = extract_from(
            "CurrentProject = typing.Annotated[Project, fastapi.Depends(fetch_project)]\n",
        );
        assert_eq!(
            facts
                .dep_type_aliases
                .get("CurrentProject")
                .map(|s| s.as_str()),
            Some("fetch_project"),
        );
    }

    #[test]
    fn local_assignment_inside_function_not_captured_as_alias() {
        let facts = extract_from("def handler():\n    X = Annotated[T, Depends(fn)]\n    pass\n");
        assert!(
            facts.dep_type_aliases.is_empty(),
            "assignments inside function bodies must not populate dep_type_aliases"
        );
    }

    #[test]
    fn class_level_assignment_not_captured_as_alias() {
        let facts = extract_from("class Deps:\n    X = Annotated[T, Depends(fn)]\n");
        assert!(
            facts.dep_type_aliases.is_empty(),
            "class-level assignments must not populate dep_type_aliases"
        );
    }

    #[test]
    fn builtin_typed_param_not_captured_as_plain_typed() {
        let facts = extract_from("def f(book_id: int, name: str): pass\n");
        assert!(
            facts.plain_typed_params.is_empty(),
            "built-in typed params must not be captured in plain_typed_params"
        );
    }

    #[test]
    fn plain_typed_param_captured_when_type_is_identifier() {
        // `project: CurrentProject` — plain identifier type, not Annotated[...]
        let facts = extract_from("def view(project: CurrentProject, guard: Guard): pass\n");
        assert!(
            facts.annotated_params.is_empty(),
            "plain identifier type must not produce annotated_param"
        );
        assert!(
            facts
                .plain_typed_params
                .iter()
                .any(|p| p.param_name == "project" && p.type_name == "CurrentProject"),
            "plain_typed_params must include project:CurrentProject"
        );
    }
}
