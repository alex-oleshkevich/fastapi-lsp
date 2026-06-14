/// Parse `.env` file content into a list of key-value entries.
///
/// Handles:
///   - `KEY=value`
///   - `export KEY=value`
///   - `KEY="quoted value"` and `KEY='quoted value'`
///   - `# comment lines` and inline `# trailing comments` (outside quotes)
///   - Empty lines and whitespace-only lines
///   - Lines with no value (`KEY=`)
use tower_lsp_server::ls_types::{Position, Range, Uri};

use crate::state::{EnvEntry, Location};

#[derive(Debug, Clone)]
pub struct DotenvEntry {
    pub key: String,
    pub value: String,
    pub line: u32,
}

pub fn parse(src: &str, _uri: &Uri) -> Vec<DotenvEntry> {
    let mut entries = Vec::new();
    for (line_idx, line) in src.lines().enumerate() {
        let line_num = line_idx as u32;
        let trimmed = line.trim();

        // Skip blank lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Strip optional `export ` prefix
        let rest = trimmed.strip_prefix("export ").map(str::trim_start).unwrap_or(trimmed);

        let eq_pos = match rest.find('=') {
            Some(p) => p,
            None => continue, // no `=` — skip
        };

        let key = rest[..eq_pos].trim().to_owned();
        if key.is_empty() || !is_valid_key(&key) {
            continue;
        }

        let raw_value = rest[eq_pos + 1..].trim();
        let value = strip_quotes_and_comment(raw_value);

        entries.push(DotenvEntry { key, value, line: line_num });
    }
    entries
}

pub fn into_env_entries(entries: &[DotenvEntry], uri: &Uri) -> Vec<(String, EnvEntry)> {
    entries
        .iter()
        .map(|e| {
            let loc = Location {
                uri: uri.clone(),
                range: Range {
                    start: Position { line: e.line, character: 0 },
                    end: Position { line: e.line, character: e.key.len() as u32 },
                },
            };
            (
                e.key.clone(),
                EnvEntry {
                    value: e.value.clone(),
                    locations: vec![loc],
                    from_process_env: false,
                },
            )
        })
        .collect()
}

fn is_valid_key(key: &str) -> bool {
    key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

fn strip_quotes_and_comment(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // Quoted: consume until matching close quote, ignoring escaped chars
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        loop {
            match chars.next() {
                None | Some('"') => break,
                Some('\\') => {
                    if let Some(c) = chars.next() {
                        match c {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'r' => out.push('\r'),
                            other => out.push(other),
                        }
                    }
                }
                Some(c) => out.push(c),
            }
        }
        return out;
    }
    if let Some(rest) = s.strip_prefix('\'') {
        return rest.split_once('\'').map(|(v, _)| v).unwrap_or(rest).to_owned();
    }

    // Unquoted: strip inline comment (first unescaped `#` preceded by whitespace)
    let mut result = String::new();
    let chars = s.chars().peekable();
    for c in chars {
        if c == '#' {
            // Inline comment only if preceded by whitespace
            if result.ends_with(|c: char| c.is_whitespace()) {
                break;
            }
        }
        result.push(c);
    }
    result.trim_end().to_owned()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Uri {
        "file:///.env".parse().unwrap()
    }

    fn keys(src: &str) -> Vec<String> {
        parse(src, &uri()).into_iter().map(|e| e.key).collect()
    }

    fn kv(src: &str) -> Vec<(String, String)> {
        parse(src, &uri())
            .into_iter()
            .map(|e| (e.key, e.value))
            .collect()
    }

    #[test]
    fn simple_key_value() {
        let entries = kv("APP_NAME=bookshop\nDEBUG=true");
        assert_eq!(entries, vec![
            ("APP_NAME".to_owned(), "bookshop".to_owned()),
            ("DEBUG".to_owned(), "true".to_owned()),
        ]);
    }

    #[test]
    fn export_prefix_stripped() {
        let entries = kv("export SECRET=abc123");
        assert_eq!(entries[0], ("SECRET".to_owned(), "abc123".to_owned()));
    }

    #[test]
    fn comment_lines_skipped() {
        let k = keys("# comment\nKEY=val\n# another");
        assert_eq!(k, vec!["KEY"]);
    }

    #[test]
    fn double_quoted_value() {
        let entries = kv(r#"MSG="hello world""#);
        assert_eq!(entries[0].1, "hello world");
    }

    #[test]
    fn single_quoted_value() {
        let entries = kv("MSG='hello world'");
        assert_eq!(entries[0].1, "hello world");
    }

    #[test]
    fn inline_comment_stripped() {
        let entries = kv("KEY=value # comment");
        assert_eq!(entries[0].1, "value");
    }

    #[test]
    fn empty_value() {
        let entries = kv("EMPTY=");
        assert_eq!(entries[0], ("EMPTY".to_owned(), "".to_owned()));
    }

    #[test]
    fn blank_lines_and_whitespace_ignored() {
        let k = keys("\n  \nKEY=val\n\n");
        assert_eq!(k, vec!["KEY"]);
    }

    #[test]
    fn line_numbers_correct() {
        let src = "# header\nA=1\nB=2";
        let entries = parse(src, &uri());
        assert_eq!(entries[0].line, 1);
        assert_eq!(entries[1].line, 2);
    }

    #[test]
    fn escape_sequences_in_double_quotes() {
        let entries = kv(r#"MSG="line1\nline2""#);
        assert_eq!(entries[0].1, "line1\nline2");
    }
}
