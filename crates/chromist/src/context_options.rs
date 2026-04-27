//! Options for creating a new browser context.

use std::path::PathBuf;

/// Options passed to [`Browser::new_context_with_options`](crate::Browser::new_context_with_options) or
/// [`Browser::launch_persistent_context`](crate::Browser::launch_persistent_context).
///
/// Build with [`BrowserContextOptions::builder()`] for a fluent API.
#[derive(Debug, Clone)]
pub struct BrowserContextOptions {
    /// HTTP proxy URL, e.g. `http://proxy.example.com:8080`.
    pub proxy_server: Option<String>,
    /// Comma-separated list of hosts to bypass the proxy.
    pub proxy_bypass_list: Option<String>,
    /// Bypass Content-Security-Policy restrictions on every page.
    pub bypass_csp: bool,
    /// HTTP Basic Auth credentials applied to every page via `Authorization`.
    pub http_credentials: Option<HttpCredentials>,
    /// Whether JavaScript is enabled in the context (default `true`).
    pub javascript_enabled: bool,
    /// Service worker handling policy (default `Allow`).
    pub service_workers: ServiceWorkersPolicy,
    /// Directory to save downloaded files.  When set, every new page has
    /// its download behaviour set to `allow` with this path via CDP.
    pub downloads_path: Option<PathBuf>,
}

impl Default for BrowserContextOptions {
    fn default() -> Self {
        Self {
            proxy_server: None,
            proxy_bypass_list: None,
            bypass_csp: false,
            http_credentials: None,
            javascript_enabled: true,
            service_workers: ServiceWorkersPolicy::Allow,
            downloads_path: None,
        }
    }
}

impl BrowserContextOptions {
    /// Returns a fresh builder seeded with [`BrowserContextOptions::default`].
    #[must_use]
    pub fn builder() -> BrowserContextOptionsBuilder {
        BrowserContextOptionsBuilder::default()
    }
}

/// Fluent builder for [`BrowserContextOptions`](crate::BrowserContextOptions).
#[derive(Debug, Default)]
pub struct BrowserContextOptionsBuilder {
    inner: BrowserContextOptions,
}

impl BrowserContextOptionsBuilder {
    /// HTTP proxy URL applied to every page in this context — e.g.
    /// `"http://proxy.corp:8080"`.
    pub fn proxy(mut self, server: impl Into<String>) -> Self {
        self.inner.proxy_server = Some(server.into());
        self
    }
    /// Comma-separated list of hosts to bypass the proxy
    /// (`"localhost,*.internal"` style).
    pub fn proxy_bypass_list(mut self, bypass: impl Into<String>) -> Self {
        self.inner.proxy_bypass_list = Some(bypass.into());
        self
    }
    /// Bypass Content-Security-Policy on every page in this context. Lets
    /// init scripts and `eval` execute on pages with strict CSP headers.
    pub fn bypass_csp(mut self) -> Self {
        self.inner.bypass_csp = true;
        self
    }
    /// Inject HTTP Basic Auth credentials via the `Authorization` request
    /// header on every page.
    pub fn http_credentials(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.inner.http_credentials = Some(HttpCredentials::new(username, password));
        self
    }
    /// Disable JavaScript execution on pages in this context (CDP
    /// `Emulation.setScriptExecutionDisabled(true)`).
    pub fn javascript_disabled(mut self) -> Self {
        self.inner.javascript_enabled = false;
        self
    }
    /// Block service-worker registration in this context. Default policy
    /// is [`ServiceWorkersPolicy::Allow`].
    pub fn block_service_workers(mut self) -> Self {
        self.inner.service_workers = ServiceWorkersPolicy::Block;
        self
    }
    /// Directory where downloaded files in this context are saved
    /// (`Browser.setDownloadBehavior(allow, path)`).
    pub fn downloads_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner.downloads_path = Some(path.into());
        self
    }
    /// Consume the builder and return the populated [`BrowserContextOptions`].
    #[must_use]
    pub fn build(self) -> BrowserContextOptions {
        self.inner
    }
}

/// HTTP Basic Auth credentials injected into every page via the `Authorization`
/// request header.
#[derive(Debug, Clone)]
pub struct HttpCredentials {
    /// Username for the Basic-auth pair.
    pub username: String,
    /// Password for the Basic-auth pair.
    pub password: String,
}

impl HttpCredentials {
    /// Construct credentials from username and password.
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self { username: username.into(), password: password.into() }
    }

    /// Encode as the value of an HTTP `Authorization: Basic …` header.
    pub fn as_header_value(&self) -> String {
        use base64::Engine;
        let raw = format!("{}:{}", self.username, self.password);
        let encoded = base64::engine::general_purpose::STANDARD.encode(raw);
        format!("Basic {encoded}")
    }
}

/// Controls whether service workers are allowed in a browser context.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ServiceWorkersPolicy {
    /// Service workers run normally (default).
    #[default]
    Allow,
    /// Block service worker registration via an init-script override.
    Block,
}
