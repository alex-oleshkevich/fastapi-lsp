use tree_sitter::{Node, Tree};

use crate::state::{FileFacts, ModelFact, range_from_node};

const PYDANTIC_BASE_NAMES: &[&str] = &[
    "BaseModel",
    "BaseSettings",
    "SQLModel",
    "RootModel",
    "TypedDict",
];

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let root = tree.root_node();
    extract_imports(src, root, facts);
    extract_module_level_names(src, root, facts);
    extract_models(src, root, facts, enc);
}

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    std::str::from_utf8(&src[node.byte_range()]).unwrap_or("")
}

fn first_identifier_in_dotted<'a>(src: &'a [u8], node: Node<'_>) -> Option<&'a str> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let t = std::str::from_utf8(&src[child.byte_range()]).unwrap_or("");
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

fn last_identifier_in_dotted<'a>(src: &'a [u8], node: Node<'_>) -> Option<&'a str> {
    let mut last = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" {
            let t = std::str::from_utf8(&src[child.byte_range()]).unwrap_or("");
            if !t.is_empty() {
                last = Some(t);
            }
        }
    }
    last
}

/// Collect simple identifier names bound at module level via assignment or `def`/`class`.
/// These are valid response_model candidates even if they're not pydantic classes.
/// Example: `ProjectSerializer = schemas.A | schemas.B` → adds "ProjectSerializer".
fn extract_module_level_names(src: &[u8], root: Node<'_>, facts: &mut FileFacts) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "expression_statement" => {
                if let Some(inner) = child.child(0)
                    && inner.kind() == "assignment"
                    && let Some(left) = inner.child_by_field_name("left")
                    && left.kind() == "identifier"
                {
                    let name = node_text(src, left);
                    if !name.is_empty() && !name.starts_with('_') {
                        facts.imported_names.push(name.to_owned());
                    }
                }
            }
            "assignment" => {
                if let Some(left) = child.child_by_field_name("left")
                    && left.kind() == "identifier"
                {
                    let name = node_text(src, left);
                    if !name.is_empty() && !name.starts_with('_') {
                        facts.imported_names.push(name.to_owned());
                    }
                }
            }
            "function_definition" | "decorated_definition" => {
                // `def foo():` or `@decorator\ndef foo():`
                let def_node = if child.kind() == "decorated_definition" {
                    child.child_by_field_name("definition").unwrap_or(child)
                } else {
                    child
                };
                if let Some(name_node) = def_node.child_by_field_name("name") {
                    let name = node_text(src, name_node);
                    if !name.is_empty() {
                        facts.imported_names.push(name.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
}

/// Collect names directly brought into scope by `import` and `from ... import` statements.
fn extract_imports(src: &[u8], root: Node<'_>, facts: &mut FileFacts) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        match child.kind() {
            "import_statement" => {
                // `import foo, bar` or `import foo.bar`
                let mut c = child.walk();
                for item in child.children(&mut c) {
                    match item.kind() {
                        "dotted_name" => {
                            // `import foo.bar` → only "foo" is bound (top-level module)
                            if let Some(first) = first_identifier_in_dotted(src, item) {
                                facts.imported_names.push(first.to_owned());
                            }
                        }
                        "identifier" => {
                            let text = node_text(src, item);
                            if !text.is_empty() {
                                facts.imported_names.push(text.to_owned());
                            }
                        }
                        "aliased_import" => {
                            // `import foo as f` → "f" in scope
                            if let Some(alias) = item.child_by_field_name("alias") {
                                let text = node_text(src, alias);
                                if !text.is_empty() {
                                    facts.imported_names.push(text.to_owned());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            "import_from_statement" => {
                // `from foo import bar, baz as b` or `from foo import *`
                // Tree-sitter structure: "from" <dotted_name/relative_import> "import" <names…>
                let mut c = child.walk();
                let mut module_path: Option<String> = None;
                let mut past_import_kw = false;
                for item in child.children(&mut c) {
                    if item.kind() == "import" {
                        past_import_kw = true;
                        continue;
                    }
                    if !past_import_kw {
                        // Before the "import" keyword: module path or relative import
                        if item.kind() == "dotted_name" || item.kind() == "relative_import" {
                            module_path = Some(node_text(src, item).to_owned());
                        }
                        continue;
                    }
                    match item.kind() {
                        "dotted_name" => {
                            // `from foo import Book` or `from foo import bar.Baz`
                            // Take only the last segment (rightmost identifier).
                            if let Some(last) = last_identifier_in_dotted(src, item) {
                                facts.imported_names.push(last.to_owned());
                                if let Some(ref mp) = module_path {
                                    facts.imported_from.insert(last.to_owned(), mp.clone());
                                }
                            }
                        }
                        "aliased_import" => {
                            // `from foo import Book as B` → alias is "B", original is "Book"
                            if let Some(alias) = item.child_by_field_name("alias") {
                                let alias_text = node_text(src, alias);
                                if !alias_text.is_empty() {
                                    facts.imported_names.push(alias_text.to_owned());
                                    if let Some(ref mp) = module_path {
                                        facts
                                            .imported_from
                                            .insert(alias_text.to_owned(), mp.clone());
                                    }
                                    if let Some(name_node) = item.child_by_field_name("name") {
                                        let original = node_text(src, name_node);
                                        if !original.is_empty() && original != alias_text {
                                            facts
                                                .import_alias_originals
                                                .insert(alias_text.to_owned(), original.to_owned());
                                        }
                                    }
                                }
                            }
                        }
                        "wildcard_import" => {
                            facts.imported_names.push("*".to_owned());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// Extract Pydantic-style model class definitions from the module top level.
fn extract_models(src: &[u8], root: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "class_definition"
            && let Some(fact) = try_extract_model(src, child, enc)
        {
            facts.models.push(fact);
        }
    }
}

fn try_extract_model(
    src: &[u8],
    node: Node<'_>,
    enc: crate::offset::Encoding,
) -> Option<ModelFact> {
    let name_node = node.child_by_field_name("name")?;
    let class_name = node_text(src, name_node);
    if class_name.is_empty() {
        return None;
    }

    let bases = node.child_by_field_name("superclasses")?;
    let is_model = inherits_from_known_base(src, bases);
    if !is_model {
        return None;
    }

    let is_settings = inherits_from_settings_base(src, bases);

    Some(ModelFact {
        name: class_name.to_owned(),
        range: range_from_node(node, src, enc),
        is_settings,
    })
}

fn inherits_from_known_base(src: &[u8], bases: Node<'_>) -> bool {
    let mut cursor = bases.walk();
    for base in bases.children(&mut cursor) {
        if base_name_matches(src, base, PYDANTIC_BASE_NAMES) {
            return true;
        }
    }
    false
}

fn inherits_from_settings_base(src: &[u8], bases: Node<'_>) -> bool {
    const SETTINGS_BASES: &[&str] = &["BaseSettings"];
    let mut cursor = bases.walk();
    for base in bases.children(&mut cursor) {
        if base_name_matches(src, base, SETTINGS_BASES) {
            return true;
        }
    }
    false
}

/// Check if a base class node (identifier or attribute) matches any of the known names.
fn base_name_matches(src: &[u8], base: Node<'_>, names: &[&str]) -> bool {
    match base.kind() {
        "identifier" => {
            let text = node_text(src, base);
            names.contains(&text)
        }
        "attribute" => {
            // `pydantic.BaseModel` — check the attribute name (rightmost part)
            if let Some(attr) = base.child_by_field_name("attribute") {
                let text = node_text(src, attr);
                names.contains(&text)
            } else {
                false
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_file;

    fn extract_from(src: &str) -> FileFacts {
        use tower_lsp_server::ls_types::Uri;
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
    fn detects_basemodel_subclass() {
        let facts = extract_from("class Book(BaseModel): pass\n");
        assert_eq!(facts.models.len(), 1);
        assert_eq!(facts.models[0].name, "Book");
        assert!(!facts.models[0].is_settings);
    }

    #[test]
    fn detects_qualified_base() {
        let facts = extract_from("class Book(pydantic.BaseModel): pass\n");
        assert_eq!(facts.models.len(), 1);
        assert_eq!(facts.models[0].name, "Book");
    }

    #[test]
    fn detects_settings_subclass() {
        let facts = extract_from("class AppSettings(BaseSettings): pass\n");
        assert_eq!(facts.models.len(), 1);
        assert!(facts.models[0].is_settings);
    }

    #[test]
    fn ignores_plain_class() {
        let facts = extract_from("class MyClass: pass\n");
        assert!(facts.models.is_empty());
    }

    #[test]
    fn ignores_non_pydantic_base() {
        let facts = extract_from("class MyView(View): pass\n");
        assert!(facts.models.is_empty());
    }

    #[test]
    fn collects_from_imports() {
        let facts =
            extract_from("from app.models import Book, Author\nfrom typing import Optional\n");
        assert!(facts.imported_names.contains(&"Book".to_owned()));
        assert!(facts.imported_names.contains(&"Author".to_owned()));
        assert!(facts.imported_names.contains(&"Optional".to_owned()));
        // Module path segments must NOT appear as imported names
        assert!(!facts.imported_names.contains(&"app".to_owned()));
        assert!(!facts.imported_names.contains(&"models".to_owned()));
        assert!(!facts.imported_names.contains(&"typing".to_owned()));
    }

    #[test]
    fn collects_aliased_imports() {
        let facts = extract_from("from app.models import Book as BookModel\n");
        assert!(facts.imported_names.contains(&"BookModel".to_owned()));
        assert!(!facts.imported_names.contains(&"Book".to_owned()));
    }

    #[test]
    fn aliased_import_tracks_original_name() {
        // `from X import router as projects_router` → alias_originals["projects_router"] = "router"
        let facts =
            extract_from("from app.features.projects.router import router as projects_router\n");
        assert_eq!(
            facts
                .import_alias_originals
                .get("projects_router")
                .map(|s| s.as_str()),
            Some("router"),
            "alias original must be tracked; got {:?}",
            facts.import_alias_originals,
        );
    }

    #[test]
    fn non_aliased_import_not_in_alias_originals() {
        let facts = extract_from("from app.models import Book\n");
        assert!(
            facts.import_alias_originals.is_empty(),
            "non-aliased imports should not appear in alias_originals",
        );
    }

    #[test]
    fn wildcard_import_leaves_sentinel() {
        let facts = extract_from("from app.models import *\n");
        assert!(facts.imported_names.contains(&"*".to_owned()));
    }

    #[test]
    fn multiple_models_extracted() {
        let src = "class Book(BaseModel): pass\nclass Author(BaseModel): pass\n";
        let facts = extract_from(src);
        let names: Vec<_> = facts.models.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"Book"));
        assert!(names.contains(&"Author"));
    }

    #[test]
    fn module_level_type_alias_suppresses_response_model_diag() {
        // `ProjectSerializer = A | B` at module level: the name must appear
        // in imported_names so model/unknown-response-model is silenced.
        let facts = extract_from("from schemas import A, B\nProjectSerializer = A | B\n");
        assert!(
            facts
                .imported_names
                .contains(&"ProjectSerializer".to_owned()),
            "module-level assignment target must be in imported_names; got {:?}",
            facts.imported_names,
        );
    }

    #[test]
    fn private_module_level_name_not_collected() {
        let facts = extract_from("_internal = 42\n");
        assert!(
            !facts.imported_names.contains(&"_internal".to_owned()),
            "underscore-prefixed names should be excluded",
        );
    }
}
