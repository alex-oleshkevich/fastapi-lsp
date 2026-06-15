pub mod annotated;
pub mod clients;
pub mod deps;
pub mod dotenv;
pub mod env;
pub mod middleware;
pub mod models;
pub mod python;
pub mod routes;
pub mod security;
pub mod templates;
pub mod url_for;

pub use python::parse_file;

/// Strip Python string prefixes (f/r/b and uppercase variants) and surrounding
/// quotes. Returns the inner content, or the input unchanged if no quotes are
/// detected. Guards against bare `"""` / `'''` (length == 3) which would
/// otherwise produce a reversed slice range and panic.
pub(crate) fn unquote(s: &str) -> String {
    let s = s.trim().trim_start_matches(['f', 'r', 'b', 'F', 'R', 'B']);
    if s.len() >= 6
        && ((s.starts_with("\"\"\"") && s.ends_with("\"\"\""))
            || (s.starts_with("'''") && s.ends_with("'''")))
    {
        s[3..s.len() - 3].to_owned()
    } else if s.len() >= 2
        && !s.starts_with("\"\"\"")
        && !s.starts_with("'''")
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        s[1..s.len() - 1].to_owned()
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_bare_triple_double_quote_does_not_panic() {
        assert_eq!(unquote("\"\"\""), "\"\"\"");
    }

    #[test]
    fn unquote_bare_triple_single_quote_does_not_panic() {
        assert_eq!(unquote("'''"), "'''");
    }

    #[test]
    fn unquote_valid_triple_double_quote() {
        assert_eq!(unquote("\"\"\"hello\"\"\""), "hello");
    }

    #[test]
    fn unquote_valid_triple_single_quote() {
        assert_eq!(unquote("'''world'''"), "world");
    }

    #[test]
    fn unquote_single_quote() {
        assert_eq!(unquote("\"hi\""), "hi");
    }

    #[test]
    fn unquote_strips_f_prefix() {
        assert_eq!(unquote("f\"hello\""), "hello");
    }

    #[test]
    fn unquote_empty_string_literal() {
        assert_eq!(unquote("\"\""), "");
    }
}
