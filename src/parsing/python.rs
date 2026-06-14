use tree_sitter::{Parser, Tree};

thread_local! {
    static PYTHON_PARSER: std::cell::RefCell<Parser> = std::cell::RefCell::new({
        let mut p = Parser::new();
        p.set_language(&tree_sitter_python::LANGUAGE.into()).expect("python grammar");
        p
    });
}

pub fn parse_file(src: &[u8]) -> Tree {
    PYTHON_PARSER.with(|p| {
        p.borrow_mut().parse(src, None).expect("tree-sitter parse")
    })
}

#[allow(dead_code)]
pub fn parse_incremental(src: &[u8], old: &Tree, edit: &tree_sitter::InputEdit) -> Tree {
    PYTHON_PARSER.with(|p| {
        let mut parser = p.borrow_mut();
        let mut old_clone = old.clone();
        old_clone.edit(edit);
        parser.parse(src, Some(&old_clone)).expect("tree-sitter parse")
    })
}
