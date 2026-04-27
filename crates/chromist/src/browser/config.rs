use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::layout::Viewport;
use crate::storage::StorageState;

/// How to run Chrome's renderer — headed, legacy headless, or the modern
/// "new" headless mode.
///
/// # Examples
///
/// ```
/// use chromist::HeadlessMode;
/// assert_eq!(HeadlessMode::default(), HeadlessMode::New);
/// // Legacy alias.
/// assert_eq!(HeadlessMode::True, HeadlessMode::Legacy);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum HeadlessMode {
    /// Run with a visible browser window — no headless flag.
    False,
    /// Legacy `--headless` flag.
    Legacy,
    /// Modern `--headless=new` flag (default).
    #[default]
    New,
}

impl HeadlessMode {
    /// Alias for [`HeadlessMode::Legacy`]. Prefer [`HeadlessMode::Legacy`].
    #[allow(non_upper_case_globals)]
    pub const True: HeadlessMode = HeadlessMode::Legacy;
}

/// Settings applied when launching a browser.
///
/// Construct via [`BrowserConfig::builder`] for fluent setup or
/// [`BrowserConfig::default`] for defaults. The builder is the recommended
/// path; mutating fields directly is supported but bypasses validation that
/// future versions may add.
#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// Explicit path to the Chrome / Chromium executable. When `None`,
    /// chromist auto-detects (see the `detection` field for knobs).
    pub executable: Option<PathBuf>,
    /// How to run the renderer (headed, legacy headless, or new headless).
    /// Default: [`HeadlessMode::New`].
    pub headless: HeadlessMode,
    /// Persistent profile directory passed via `--user-data-dir=…`. When
    /// `None`, chromist creates a tempdir and removes it on drop.
    pub user_data_dir: Option<PathBuf>,
    /// TCP port for `--remote-debugging-port`. `0` lets Chromium pick a free
    /// port (recommended).
    pub port: u16,
    /// When `false`, passes `--no-sandbox`. Required in some Linux containers
    /// but reduces process isolation.
    pub sandbox: bool,
    /// Extra command-line arguments appended after the chromist defaults.
    pub args: Vec<String>,
    /// Maximum time to wait for the browser to print its WebSocket URL on
    /// stderr after `spawn`. Default 20 s.
    pub launch_timeout: Duration,
    /// Default per-CDP-command timeout used by [`Handler`](crate::HandlerHandle).
    /// Default 30 s.
    pub request_timeout: Duration,
    /// Initial viewport for newly created pages. `None` uses Chromium's own
    /// default. Default `Some(1280×720)`.
    pub viewport: Option<Viewport>,
    /// Override the OS window size via `--window-size=W,H`. When set, takes
    /// precedence over [`Self::viewport`] for the host window dimensions.
    pub window_size: Option<(u32, u32)>,
    /// Pass `--incognito` so the default context is non-persistent.
    pub incognito: bool,
    /// Paths passed via `--load-extension=…` and `--disable-extensions-except=…`.
    pub extensions: Vec<PathBuf>,
    /// Environment variables set on the spawned browser process.
    pub process_envs: HashMap<String, String>,
    /// Skip the chromist-supplied default arguments entirely.
    pub ignore_default_args: bool,
    /// Suppress specific default arguments while keeping the rest.
    pub disable_default_args: Vec<String>,
    /// When `false`, every page disables HTTP cache via
    /// `Network.setCacheDisabled`. Default `true`.
    pub cache_enabled: bool,
    /// Pass `--ignore-certificate-errors` so the browser accepts invalid
    /// HTTPS certificates.
    pub respect_https_errors: bool,
    /// Enable `Fetch.enable` interception by default on every new page.
    /// Most users should rely on the per-page `route()` API instead.
    pub request_intercept: bool,
    /// Whether to surface CDP messages chromist could not deserialize as
    /// errors. When `false` (default) they are logged at `trace!` and dropped.
    pub surface_invalid_messages: bool,
    /// Knobs controlling executable auto-detection.
    pub detection: crate::detection::DetectionOptions,
    /// Use `--remote-debugging-pipe` (OS pipes) instead of
    /// `--remote-debugging-port` (WebSocket).
    ///
    /// Pipe mode avoids open TCP ports and is more secure. Only supported on
    /// Unix targets.
    pub use_pipe: bool,
    /// Optional storage-state snapshot to restore (cookies + localStorage)
    /// into the default browser context right after the browser connects.
    pub storage_state: Option<StorageState>,
    /// HTTP proxy URL applied to the browser process via `--proxy-server=…`.
    /// For per-context proxy, use [`BrowserContextOptions::proxy_server`](crate::BrowserContextOptions).
    pub proxy_server: Option<String>,
    /// Comma-separated proxy bypass list (`--proxy-bypass-list=…`).
    pub proxy_bypass_list: Option<String>,
    /// Delay inserted between actions for slow-motion debugging.
    /// Stored for external use; action-level injection is not yet implemented.
    pub slow_mo: Option<Duration>,
    /// Default download directory.  When set, [`Browser::launch_persistent_context`](crate::Browser::launch_persistent_context)
    /// and context creation will configure downloads to `allow` with this path.
    pub downloads_path: Option<PathBuf>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig {
            executable: None,
            headless: HeadlessMode::default(),
            user_data_dir: None,
            port: 0,
            sandbox: true,
            args: Vec::new(),
            launch_timeout: Duration::from_secs(20),
            request_timeout: Duration::from_secs(30),
            viewport: Some(Viewport::new(1280, 720)),
            window_size: None,
            incognito: false,
            extensions: Vec::new(),
            process_envs: HashMap::new(),
            ignore_default_args: false,
            disable_default_args: Vec::new(),
            cache_enabled: true,
            respect_https_errors: false,
            request_intercept: false,
            surface_invalid_messages: false,
            detection: crate::detection::DetectionOptions::default(),
            use_pipe: false,
            storage_state: None,
            proxy_server: None,
            proxy_bypass_list: None,
            slow_mo: None,
            downloads_path: None,
        }
    }
}

impl BrowserConfig {
    /// Returns a new builder with default settings.
    ///
    /// # Examples
    ///
    /// ```
    /// use chromist::BrowserConfig;
    /// let cfg = BrowserConfig::builder()
    ///     .no_sandbox()
    ///     .window_size(1920, 1080)
    ///     .build();
    /// assert!(!cfg.sandbox);
    /// assert_eq!(cfg.window_size, Some((1920, 1080)));
    /// ```
    #[must_use]
    pub fn builder() -> BrowserConfigBuilder {
        BrowserConfigBuilder::default()
    }

    /// Returns true when running in any headless mode (`Legacy` or `New`).
    pub fn is_headless(&self) -> bool {
        !matches!(self.headless, HeadlessMode::False)
    }

    /// Resolve the browser executable path: explicit `executable` field if
    /// set, else auto-detect via `crate::detection`. Returns
    /// [`crate::error::CdpError::LaunchNotFound`] if neither yields a path.
    pub fn resolve_executable(&self) -> crate::Result<PathBuf> {
        if let Some(ref p) = self.executable {
            return Ok(p.clone());
        }
        crate::detection::detect_executable_with_options(&self.detection)
            .ok_or(crate::error::CdpError::LaunchNotFound)
    }
}

/// Fluent builder for [`BrowserConfig`].
///
/// Obtain via [`BrowserConfig::builder`]. Methods consume `self` and
/// return `Self` so calls chain.
#[derive(Debug, Default)]
pub struct BrowserConfigBuilder {
    inner: BrowserConfig,
}

impl BrowserConfigBuilder {
    /// Override the auto-detected Chrome / Chromium executable with an
    /// explicit path.
    pub fn executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner.executable = Some(path.into());
        self
    }
    /// Set the headless mode explicitly. See [`HeadlessMode`].
    pub fn headless(mut self, mode: HeadlessMode) -> Self {
        self.inner.headless = mode;
        self
    }
    /// Run with a visible window — shorthand for
    /// `headless(HeadlessMode::False)`.
    pub fn with_head(mut self) -> Self {
        self.inner.headless = HeadlessMode::False;
        self
    }
    /// Persistent profile directory. When set, `--user-data-dir=…` is passed
    /// to Chromium and chromist does **not** create or remove the directory.
    pub fn user_data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner.user_data_dir = Some(path.into());
        self
    }
    /// TCP port for `--remote-debugging-port`. Pass `0` (the default) to let
    /// Chromium pick a free port.
    pub fn port(mut self, port: u16) -> Self {
        self.inner.port = port;
        self
    }
    /// Pass `--no-sandbox`. Required in some Linux containers; reduces
    /// process isolation.
    pub fn no_sandbox(mut self) -> Self {
        self.inner.sandbox = false;
        self
    }
    /// Append a single command-line argument.
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.inner.args.push(arg.into());
        self
    }
    /// Append multiple command-line arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inner.args.extend(args.into_iter().map(Into::into));
        self
    }
    /// Set the viewport. Pass `None` to use the browser's own default instead of
    /// the chromist default of 1280×720.
    pub fn viewport(mut self, viewport: impl Into<Option<Viewport>>) -> Self {
        self.inner.viewport = viewport.into();
        self
    }
    /// Override the OS window size via `--window-size=W,H`. Distinct from
    /// the rendering viewport.
    pub fn window_size(mut self, width: u32, height: u32) -> Self {
        self.inner.window_size = Some((width, height));
        self
    }
    /// Pass `--incognito` to start with a non-persistent default context.
    pub fn incognito(mut self) -> Self {
        self.inner.incognito = true;
        self
    }
    /// Add an unpacked extension directory to load via `--load-extension=…`.
    pub fn extension(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner.extensions.push(path.into());
        self
    }
    /// Set an environment variable on the browser child process.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.process_envs.insert(key.into(), value.into());
        self
    }
    /// Skip every chromist-supplied default argument.
    pub fn ignore_default_args(mut self) -> Self {
        self.inner.ignore_default_args = true;
        self
    }
    /// Suppress one specific default argument while keeping the rest.
    pub fn disable_default_arg(mut self, arg: impl Into<String>) -> Self {
        self.inner.disable_default_args.push(arg.into());
        self
    }
    /// Maximum time to wait for the browser to print its WebSocket URL on
    /// stderr. Default 20 s.
    pub fn launch_timeout(mut self, d: Duration) -> Self {
        self.inner.launch_timeout = d;
        self
    }
    /// Default timeout for any single CDP command. Default 30 s.
    pub fn request_timeout(mut self, d: Duration) -> Self {
        self.inner.request_timeout = d;
        self
    }
    /// Toggle the HTTP cache for newly created pages.
    pub fn cache(mut self, enabled: bool) -> Self {
        self.inner.cache_enabled = enabled;
        self
    }
    /// Pass `--ignore-certificate-errors` so HTTPS certificate failures are
    /// ignored.
    pub fn respect_https_errors(mut self) -> Self {
        self.inner.respect_https_errors = true;
        self
    }
    /// Enable `Fetch.enable` interception by default on every new page.
    /// Most callers should use `Page::route` instead.
    pub fn request_intercept(mut self, enabled: bool) -> Self {
        self.inner.request_intercept = enabled;
        self
    }

    /// Add multiple extensions in one call.
    pub fn extensions<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<PathBuf>,
    {
        self.inner.extensions.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Set multiple environment variables in one call.
    pub fn envs<I, K, V>(mut self, envs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        for (k, v) in envs {
            self.inner.process_envs.insert(k.into(), v.into());
        }
        self
    }

    /// Append `--disable-blink-features=AutomationControlled` so
    /// `navigator.webdriver` is `false`. A weak signal — pages that
    /// fingerprint chromist via other vectors will still detect automation.
    pub fn hide(mut self) -> Self {
        self.inner.args.push("--disable-blink-features=AutomationControlled".into());
        self
    }

    /// Disable Chromium's HTTPS-first auto-upgrade feature, which can
    /// interfere with HTTP-only test fixtures.
    pub fn disable_https_first(mut self) -> Self {
        self.inner
            .args
            .push("--disable-features=HttpsUpgrades,HttpsFirstBalancedModeAutoEnable".into());
        self
    }

    /// Treat undeserialisable CDP messages as errors instead of dropping
    /// them at `trace!` level.
    pub fn surface_invalid_messages(mut self) -> Self {
        self.inner.surface_invalid_messages = true;
        self
    }

    /// Override the executable auto-detection knobs (which channels to
    /// search, whether to consider unstable / Edge installations).
    pub fn chrome_detection(mut self, opts: crate::detection::DetectionOptions) -> Self {
        self.inner.detection = opts;
        self
    }

    /// Use OS-pipe transport (`--remote-debugging-pipe`) instead of a TCP
    /// WebSocket.
    ///
    /// Pipe mode avoids open TCP ports and is more secure. Only supported on
    /// Unix targets. Calling this on unsupported platforms will produce a
    /// [`CdpError::LaunchExit`](crate::CdpError::LaunchExit) at launch time.
    pub fn use_pipe(mut self) -> Self {
        self.inner.use_pipe = true;
        self
    }

    /// Restore a [`StorageState`] snapshot into the default context after the
    /// browser connects.  Cookies are applied immediately; `localStorage` is
    /// not restored automatically (use `Page::eval` after navigation instead).
    pub fn storage_state(mut self, state: StorageState) -> Self {
        self.inner.storage_state = Some(state);
        self
    }

    /// HTTP proxy URL applied to the browser process via `--proxy-server=…`.
    pub fn proxy(mut self, server: impl Into<String>) -> Self {
        self.inner.proxy_server = Some(server.into());
        self
    }

    /// Comma-separated proxy bypass list (`--proxy-bypass-list=…`).
    pub fn proxy_bypass_list(mut self, bypass: impl Into<String>) -> Self {
        self.inner.proxy_bypass_list = Some(bypass.into());
        self
    }

    /// Slow-motion delay between actions (stored for external use; action-level
    /// injection is not yet implemented).
    pub fn slow_mo(mut self, delay: Duration) -> Self {
        self.inner.slow_mo = Some(delay);
        self
    }

    /// Default download directory applied via `Browser.setDownloadBehavior`.
    pub fn downloads_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner.downloads_path = Some(path.into());
        self
    }

    #[must_use]
    /// Consume the builder and return the populated [`BrowserConfig`].
    pub fn build(self) -> BrowserConfig {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_headless_new() {
        let cfg = BrowserConfig::default();
        assert!(cfg.is_headless());
        assert_eq!(cfg.headless, HeadlessMode::New);
        assert!(cfg.sandbox);
        assert!(cfg.cache_enabled);
        assert!(!cfg.incognito);
        assert!(!cfg.use_pipe);
        assert_eq!(cfg.port, 0);
    }

    #[test]
    fn with_head_disables_headless() {
        let cfg = BrowserConfig::builder().with_head().build();
        assert!(!cfg.is_headless());
        assert_eq!(cfg.headless, HeadlessMode::False);
    }

    #[test]
    fn legacy_alias_matches_legacy_variant() {
        assert_eq!(HeadlessMode::True, HeadlessMode::Legacy);
    }

    #[test]
    fn builder_arg_and_args_accumulate() {
        let cfg = BrowserConfig::builder().arg("--foo").args(["--bar", "--baz"]).build();
        assert_eq!(cfg.args, vec!["--foo", "--bar", "--baz"]);
    }

    #[test]
    fn builder_no_sandbox_flag() {
        let cfg = BrowserConfig::builder().no_sandbox().build();
        assert!(!cfg.sandbox);
    }

    #[test]
    fn builder_envs_merge() {
        let cfg = BrowserConfig::builder().env("A", "1").envs([("B", "2"), ("C", "3")]).build();
        assert_eq!(cfg.process_envs.get("A").map(String::as_str), Some("1"));
        assert_eq!(cfg.process_envs.get("B").map(String::as_str), Some("2"));
        assert_eq!(cfg.process_envs.get("C").map(String::as_str), Some("3"));
    }

    #[test]
    fn builder_extensions_accumulate() {
        let cfg =
            BrowserConfig::builder().extension("/ext/a").extensions(["/ext/b", "/ext/c"]).build();
        assert_eq!(cfg.extensions.len(), 3);
    }

    #[test]
    fn cache_toggles() {
        let off = BrowserConfig::builder().cache(false).build();
        assert!(!off.cache_enabled);
        let on = BrowserConfig::builder().cache(true).build();
        assert!(on.cache_enabled);
    }

    #[test]
    fn request_intercept_toggles() {
        let on = BrowserConfig::builder().request_intercept(true).build();
        assert!(on.request_intercept);
        let off = BrowserConfig::builder().request_intercept(false).build();
        assert!(!off.request_intercept);
    }

    #[test]
    fn ignore_default_args_flag() {
        let cfg = BrowserConfig::builder().ignore_default_args().build();
        assert!(cfg.ignore_default_args);
    }

    #[test]
    fn hide_adds_blink_flag() {
        let cfg = BrowserConfig::builder().hide().build();
        assert!(cfg.args.iter().any(|a| a == "--disable-blink-features=AutomationControlled"));
    }

    #[test]
    fn disable_https_first_adds_feature_flag() {
        let cfg = BrowserConfig::builder().disable_https_first().build();
        assert!(cfg.args.iter().any(|a| a.contains("HttpsUpgrades")));
    }

    #[test]
    fn viewport_optional_none() {
        let cfg = BrowserConfig::builder().viewport(None).build();
        assert!(cfg.viewport.is_none());
    }

    #[test]
    fn window_size_round_trip() {
        let cfg = BrowserConfig::builder().window_size(1920, 1080).build();
        assert_eq!(cfg.window_size, Some((1920, 1080)));
    }

    #[test]
    fn port_and_executable() {
        let cfg = BrowserConfig::builder().port(9222).executable("/usr/bin/chromium").build();
        assert_eq!(cfg.port, 9222);
        assert_eq!(cfg.executable.as_deref(), Some(std::path::Path::new("/usr/bin/chromium")));
    }

    #[test]
    fn timeouts_are_configurable() {
        use std::time::Duration;
        let cfg = BrowserConfig::builder()
            .launch_timeout(Duration::from_secs(5))
            .request_timeout(Duration::from_secs(15))
            .build();
        assert_eq!(cfg.launch_timeout, Duration::from_secs(5));
        assert_eq!(cfg.request_timeout, Duration::from_secs(15));
    }

    #[test]
    fn incognito_and_pipe_flags() {
        let cfg = BrowserConfig::builder().incognito().use_pipe().build();
        assert!(cfg.incognito);
        assert!(cfg.use_pipe);
    }

    #[test]
    fn surface_invalid_messages_flag() {
        let cfg = BrowserConfig::builder().surface_invalid_messages().build();
        assert!(cfg.surface_invalid_messages);
    }

    #[test]
    fn respect_https_errors_flag() {
        let cfg = BrowserConfig::builder().respect_https_errors().build();
        assert!(cfg.respect_https_errors);
    }

    #[test]
    fn disable_default_arg_accumulates() {
        let cfg = BrowserConfig::builder()
            .disable_default_arg("--disable-extensions")
            .disable_default_arg("--disable-sync")
            .build();
        assert_eq!(cfg.disable_default_args.len(), 2);
    }
}
