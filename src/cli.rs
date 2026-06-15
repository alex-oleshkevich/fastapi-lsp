use std::net::IpAddr;
use std::path::PathBuf;

use clap::{Args, ValueEnum};

#[derive(Args, Debug)]
pub struct RoutesArgs {
    /// Path to analyse (file or directory; directory is treated as workspace root)
    #[arg(default_value = ".")]
    pub path: PathBuf,
    /// Output format
    #[arg(long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Args, Debug)]
pub struct LspArgs {
    /// Use TCP transport instead of stdio
    #[arg(long)]
    pub tcp: bool,
    /// TCP listen address (only with --tcp)
    #[arg(long, default_value = "127.0.0.1", requires = "tcp")]
    pub address: IpAddr,
    /// TCP listen port (only with --tcp)
    #[arg(long, default_value_t = 9257, requires = "tcp")]
    pub port: u16,
}

#[derive(Args, Debug)]
pub struct CheckArgs {
    /// Path to analyse (file or directory; directory is treated as workspace root)
    #[arg(default_value = ".")]
    pub path: PathBuf,
    /// Output format
    #[arg(long, value_enum, default_value = "text")]
    pub format: OutputFormat,
    /// Run only these diagnostic codes (comma-separated); mutually exclusive with --ignore
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<DiagCode>,
    /// Skip these diagnostic codes (comma-separated); mutually exclusive with --only
    #[arg(long, value_delimiter = ',')]
    pub ignore: Vec<DiagCode>,
    /// Apply deterministic quick fixes in-place
    #[arg(long)]
    pub fix: bool,
}

#[derive(Debug, Clone, ValueEnum, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

/// A validated diagnostic code string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagCode(pub String);

/// All diagnostic codes emitted by this server (single source of truth for --only/--ignore validation).
pub const KNOWN_CODES: &[&str] = &[
    "di/cycle",
    "di/depends-called",
    "di/override-unused",
    "route/duplicate",
    "route/shadowed",
    "route/duplicate-name",
    "route/param-missing-arg",
    "route/arg-missing-param",
    "route/router-not-included",
    "url/unknown-name",
    "url/param-mismatch",
    "model/unknown-response-model",
    "env/undefined-key",
    "tpl/missing-template",
    "test/unknown-path",
    "oauth2/unknown-token-url",
];

impl std::str::FromStr for DiagCode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if KNOWN_CODES.contains(&s) {
            Ok(DiagCode(s.to_owned()))
        } else {
            Err(format!(
                "unknown diagnostic code '{s}'; valid codes are: {}",
                KNOWN_CODES.join(", ")
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_parse_ok() {
        for code in KNOWN_CODES {
            let result: Result<DiagCode, _> = code.parse();
            assert!(result.is_ok(), "known code '{code}' should parse");
            assert_eq!(result.unwrap().0, *code);
        }
    }

    #[test]
    fn unknown_code_returns_error() {
        let result: Result<DiagCode, _> = "not/a/real-code".parse();
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("not/a/real-code"));
        assert!(msg.contains("valid codes are"));
    }

    #[test]
    fn typo_is_rejected() {
        let result: Result<DiagCode, _> = "di/cycl".parse();
        assert!(result.is_err());
    }

    #[test]
    fn known_codes_list_non_empty() {
        assert!(!KNOWN_CODES.is_empty());
        assert!(KNOWN_CODES.contains(&"tpl/missing-template"));
        assert!(KNOWN_CODES.contains(&"di/cycle"));
        assert!(KNOWN_CODES.contains(&"url/unknown-name"));
    }
}
