//! The closed failure list of the `browser` cell.
//!
//! No `thiserror` here: `meclaw-cells` does not depend on it, and the shape
//! this crate established is a plain enum plus `error_code()` and `detail()`
//! (`stdio_child::error`, `mcp::wire::McpError`). `error_code` is a public
//! contract string (`docs/stability.md`), so the list is closed and its order
//! is the order [`ERROR_CODES`] publishes.

/// Every `error_code` this cell type can emit. Closed set, nine of them.
///
/// A tenth would be a contract change, and there is deliberately not one for
/// "the sandbox did not hold": that is a spawn that failed, and it says so
/// (`spawn_failed`, with the reason in the detail).
pub const ERROR_CODES: &[&str; 9] = &[
    "invalid_input",
    "unknown_page",
    "too_many_pages",
    "spawn_failed",
    "startup_timeout",
    "browser_crashed",
    "navigate_failed",
    "cdp_timeout",
    "client_too_slow",
];

/// What went wrong in a `browser` cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserError {
    /// A message, a param or a link frame the cell could not read.
    InvalidInput(String),
    /// A verb, a join or a frame named a page this cell does not hold.
    UnknownPage(String),
    /// `max_pages` is reached and nothing suspended could be given up.
    TooManyPages {
        /// The cap that was reached.
        max: u32,
    },
    /// The browser process could not be started, or started without its own
    /// sandbox. Fail-closed: there is no page on the other side of this.
    SpawnFailed(String),
    /// The browser started but never answered `Browser.getVersion`.
    StartupTimeout {
        /// The deadline that elapsed, in milliseconds.
        ms: u64,
    },
    /// The browser died, or a renderer did.
    BrowserCrashed(String),
    /// A navigation was refused by the page or by the browser.
    NavigateFailed(String),
    /// One CDP round trip ran past `external_timeout_ms` (A-timeout).
    CdpTimeout {
        /// The method nobody answered.
        method: String,
    },
    /// A viewer stopped reading its frames and was given up.
    ClientTooSlow,
}

impl BrowserError {
    /// The public contract string. One of [`ERROR_CODES`], always.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::UnknownPage(_) => "unknown_page",
            Self::TooManyPages { .. } => "too_many_pages",
            Self::SpawnFailed(_) => "spawn_failed",
            Self::StartupTimeout { .. } => "startup_timeout",
            Self::BrowserCrashed(_) => "browser_crashed",
            Self::NavigateFailed(_) => "navigate_failed",
            Self::CdpTimeout { .. } => "cdp_timeout",
            Self::ClientTooSlow => "client_too_slow",
        }
    }

    /// Short, cause-carrying, and written for whoever reads the emission.
    pub fn detail(&self) -> String {
        match self {
            Self::InvalidInput(e) => e.clone(),
            Self::UnknownPage(page) => format!("no page {page:?} is open on this browser"),
            Self::TooManyPages { max } => format!(
                "{max} pages are open and none of them is suspended; close one before opening another"
            ),
            Self::SpawnFailed(e) => e.clone(),
            Self::StartupTimeout { ms } => {
                format!("the browser did not answer Browser.getVersion within {ms} ms")
            }
            Self::BrowserCrashed(e) => e.clone(),
            Self::NavigateFailed(e) => e.clone(),
            Self::CdpTimeout { method } => format!("{method} was not answered in time"),
            Self::ClientTooSlow => {
                "the viewer stopped reading its frames and was given up".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_names_one_of_the_nine_codes() {
        let all = [
            BrowserError::InvalidInput("x".into()),
            BrowserError::UnknownPage("card-1".into()),
            BrowserError::TooManyPages { max: 8 },
            BrowserError::SpawnFailed("x".into()),
            BrowserError::StartupTimeout { ms: 20_000 },
            BrowserError::BrowserCrashed("x".into()),
            BrowserError::NavigateFailed("x".into()),
            BrowserError::CdpTimeout {
                method: "Page.navigate".into(),
            },
            BrowserError::ClientTooSlow,
        ];
        for e in &all {
            assert!(
                ERROR_CODES.contains(&e.error_code()),
                "{:?} emits an undeclared code",
                e
            );
            assert!(!e.detail().is_empty(), "{e:?} says nothing");
        }
        let seen: Vec<&str> = all.iter().map(BrowserError::error_code).collect();
        assert_eq!(seen, ERROR_CODES.to_vec(), "nine variants, nine codes");
    }
}
