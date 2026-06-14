/// Extract env lookup sites, env file declarations, and BaseSettings class bindings.
/// Covers REQ-ENV-02, REQ-ENV-03, REQ-ENV-08.
use tree_sitter::{Node, Tree};

use tower_lsp_server::ls_types::{Position, Range};

use crate::state::{
    EnvFileDecl, EnvLoader, EnvLookupSite, FileFacts, LoaderKind, SettingsClassDecl,
    SettingsField, range_from_node,
};
use super::unquote;

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let root = tree.root_node();

    // First pass: collect binding names (config = Config(...), env = Env(), etc.)
    let bindings = collect_bindings(src, root);

    // Second pass: extract sites
    walk(src, root, facts, &bindings, enc);
}

// ── Binding discovery ─────────────────────────────────────────────────────────

#[derive(Default)]
struct Bindings {
    starlette_configs: Vec<String>,   // names bound to starlette.config.Config(...)
    environs_envs: Vec<String>,       // names bound to environs.Env(...)
    dotenv_dicts: Vec<String>,        // names bound to dotenv_values(...)
}

fn collect_bindings(src: &[u8], root: Node<'_>) -> Bindings {
    let mut b = Bindings::default();
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        collect_binding(src, child, &mut b);
    }
    b
}

fn collect_binding(src: &[u8], node: Node<'_>, b: &mut Bindings) {
    if node.kind() != "assignment" && node.kind() != "expression_statement" {
        let mut c = node.walk();
        for child in node.children(&mut c) {
            collect_binding(src, child, b);
        }
        return;
    }
    let node = if node.kind() == "expression_statement" {
        match node.child(0) {
            Some(n) if n.kind() == "assignment" => n,
            _ => return,
        }
    } else {
        node
    };

    let lhs = match node.child_by_field_name("left") {
        Some(n) if n.kind() == "identifier" => node_text(src, n).to_owned(),
        _ => return,
    };
    let rhs = match node.child_by_field_name("right") {
        Some(n) if n.kind() == "call" => n,
        _ => return,
    };

    let callee_name = callee_short_name(src, rhs);
    match callee_name {
        "Config" => b.starlette_configs.push(lhs),
        "Env" => b.environs_envs.push(lhs),
        _ if callee_name == "dotenv_values" => b.dotenv_dicts.push(lhs),
        _ => {}
    }
}

fn callee_short_name<'a>(src: &'a [u8], call: Node<'_>) -> &'a str {
    let func = match call.child_by_field_name("function") {
        Some(n) => n,
        None => return "",
    };
    match func.kind() {
        "identifier" => node_text(src, func),
        "attribute" => func.child_by_field_name("attribute").map(|n| node_text(src, n)).unwrap_or(""),
        _ => "",
    }
}

// ── Site extraction ───────────────────────────────────────────────────────────

fn walk(src: &[u8], node: Node<'_>, facts: &mut FileFacts, bindings: &Bindings, enc: crate::offset::Encoding) {
    match node.kind() {
        "class_definition" => {
            extract_settings_class(src, node, facts, enc);
            // Recurse for nested classes
            recurse(src, node, facts, bindings, enc);
        }
        "call" => {
            extract_call_site(src, node, facts, bindings, enc);
            recurse(src, node, facts, bindings, enc);
        }
        "subscript" => {
            extract_subscript_site(src, node, facts, bindings, enc);
            recurse(src, node, facts, bindings, enc);
        }
        _ => recurse(src, node, facts, bindings, enc),
    }
}

fn recurse(src: &[u8], node: Node<'_>, facts: &mut FileFacts, bindings: &Bindings, enc: crate::offset::Encoding) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(src, child, facts, bindings, enc);
    }
}

// ── os.environ["KEY"] / os.environ.get("KEY") ────────────────────────────────

fn extract_subscript_site(src: &[u8], node: Node<'_>, facts: &mut FileFacts, bindings: &Bindings, enc: crate::offset::Encoding) {
    let value = match node.child_by_field_name("value") {
        Some(n) => n,
        None => return,
    };
    if !is_os_environ(src, value) && !is_dotenv_dict(src, value, bindings) {
        return;
    }

    let key_node = match node.child_by_field_name("subscript") {
        Some(n) => n,
        None => return,
    };
    let key = match string_value(src, key_node) {
        Some(k) => k,
        None => return,
    };

    let loader = if is_os_environ(src, value) { EnvLoader::OsEnviron } else { EnvLoader::DotenvValues };
    facts.env_lookups.push(EnvLookupSite {
        key,
        has_default: false,
        loader,
        range: range_from_node(node, src, enc),
        key_range: range_from_node(key_node, src, enc),
        replace_range: key_replace_range(src, key_node),
    });
}

fn is_os_environ(src: &[u8], node: Node<'_>) -> bool {
    // Matches `os.environ` or bare `environ`
    match node.kind() {
        "attribute" => {
            let obj = node.child_by_field_name("object").map(|n| node_text(src, n)).unwrap_or("");
            let attr = node.child_by_field_name("attribute").map(|n| node_text(src, n)).unwrap_or("");
            obj == "os" && attr == "environ"
        }
        "identifier" => node_text(src, node) == "environ",
        _ => false,
    }
}

fn is_dotenv_dict(src: &[u8], node: Node<'_>, bindings: &Bindings) -> bool {
    node.kind() == "identifier" && bindings.dotenv_dicts.iter().any(|n| n == node_text(src, node))
}

// ── os.environ.get("KEY") / os.getenv("KEY") ─────────────────────────────────

fn extract_call_site(src: &[u8], node: Node<'_>, facts: &mut FileFacts, bindings: &Bindings, enc: crate::offset::Encoding) {
    let func = match node.child_by_field_name("function") {
        Some(n) => n,
        None => return,
    };
    let args = match node.child_by_field_name("arguments") {
        Some(a) => a,
        None => return,
    };

    // os.environ.get("KEY") or os.getenv("KEY")
    if is_os_get(src, func) {
        if let Some(key_node) = first_string_arg_node(args) {
            let has_default = has_second_positional(src, args);
            facts.env_lookups.push(EnvLookupSite {
                key: unquote(node_text(src, key_node)).to_owned(),
                has_default,
                loader: EnvLoader::OsEnviron,
                range: range_from_node(node, src, enc),
                key_range: range_from_node(key_node, src, enc),
                replace_range: key_replace_range(src, key_node),
            });
        }
        return;
    }

    // starlette Config instance call: config("KEY") / config("KEY", cast=..., default=...)
    // Signature: config(key, cast=str, default=<missing>)
    // The 2nd positional is `cast` (a type), not a default — only `default=` kwarg or
    // the 3rd positional counts as a default value.
    if let Some(cfg_name) = as_callable_name(src, func) {
        if bindings.starlette_configs.iter().any(|n| n == cfg_name) {
            if let Some(key_node) = first_string_arg_node(args) {
                let has_default = has_kwarg(src, args, "default") || has_third_positional(src, args);
                facts.env_lookups.push(EnvLookupSite {
                    key: unquote(node_text(src, key_node)).to_owned(),
                    has_default,
                    loader: EnvLoader::StarletteConfig,
                    range: range_from_node(node, src, enc),
                    key_range: range_from_node(key_node, src, enc),
                    replace_range: key_replace_range(src, key_node),
                });
            }
            return;
        }

        // starlette Config("file") constructor → env file decl
        if cfg_name == "Config"
            && let Some(path) = first_string_arg(src, args) {
                facts.env_file_decls.push(EnvFileDecl {
                    path,
                    loader: LoaderKind::StarletteConfig,
                    range: range_from_node(node, src, enc),
                });
            }

        // environs Env instance method: env.str("KEY"), env.int("KEY"), env("KEY")
        if let Some(method) = as_method_name(src, func) {
            let obj = func.child_by_field_name("object").map(|n| node_text(src, n)).unwrap_or("");
            if bindings.environs_envs.iter().any(|n| n == obj) {
                let is_lookup = matches!(
                    method,
                    "str" | "int" | "float" | "bool" | "list" | "dict" | "url" | "path"
                ) || method == obj; // direct call
                if is_lookup {
                    if let Some(key_node) = first_string_arg_node(args) {
                        let has_default = has_kwarg(src, args, "default");
                        facts.env_lookups.push(EnvLookupSite {
                            key: unquote(node_text(src, key_node)).to_owned(),
                            has_default,
                            loader: EnvLoader::Environs,
                            range: range_from_node(node, src, enc),
                            key_range: range_from_node(key_node, src, enc),
                            replace_range: key_replace_range(src, key_node),
                        });
                    }
                    return;
                }
                // env.read_env("file") → file decl
                if method == "read_env" {
                    if let Some(path) = first_string_arg(src, args) {
                        facts.env_file_decls.push(EnvFileDecl {
                            path,
                            loader: LoaderKind::Environs,
                            range: range_from_node(node, src, enc),
                        });
                    }
                    return;
                }
            }
        }

        // load_dotenv("file") or dotenv_values("file") → file decl
        if matches!(cfg_name, "load_dotenv" | "dotenv_values") {
            let path = first_string_arg(src, args).unwrap_or_else(|| ".env".to_owned());
            facts.env_file_decls.push(EnvFileDecl {
                path,
                loader: LoaderKind::Dotenv,
                range: range_from_node(node, src, enc),
            });
        }
    }
}

fn is_os_get(src: &[u8], func: Node<'_>) -> bool {
    match func.kind() {
        "attribute" => {
            let attr = func.child_by_field_name("attribute").map(|n| node_text(src, n)).unwrap_or("");
            let obj = func.child_by_field_name("object");
            if attr == "getenv" {
                // os.getenv
                return obj.map(|n| node_text(src, n) == "os").unwrap_or(false);
            }
            if attr == "get" {
                // os.environ.get
                return obj.map(|n| is_os_environ(src, n)).unwrap_or(false);
            }
            false
        }
        _ => false,
    }
}

fn as_callable_name<'a>(src: &'a [u8], func: Node<'_>) -> Option<&'a str> {
    match func.kind() {
        "identifier" => Some(node_text(src, func)),
        "attribute" => func.child_by_field_name("attribute").map(|n| node_text(src, n)),
        _ => None,
    }
}

fn as_method_name<'a>(src: &'a [u8], func: Node<'_>) -> Option<&'a str> {
    if func.kind() == "attribute" {
        func.child_by_field_name("attribute").map(|n| node_text(src, n))
    } else {
        None
    }
}

fn first_string_arg(src: &[u8], args: Node<'_>) -> Option<String> {
    first_string_arg_node(args).map(|n| unquote(node_text(src, n)).to_owned())
}

/// Compute the replace Range for textEdit: content only, excluding opening/closing quote chars.
/// Handles `"key"`, `'key'`, `"""key"""`, `f"key"`, `r'key'`, etc.
fn key_replace_range(src: &[u8], key_node: Node<'_>) -> Range {
    let text = key_node.utf8_text(src).unwrap_or("");
    // Count leading string-prefix chars (f, r, b, etc.)
    let after_prefix = text.trim_start_matches(['f', 'r', 'b', 'F', 'R', 'B']);
    let prefix_chars = (text.len() - after_prefix.len()) as u32;
    let quote_chars: u32 = if after_prefix.starts_with("\"\"\"") || after_prefix.starts_with("'''") { 3 } else { 1 };
    let open_offset = prefix_chars + quote_chars;

    let start = key_node.start_position();
    let end = key_node.end_position();
    Range {
        start: Position::new(start.row as u32, start.column as u32 + open_offset),
        end: Position::new(end.row as u32, end.column as u32 - quote_chars),
    }
}

fn first_string_arg_node(args: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            "string" => return Some(child),
            _ => break,
        }
    }
    None
}

fn has_second_positional(src: &[u8], args: Node<'_>) -> bool {
    let _ = src;
    count_positionals(args) >= 2
}

fn has_third_positional(src: &[u8], args: Node<'_>) -> bool {
    let _ = src;
    count_positionals(args) >= 3
}

fn count_positionals(args: Node<'_>) -> usize {
    let mut cursor = args.walk();
    let mut count = 0usize;
    for child in args.children(&mut cursor) {
        match child.kind() {
            "(" | ")" | "," => continue,
            "keyword_argument" | "dictionary_splat_argument" | "list_splat_argument" => break,
            _ => count += 1,
        }
    }
    count
}

fn has_kwarg(src: &[u8], args: Node<'_>, name: &str) -> bool {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "keyword_argument" {
            let key = child.child(0).map(|n| node_text(src, n)).unwrap_or("");
            if key == name {
                return true;
            }
        }
    }
    false
}

// ── BaseSettings class extraction ─────────────────────────────────────────────

fn extract_settings_class(src: &[u8], node: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    // Check that the class inherits from BaseSettings (or a name containing "Settings")
    let superclasses = node.child_by_field_name("superclasses");
    let is_settings = superclasses.map(|sc| {
        let text = node_text(src, sc);
        text.contains("BaseSettings") || text.contains("Settings")
    }).unwrap_or(false);

    if !is_settings {
        return;
    }

    let superclass_names = superclasses
        .map(|sc| extract_superclass_names(src, sc))
        .unwrap_or_default();

    let class_name = match node.child_by_field_name("name") {
        Some(n) => node_text(src, n).to_owned(),
        None => return,
    };
    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };

    let mut env_prefix: Option<String> = None;
    let mut env_file: Option<String> = None;
    let mut fields: Vec<SettingsField> = vec![];

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        match child.kind() {
            "expression_statement" => {
                if let Some(inner) = child.child(0)
                    && (inner.kind() == "assignment" || inner.kind() == "annotated_assignment")
                {
                    if let Some((prefix, file)) = try_extract_settings_config_dict(src, inner) {
                        if prefix.is_some() { env_prefix = prefix; }
                        if file.is_some() { env_file = file; }
                    } else if let Some(field) = extract_settings_field(src, inner, enc) {
                        fields.push(field);
                    }
                }
            }
            "assignment" | "annotated_assignment" => {
                if let Some((prefix, file)) = try_extract_settings_config_dict(src, child) {
                    if prefix.is_some() { env_prefix = prefix; }
                    if file.is_some() { env_file = file; }
                } else if let Some(field) = extract_settings_field(src, child, enc) {
                    fields.push(field);
                }
            }
            "class_definition" => {
                // Inner `Config` class (legacy pydantic v1)
                let inner_name = child
                    .child_by_field_name("name")
                    .map(|n| node_text(src, n))
                    .unwrap_or("");
                if inner_name == "Config" {
                    let (prefix, file) = extract_legacy_config(src, child);
                    env_prefix = prefix;
                    env_file = file;
                }
            }
            _ => {}
        }
    }

    // Apply prefix to field env_keys
    let prefixed_fields: Vec<SettingsField> = fields.into_iter().map(|mut f| {
        if let (Some(k), Some(p)) = (&f.env_key, &env_prefix) {
            f.env_key = Some(format!("{}{}", p.to_uppercase(), k));
        }
        f
    }).collect();

    facts.settings_classes.push(SettingsClassDecl {
        class_name,
        superclass_names,
        env_prefix,
        env_file,
        fields: prefixed_fields,
        range: range_from_node(node, src, enc),
    });
}

/// Extract the direct superclass names from the argument list node.
/// `class Foo(BaseSettings, SomeMixin)` → `["BaseSettings", "SomeMixin"]`
fn extract_superclass_names(src: &[u8], superclasses: Node<'_>) -> Vec<String> {
    let mut names = vec![];
    let mut cursor = superclasses.walk();
    for child in superclasses.children(&mut cursor) {
        match child.kind() {
            "identifier" => names.push(node_text(src, child).to_owned()),
            "attribute" => {
                if let Some(attr) = child.child_by_field_name("attribute") {
                    names.push(node_text(src, attr).to_owned());
                }
            }
            _ => {}
        }
    }
    names
}

/// If `node` is `model_config = SettingsConfigDict(env_prefix=..., env_file=...)`,
/// return `(env_prefix, env_file)`. Otherwise return `None`.
fn try_extract_settings_config_dict(src: &[u8], node: Node<'_>) -> Option<(Option<String>, Option<String>)> {
    let left = node.child_by_field_name("left")?;
    if node_text(src, left) != "model_config" {
        return None;
    }
    let right = node.child_by_field_name("right")?;
    if right.kind() != "call" || callee_short_name(src, right) != "SettingsConfigDict" {
        return None;
    }
    let args = right.child_by_field_name("arguments")?;
    Some((
        kwarg_string_value(src, args, "env_prefix"),
        kwarg_string_value(src, args, "env_file"),
    ))
}

fn kwarg_string_value(src: &[u8], args: Node<'_>, name: &str) -> Option<String> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() == "keyword_argument" {
            let key = child.child(0).map(|n| node_text(src, n)).unwrap_or("");
            if key == name {
                if let Some(val) = child.child(2) {
                    return string_value(src, val);
                }
            }
        }
    }
    None
}

fn extract_settings_field(src: &[u8], node: Node<'_>, enc: crate::offset::Encoding) -> Option<SettingsField> {
    // Handle both `annotated_assignment` (field: type = default) and `assignment`
    let left = node.child_by_field_name("left")?;
    if left.kind() != "identifier" {
        return None;
    }
    let field_name = node_text(src, left).to_owned();
    // Skip dunder names and model_config
    if field_name.starts_with("__") || field_name == "model_config" {
        return None;
    }

    let has_default = node.child_by_field_name("right").is_some();

    // Check for alias/validation_alias in Field(...) default
    let env_key = extract_field_alias(src, node)
        .unwrap_or_else(|| field_name.to_uppercase());

    Some(SettingsField {
        field_name,
        env_key: Some(env_key),
        has_default,
        range: range_from_node(node, src, enc),
    })
}

fn extract_field_alias(src: &[u8], node: Node<'_>) -> Option<String> {
    let value = node.child_by_field_name("right")?;
    if value.kind() != "call" {
        return None;
    }
    // Field(validation_alias="KEY") or Field(alias="KEY")
    let args = value.child_by_field_name("arguments")?;
    // validation_alias takes precedence over alias
    for prefer in ["validation_alias", "alias"] {
        let mut cursor = args.walk();
        for child in args.children(&mut cursor) {
            if child.kind() == "keyword_argument" {
                let key = child.child(0).map(|n| node_text(src, n)).unwrap_or("");
                if key == prefer
                    && let Some(val) = child.child(2)
                        && let Some(s) = string_value(src, val) {
                            return Some(s.to_uppercase());
                        }
            }
        }
    }
    None
}

fn extract_legacy_config(src: &[u8], node: Node<'_>) -> (Option<String>, Option<String>) {
    let body = match node.child_by_field_name("body") {
        Some(b) => b,
        None => return (None, None),
    };
    let mut env_prefix = None;
    let mut env_file = None;
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        let stmt = if child.kind() == "expression_statement" {
            match child.child(0) {
                Some(n) if n.kind() == "assignment" => n,
                _ => continue,
            }
        } else if child.kind() == "assignment" {
            child
        } else {
            continue
        };

        let name = stmt.child_by_field_name("left")
            .map(|n| node_text(src, n))
            .unwrap_or("");
        let val = stmt.child_by_field_name("right");

        if name == "env_prefix" {
            env_prefix = val.and_then(|v| string_value(src, v));
        } else if name == "env_file" {
            env_file = val.and_then(|v| string_value(src, v));
        }
    }
    (env_prefix, env_file)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn string_value(src: &[u8], node: Node<'_>) -> Option<String> {
    if node.kind() == "string" {
        Some(unquote(node_text(src, node)))
    } else {
        None
    }
}

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_file;
    use tower_lsp_server::ls_types::Uri;

    fn uri() -> Uri {
        "file:///app/main.py".parse().unwrap()
    }

    fn run(src: &str) -> FileFacts {
        let bytes = src.as_bytes();
        let tree = parse_file(bytes);
        let mut facts = FileFacts::new(uri());
        extract(bytes, &tree, &mut facts, crate::offset::Encoding::Utf8);
        facts
    }

    #[test]
    fn os_environ_subscript() {
        let facts = run(r#"import os; key = os.environ["DATABASE_URL"]"#);
        assert_eq!(facts.env_lookups.len(), 1);
        assert_eq!(facts.env_lookups[0].key, "DATABASE_URL");
        assert_eq!(facts.env_lookups[0].loader, EnvLoader::OsEnviron);
    }

    #[test]
    fn os_getenv_with_default() {
        let facts = run(r#"import os; v = os.getenv("PORT", "8000")"#);
        assert_eq!(facts.env_lookups.len(), 1);
        assert_eq!(facts.env_lookups[0].key, "PORT");
        assert!(facts.env_lookups[0].has_default);
    }

    #[test]
    fn os_environ_get_no_default() {
        let facts = run(r#"import os; v = os.environ.get("SECRET_KEY")"#);
        assert_eq!(facts.env_lookups.len(), 1);
        assert!(!facts.env_lookups[0].has_default);
    }

    #[test]
    fn starlette_config_lookup() {
        let facts = run(r#"
from starlette.config import Config
config = Config()
DB = config("DATABASE_URL", cast=str)
"#);
        assert_eq!(facts.env_lookups.len(), 1);
        assert_eq!(facts.env_lookups[0].key, "DATABASE_URL");
        assert_eq!(facts.env_lookups[0].loader, EnvLoader::StarletteConfig);
    }

    #[test]
    fn starlette_config_file_decl() {
        let facts = run(r#"
from starlette.config import Config
config = Config(".env.prod")
"#);
        assert_eq!(facts.env_file_decls.len(), 1);
        assert_eq!(facts.env_file_decls[0].path, ".env.prod");
        assert!(matches!(facts.env_file_decls[0].loader, LoaderKind::StarletteConfig));
    }

    #[test]
    fn load_dotenv_file_decl() {
        let facts = run(r#"from dotenv import load_dotenv; load_dotenv("conf/.env")"#);
        assert_eq!(facts.env_file_decls.len(), 1);
        assert_eq!(facts.env_file_decls[0].path, "conf/.env");
    }

    #[test]
    fn load_dotenv_default_file() {
        let facts = run(r#"from dotenv import load_dotenv; load_dotenv()"#);
        assert_eq!(facts.env_file_decls.len(), 1);
        assert_eq!(facts.env_file_decls[0].path, ".env");
    }

    #[test]
    fn base_settings_fields() {
        let facts = run(r#"
from pydantic_settings import BaseSettings

class Settings(BaseSettings):
    database_url: str
    port: int = 8000
"#);
        assert_eq!(facts.settings_classes.len(), 1);
        let cls = &facts.settings_classes[0];
        assert_eq!(cls.class_name, "Settings");
        let keys: Vec<_> = cls.fields.iter().filter_map(|f| f.env_key.as_deref()).collect();
        assert!(keys.contains(&"DATABASE_URL"));
        assert!(keys.contains(&"PORT"));
        let port = cls.fields.iter().find(|f| f.field_name == "port").unwrap();
        assert!(port.has_default);
    }

    #[test]
    fn base_settings_with_prefix() {
        let facts = run(r#"
from pydantic_settings import BaseSettings, SettingsConfigDict

class AppSettings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="APP_")
    db_url: str
"#);
        assert_eq!(facts.settings_classes.len(), 1);
        let cls = &facts.settings_classes[0];
        assert_eq!(cls.class_name, "AppSettings");
        assert_eq!(cls.env_prefix.as_deref(), Some("APP_"));
        let keys: Vec<_> = cls.fields.iter().filter_map(|f| f.env_key.as_deref()).collect();
        assert!(keys.contains(&"APP_DB_URL"), "expected APP_DB_URL, got {keys:?}");
    }

    #[test]
    fn superclass_names_extracted() {
        let facts = run(r#"
from pydantic_settings import BaseSettings

class AppSettings(BaseSettings):
    api_key: str
"#);
        let cls = &facts.settings_classes[0];
        assert!(cls.superclass_names.contains(&"BaseSettings".to_owned()));
    }

    #[test]
    fn starlette_config_cast_is_not_default() {
        // config("KEY", str) — 2nd positional is cast type, not a default.
        // The diagnostic suppression must NOT fire.
        let facts = run(r#"
from starlette.config import Config
config = Config()
VALUE = config("MY_KEY", str)
"#);
        let lookup = facts.env_lookups.iter().find(|l| l.key == "MY_KEY").unwrap();
        assert!(!lookup.has_default, "cast arg must not be treated as default");
    }

    #[test]
    fn starlette_config_default_kwarg_is_default() {
        let facts = run(r#"
from starlette.config import Config
config = Config()
VALUE = config("MY_KEY", default="fallback")
"#);
        let lookup = facts.env_lookups.iter().find(|l| l.key == "MY_KEY").unwrap();
        assert!(lookup.has_default);
    }

    #[test]
    fn starlette_config_third_positional_is_default() {
        let facts = run(r#"
from starlette.config import Config
config = Config()
VALUE = config("MY_KEY", str, "fallback")
"#);
        let lookup = facts.env_lookups.iter().find(|l| l.key == "MY_KEY").unwrap();
        assert!(lookup.has_default);
    }

    #[test]
    fn base_settings_alias() {
        let facts = run(r#"
from pydantic_settings import BaseSettings
from pydantic import Field

class Settings(BaseSettings):
    db: str = Field(validation_alias="DATABASE_URL")
"#);
        assert_eq!(facts.settings_classes.len(), 1);
        let cls = &facts.settings_classes[0];
        let db = cls.fields.iter().find(|f| f.field_name == "db").unwrap();
        assert_eq!(db.env_key.as_deref(), Some("DATABASE_URL"));
    }
}
