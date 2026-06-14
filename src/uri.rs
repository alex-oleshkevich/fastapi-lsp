use std::path::{Path, PathBuf};
use tower_lsp_server::ls_types::Uri;

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let encoded = path
        .display()
        .to_string()
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('#', "%23")
        .replace('?', "%3F");
    format!("file://{encoded}").parse().ok()
}

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let s = uri.as_str().strip_prefix("file://")?;
    // Percent-decode common encodings that path_to_uri encodes
    let decoded = s
        .replace("%20", " ")
        .replace("%23", "#")
        .replace("%3F", "?")
        .replace("%3f", "?")
        .replace("%25", "%");
    Some(PathBuf::from(decoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_ascii_path() {
        let path = std::path::Path::new("/home/user/project/app.py");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(uri_to_path(&uri), Some(PathBuf::from("/home/user/project/app.py")));
    }

    #[test]
    fn roundtrip_path_with_space() {
        let path = std::path::Path::new("/home/user/my project/app.py");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(uri_to_path(&uri), Some(PathBuf::from("/home/user/my project/app.py")));
    }

    #[test]
    fn roundtrip_path_with_hash() {
        let path = std::path::Path::new("/home/user/my#project/app.py");
        let uri = path_to_uri(path).unwrap();
        assert_eq!(uri_to_path(&uri), Some(PathBuf::from("/home/user/my#project/app.py")));
    }

    #[test]
    fn uri_without_file_scheme_returns_none() {
        let uri: Uri = "https://example.com/file.py".parse().unwrap();
        assert!(uri_to_path(&uri).is_none());
    }
}
