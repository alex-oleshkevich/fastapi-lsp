use tower_lsp_server::ls_types::{Position, Range};
/// Extract OAuth2 security scheme sites — `tokenUrl` and `authorizationUrl` kwargs
/// from calls to recognized OAuth2 classes (OAuth2PasswordBearer, etc.).
use tree_sitter::{Node, Tree};

use super::unquote;
use crate::state::{FileFacts, SecuritySchemeSite, range_from_node};

const OAUTH2_CLASSES: &[&str] = &[
    "OAuth2PasswordBearer",
    "OAuth2AuthorizationCodeBearer",
    "OAuth2",
    "OAuth2ImplicitBearer",
];
const TOKEN_URL_KWARGS: &[&str] = &["tokenUrl", "authorizationUrl"];

pub fn extract(src: &[u8], tree: &Tree, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    walk(src, tree.root_node(), facts, enc);
}

fn walk(src: &[u8], node: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    if node.kind() == "call" {
        extract_call(src, node, facts, enc);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(src, child, facts, enc);
    }
}

fn extract_call(src: &[u8], call: Node<'_>, facts: &mut FileFacts, enc: crate::offset::Encoding) {
    let func = match call.child_by_field_name("function") {
        Some(n) => n,
        None => return,
    };

    let class_name = match func.kind() {
        "identifier" => node_text(src, func),
        "attribute" => func
            .child_by_field_name("attribute")
            .map(|n| node_text(src, n))
            .unwrap_or(""),
        _ => return,
    };

    if !OAUTH2_CLASSES.contains(&class_name) {
        return;
    }

    let args = match call.child_by_field_name("arguments") {
        Some(a) => a,
        None => return,
    };

    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        if child.kind() != "keyword_argument" {
            continue;
        }
        let key = child
            .child_by_field_name("name")
            .map(|n| node_text(src, n))
            .unwrap_or("");
        if !TOKEN_URL_KWARGS.contains(&key) {
            continue;
        }
        let val = match child.child_by_field_name("value") {
            Some(n) if n.kind() == "string" => n,
            _ => continue,
        };
        let raw = node_text(src, val);
        let unquoted = unquote(raw);
        if unquoted.is_empty() {
            continue;
        }
        let path = normalize_oauth2_url(&unquoted);
        let range = range_from_node(val, src, enc);
        let replace_range = string_content_range(val, raw);

        facts.security_scheme_sites.push(SecuritySchemeSite {
            path,
            range,
            replace_range,
        });
    }
}

/// Normalize an OAuth2 URL: prepend `/` if not already absolute.
fn normalize_oauth2_url(url: &str) -> String {
    if url.starts_with('/') {
        url.to_owned()
    } else {
        format!("/{url}")
    }
}

/// Range of the string content (excluding quotes and any string prefix).
fn string_content_range(node: Node<'_>, raw: &str) -> Range {
    let after_prefix = raw.trim_start_matches(['f', 'r', 'b', 'F', 'R', 'B']);
    let prefix_chars = (raw.len() - after_prefix.len()) as u32;
    let quote_chars: u32 = if after_prefix.starts_with("\"\"\"") || after_prefix.starts_with("'''")
    {
        3
    } else {
        1
    };
    let open_offset = prefix_chars + quote_chars;
    let start = node.start_position();
    let end = node.end_position();
    Range {
        start: Position::new(start.row as u32, start.column as u32 + open_offset),
        end: Position::new(end.row as u32, end.column as u32 - quote_chars),
    }
}

fn node_text<'a>(src: &'a [u8], node: Node<'_>) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offset::Encoding;
    use crate::parsing::parse_file;

    fn extract_sites(src: &str) -> Vec<SecuritySchemeSite> {
        let bytes = src.as_bytes();
        let tree = parse_file(bytes);
        let uri: tower_lsp_server::ls_types::Uri = "file:///app.py".parse().unwrap();
        let mut facts = FileFacts::new(uri);
        extract(bytes, &tree, &mut facts, Encoding::Utf8);
        facts.security_scheme_sites
    }

    #[test]
    fn oauth2_password_bearer_extracts_token_url() {
        let sites = extract_sites(r#"oauth2_scheme = OAuth2PasswordBearer(tokenUrl="token")"#);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].path, "/token");
    }

    #[test]
    fn absolute_token_url_not_double_prefixed() {
        let sites = extract_sites(r#"oauth2_scheme = OAuth2PasswordBearer(tokenUrl="/api/token")"#);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].path, "/api/token");
    }

    #[test]
    fn authorization_url_extracted() {
        let sites = extract_sites(
            r#"scheme = OAuth2AuthorizationCodeBearer(authorizationUrl="/auth/authorize", tokenUrl="/auth/token")"#,
        );
        assert_eq!(sites.len(), 2);
        let paths: Vec<&str> = sites.iter().map(|s| s.path.as_str()).collect();
        assert!(paths.contains(&"/auth/authorize"));
        assert!(paths.contains(&"/auth/token"));
    }

    #[test]
    fn non_oauth2_call_ignored() {
        let sites = extract_sites(r#"x = SomeClass(tokenUrl="token")"#);
        assert!(sites.is_empty(), "SomeClass is not an OAuth2 class");
    }

    #[test]
    fn replace_range_excludes_quotes() {
        let src = r#"oauth2_scheme = OAuth2PasswordBearer(tokenUrl="token")"#;
        let sites = extract_sites(src);
        assert_eq!(sites.len(), 1);
        let r = sites[0].replace_range;
        // "token" starts at col 46 (after `tokenUrl="`), ends at col 51
        assert_eq!(r.start.line, 0);
        // The opening quote of "token" is at col 46, content starts at 47
        let snippet = &src[r.start.character as usize..r.end.character as usize];
        assert_eq!(snippet, "token");
    }

    #[test]
    fn normalize_relative_url() {
        assert_eq!(normalize_oauth2_url("token"), "/token");
        assert_eq!(normalize_oauth2_url("/api/token"), "/api/token");
        assert_eq!(normalize_oauth2_url("auth/login"), "/auth/login");
    }
}
