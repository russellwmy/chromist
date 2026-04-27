//! Storage-state snapshot — cookies and `localStorage` per origin.
//!
//! [`StorageState`] is a JSON-serializable value that captures the browser
//! cookies and per-origin `localStorage` contents at a point in time.  It can
//! be serialized to disk, reloaded in a later test run, and applied to a fresh
//! browser via [`BrowserConfigBuilder::storage_state`] to skip re-authentication.

use serde::{Deserialize, Serialize};

use crate::cdp::browser_protocol::network as cdp_network;

/// A cookie snapshot compatible with `Network.CookieParam`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cookie {
    /// Cookie name (e.g. `"session_id"`).
    pub name: String,
    /// Cookie value (URL-decoded).
    pub value: String,
    /// Domain the cookie belongs to.
    pub domain: String,
    /// Path the cookie belongs to.
    pub path: String,
    /// Expiry as seconds since the Unix epoch.  `None` means session cookie.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires: Option<f64>,
    /// Whether the cookie is HTTP-only.
    pub http_only: bool,
    /// Whether the cookie is marked Secure.
    pub secure: bool,
    /// `SameSite` attribute as a string (`"Strict"`, `"Lax"`, `"None"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub same_site: Option<String>,
    /// Source URL (used when restoring cookies without an explicit domain).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// `localStorage` contents for a single origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginStorage {
    /// Origin string, e.g. `"https://example.com"`.
    pub origin: String,
    /// Key–value pairs from `localStorage`.
    pub local_storage: Vec<(String, String)>,
}

/// Full storage snapshot: cookies and per-origin `localStorage`.
///
/// # Examples
///
/// ```no_run
/// use chromist::{Browser, BrowserConfig, StorageState};
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let browser = Browser::launch(BrowserConfig::builder().build()).await?;
/// let ctx = browser.handle().default_browser_context();
/// let state = ctx.storage_state().await?;
/// let json = serde_json::to_string(&state)?;
/// std::fs::write("auth.json", &json)?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageState {
    /// All cookies present in the browser context.
    pub cookies: Vec<Cookie>,
    /// Per-origin `localStorage` snapshots.
    pub origins: Vec<OriginStorage>,
}

impl From<cdp_network::Cookie> for Cookie {
    fn from(c: cdp_network::Cookie) -> Self {
        Cookie {
            name: c.name,
            value: c.value,
            domain: c.domain,
            path: c.path,
            expires: if c.expires < 0.0 { None } else { Some(c.expires) },
            http_only: c.http_only,
            secure: c.secure,
            same_site: c.same_site.map(|s| s.as_ref().to_string()),
            url: None,
        }
    }
}

impl From<Cookie> for cdp_network::CookieParam {
    fn from(c: Cookie) -> Self {
        let mut p = cdp_network::CookieParam::new(c.name, c.value);
        p.domain = Some(c.domain);
        p.path = Some(c.path);
        p.expires = c.expires.map(cdp_network::TimeSinceEpoch::new);
        p.http_only = Some(c.http_only);
        p.secure = Some(c.secure);
        if let Some(ss) = c.same_site {
            p.same_site = ss.parse().ok();
        }
        if let Some(url) = c.url {
            p.url = Some(url);
        }
        p
    }
}
