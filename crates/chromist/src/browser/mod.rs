//! Browser process launch, lifetime, and top-level handle.
//!
//! [`Browser::launch`] spawns a Chrome/Chromium child process, parses the
//! `DevTools listening on ws://…` line from stderr to discover the WebSocket
//! URL (or, with `use_pipe`, negotiates the OS-pipe transport on fds 3/4),
//! opens the CDP connection, and spawns the `Handler`
//! task. It returns the `Browser`, which owns the connection handle internally;
//! call [`Browser::handle`] if you need raw access to the dispatcher.
//!
//! [`Browser::connect`] attaches to an already-running browser via a known
//! debug URL instead of launching a new process.
//!
//! The child process is killed on [`Browser::kill`] or drop. This module
//! also owns the `unsafe` pipe-fd setup used by the pipe transport on Unix.

pub(crate) mod argument;
pub(crate) mod config;
mod launch;

pub use config::{BrowserConfig, BrowserConfigBuilder};

use std::process::Child;

use chromist_types::CdpJsonEventMessage;

use crate::conn::{AnyConnection, Connection};
use crate::error::CdpError;
use crate::handler::{Handler, HandlerHandle};
use crate::storage::StorageState;

/// Owner of the browser child process and its CDP connection.
///
/// Unlike the other handle types in this crate (`Page`, `Frame`, `Element`,
/// `Locator`), `Browser` is **not `Clone`** — it owns the subprocess and the
/// temporary user-data directory, both of which are released on drop. Use
/// [`Browser::handle`] to get a cheaply cloneable [`HandlerHandle`](crate::HandlerHandle) when you
/// need to share access to the underlying CDP connection.
#[derive(Debug)]
pub struct Browser {
    child: Option<Child>,
    handle: HandlerHandle,
    ws_url: String,
    config: Option<BrowserConfig>,
    incognito: bool,
    /// Temporary user-data directory created when no `user_data_dir` is
    /// configured.  Stored here so it is cleaned up when the `Browser` is
    /// dropped / killed rather than leaked.
    tempdir: Option<std::path::PathBuf>,
}

impl Browser {
    /// Launch a browser with [`BrowserConfig::default`].
    ///
    /// Shorthand for `Browser::launch(BrowserConfig::default()).await`. Use
    /// [`Browser::launch`] when you need a customised configuration.
    pub async fn launch_default() -> crate::Result<Self> {
        Self::launch(BrowserConfig::default()).await
    }

    /// Launch a Chrome / Chromium child process with `config` and connect to it.
    ///
    /// Steps:
    /// 1. Reject pipe transport on non-Unix platforms.
    /// 2. Resolve the executable path (explicit `executable` or
    ///    auto-detect).
    /// 3. Resolve the user-data directory (explicit or fresh tempdir owned by
    ///    the returned `Browser`).
    /// 4. Build the Chromium CLI command via `launch::build_command`.
    /// 5. Spawn via either `launch::spawn_pipe` or
    ///    `launch::spawn_websocket` depending on `config.use_pipe`.
    /// 6. Spawn the `Handler` task; auto-discover targets and (WebSocket
    ///    only) enable flattened auto-attach so cross-process navigations
    ///    refresh the session cell.
    /// 7. Apply [`StorageState`] if configured.
    pub async fn launch(config: BrowserConfig) -> crate::Result<Self> {
        if config.use_pipe {
            #[cfg(not(unix))]
            return Err(CdpError::UnsupportedTransport("pipe transport is only supported on Unix"));
        }

        let exe = config.resolve_executable()?;
        let (user_data_dir, temp_user_data) = launch::resolve_user_data_dir(&config)?;
        let cmd = launch::build_command(&config, &exe, &user_data_dir);

        let (child, conn, ws_url) = if config.use_pipe {
            #[cfg(unix)]
            {
                let (child, conn) = launch::spawn_pipe(cmd)?;
                (child, conn, String::new())
            }
            #[cfg(not(unix))]
            unreachable!("pipe transport rejected above on non-Unix");
        } else {
            launch::spawn_websocket(cmd, config.launch_timeout).await?
        };

        let handle = Handler::spawn(conn);
        let _ = handle
            .execute(
                crate::cdp::browser_protocol::target::SetDiscoverTargetsParams::new(true),
                None,
            )
            .await;

        // WebSocket transport only: enable flattened auto-attach so
        // renderer-process swaps refresh the handler's session cell. Pipe
        // mode wires this elsewhere; replicating it would double-attach.
        if !config.use_pipe {
            let mut auto_attach =
                crate::cdp::browser_protocol::target::SetAutoAttachParams::new(true, false);
            auto_attach.flatten = Some(true);
            let _ = handle.execute(auto_attach, None).await;
            tracing::info!(ws_url = %ws_url, "browser launched successfully");
        }

        let incognito = config.incognito;
        let storage_state = config.storage_state.clone();
        let browser = Browser {
            child: Some(child),
            handle,
            ws_url,
            config: Some(config),
            incognito,
            tempdir: temp_user_data,
        };
        if let Some(state) = storage_state {
            browser.apply_storage_state(state).await;
        }
        Ok(browser)
    }

    /// Attach to an already-running browser at the given WebSocket DevTools URL.
    ///
    /// Unlike [`Browser::launch`], no child process is spawned and dropping
    /// the returned [`Browser`] does **not** terminate the browser — it only
    /// shuts down chromist's connection.
    ///
    /// # Errors
    ///
    /// Returns [`CdpError::WebSocket`] if the WebSocket handshake fails.
    pub async fn connect(ws_url: impl Into<String>) -> crate::Result<Self> {
        let ws_url = ws_url.into();
        let conn: Connection<CdpJsonEventMessage> = Connection::connect(&ws_url).await?;
        let handle = Handler::spawn(AnyConnection::Ws(conn));
        let _ = handle
            .execute(
                crate::cdp::browser_protocol::target::SetDiscoverTargetsParams::new(true),
                None,
            )
            .await;
        // Auto-attach (with flatten) so renderer-process swaps for an already
        // attached page produce a fresh `Target.attachedToTarget` and the
        // handler's session cell is updated; without this every cross-process
        // navigation would leave page commands targeting a dead session id.
        let mut auto_attach =
            crate::cdp::browser_protocol::target::SetAutoAttachParams::new(true, false);
        auto_attach.flatten = Some(true);
        let _ = handle.execute(auto_attach, None).await;
        let browser = Browser {
            child: None,
            handle: handle.clone(),
            ws_url,
            config: None,
            incognito: false,
            tempdir: None,
        };
        Ok(browser)
    }

    /// Connect to a browser via its HTTP DevTools endpoint.
    /// Fetches the WebSocket URL from `http://{addr}/json/version`.
    pub async fn connect_to_http(addr: impl Into<String>) -> crate::Result<Self> {
        let addr = addr.into();
        let url = if addr.starts_with("http://") || addr.starts_with("https://") {
            format!("{}/json/version", addr.trim_end_matches('/'))
        } else {
            format!("http://{}/json/version", addr)
        };
        let ws_url = fetch_ws_url_from_http(&url).await?;
        Self::connect(ws_url).await
    }

    /// Connect to a browser via its CDP DevTools endpoint URL.
    ///
    /// Alias for [`connect_to_http`].
    ///
    /// [`connect_to_http`]: Browser::connect_to_http
    pub async fn connect_over_cdp(endpoint_url: impl Into<String>) -> crate::Result<Self> {
        Self::connect_to_http(endpoint_url).await
    }

    /// Launch a browser with a persistent user-data directory and return the
    /// default [`BrowserContext`](crate::BrowserContext).
    ///
    /// The caller **must** keep the returned [`Browser`] alive for the duration of the
    /// session; dropping it kills the child process.
    pub async fn launch_persistent_context(
        user_data_dir: impl Into<std::path::PathBuf>,
        mut config: BrowserConfig,
    ) -> crate::Result<(Self, crate::handler::BrowserContext)> {
        config.user_data_dir = Some(user_data_dir.into());
        let browser = Self::launch(config).await?;
        let ctx = browser.handle.default_browser_context();
        Ok((browser, ctx))
    }

    /// Returns the WebSocket DevTools URL the connection is using.
    ///
    /// Empty string when running in pipe-transport mode.
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    /// Returns the underlying [`HandlerHandle`](crate::HandlerHandle) for
    /// raw CDP access.
    ///
    /// Most callers should not need this — the typed methods on
    /// [`Browser`], [`Page`](crate::Page), [`Frame`](crate::Frame),
    /// [`Element`](crate::Element), and the sub-handles cover the supported
    /// surface. Use the handle as an escape hatch for issuing CDP commands
    /// chromist does not yet wrap.
    pub fn handle(&self) -> &HandlerHandle {
        &self.handle
    }

    /// Returns a mutable reference to the underlying child process, when one
    /// exists.
    ///
    /// `None` for browsers attached via [`Browser::connect`] or
    /// [`Browser::connect_to_http`] — those connections do not own a child.
    pub fn child_mut(&mut self) -> Option<&mut std::process::Child> {
        self.child.as_mut()
    }

    /// Block until the browser child process exits.
    ///
    /// Uses `tokio::task::block_in_place` so that the current tokio thread can
    /// yield back to the executor while the synchronous `Child::wait` call
    /// blocks, preventing the runtime from stalling.
    pub async fn wait(&mut self) -> crate::Result<std::process::ExitStatus> {
        let child = self.child.as_mut().ok_or(CdpError::NoChildProcess)?;
        tokio::task::block_in_place(|| child.wait()).map_err(CdpError::Io)
    }

    /// Non-blocking check whether the browser child process has exited.
    pub fn try_wait(&mut self) -> crate::Result<Option<std::process::ExitStatus>> {
        let child = self.child.as_mut().ok_or(CdpError::NoChildProcess)?;
        child.try_wait().map_err(CdpError::Io)
    }

    /// Gracefully close the browser via CDP.
    pub async fn close(&self) -> crate::Result<()> {
        let _ = self
            .handle
            .execute(crate::cdp::browser_protocol::browser::CloseParams::default(), None)
            .await;
        self.handle.shutdown();
        Ok(())
    }

    /// Get the version info of the browser.
    pub async fn version(
        &self,
    ) -> crate::Result<crate::cdp::browser_protocol::browser::GetVersionResponse> {
        self.handle
            .execute(crate::cdp::browser_protocol::browser::GetVersionParams::default(), None)
            .await
    }

    /// Get the user agent string.
    pub async fn user_agent(&self) -> crate::Result<String> {
        let resp = self.version().await?;
        Ok(resp.user_agent)
    }

    /// Enumerate all open targets (pages, service workers, etc.).
    ///
    /// Reads from the handler's internal target cache — no CDP round-trip required.
    pub fn targets(&self) -> crate::Result<Vec<crate::cdp::browser_protocol::target::TargetInfo>> {
        self.handle.targets()
    }

    /// Attach to a page by its CDP target ID.
    pub async fn page_by_target(
        &self,
        target_id: crate::cdp::browser_protocol::target::TargetId,
    ) -> crate::Result<crate::page::Page> {
        crate::page::Page::attach(self.handle.clone(), target_id).await
    }

    /// Attach to all existing page targets and return them.
    pub async fn pages(&self) -> crate::Result<Vec<crate::page::Page>> {
        let targets = self.targets()?;
        let mut pages = Vec::new();
        for info in targets {
            if info.r#type == "page" {
                if let Ok(p) = crate::page::Page::attach(self.handle.clone(), info.target_id).await
                {
                    pages.push(p);
                }
            }
        }
        Ok(pages)
    }

    /// Open a new tab and navigate it to `url`.
    ///
    /// Returns a [`Page`](crate::Page) ready for interaction. Equivalent to
    /// `Target.createTarget` + `Page.attach`.
    pub async fn new_page(
        &self,
        params: impl Into<crate::cdp::browser_protocol::target::CreateTargetParams>,
    ) -> crate::Result<crate::page::Page> {
        let resp = self.handle.execute(params.into(), None).await?;
        crate::page::Page::attach(self.handle.clone(), resp.target_id).await
    }

    /// Create a new CDP browser context and return its raw ID.
    ///
    /// Low-level escape hatch for callers that need to issue raw CDP
    /// `Target.*` commands against a context ID. For the high-level
    /// [`BrowserContext`](crate::BrowserContext) handle, use
    /// [`new_context`](Self::new_context) or [`new_context_with_options`](Self::new_context_with_options).
    ///
    /// Pass `CreateBrowserContextParams::default()` for a plain incognito context, or
    /// use the builder to set a proxy, bypass list, or `dispose_on_detach`.
    pub async fn create_browser_context(
        &self,
        params: impl Into<crate::cdp::browser_protocol::target::CreateBrowserContextParams>,
    ) -> crate::Result<crate::cdp::browser_protocol::browser::BrowserContextId> {
        use crate::cdp::browser_protocol::target as cdp_target;
        let resp = self
            .handle
            .execute(params.into() as cdp_target::CreateBrowserContextParams, None)
            .await?;
        Ok(resp.browser_context_id)
    }

    /// Dispose an incognito browser context by raw ID.
    ///
    /// Low-level counterpart to [`create_browser_context`](Self::create_browser_context).
    /// Prefer [`BrowserContext::close`](crate::BrowserContext::close) when
    /// you hold a high-level handle — it also closes the context's pages
    /// before disposal.
    pub async fn dispose_browser_context(
        &self,
        id: crate::cdp::browser_protocol::browser::BrowserContextId,
    ) -> crate::Result<()> {
        use crate::cdp::browser_protocol::target as cdp_target;
        self.handle.execute(cdp_target::DisposeBrowserContextParams::new(id), None).await?;
        Ok(())
    }

    /// Create a new isolated [`BrowserContext`](crate::BrowserContext).
    ///
    /// The returned context is a first-class isolation unit: it owns its
    /// own cookies, `localStorage`, init scripts, exposed functions and
    /// bindings, route registry, and permissions. Pages opened through
    /// [`BrowserContext::new_page`](crate::BrowserContext::new_page)
    /// inherit the context's state automatically.
    ///
    /// Tear down the context via [`BrowserContext::close`](crate::BrowserContext::close).
    pub async fn new_context(&self) -> crate::Result<crate::handler::BrowserContext> {
        use crate::cdp::browser_protocol::target as cdp_target;
        let resp =
            self.handle.execute(cdp_target::CreateBrowserContextParams::default(), None).await?;
        Ok(crate::handler::BrowserContext::new_with_id(
            self.handle.clone(),
            Some(resp.browser_context_id),
        ))
    }

    /// Create a new isolated [`BrowserContext`](crate::BrowserContext) with
    /// explicit [`BrowserContextOptions`](crate::BrowserContextOptions).
    ///
    /// Proxy, CSP bypass, HTTP credentials, JavaScript toggle, service-worker
    /// policy, and downloads directory are all configurable through the options.
    /// Tear down the context via [`BrowserContext::close`](crate::BrowserContext::close).
    pub async fn new_context_with_options(
        &self,
        opts: crate::context_options::BrowserContextOptions,
    ) -> crate::Result<crate::handler::BrowserContext> {
        use crate::cdp::browser_protocol::target as cdp_target;
        let mut params = cdp_target::CreateBrowserContextParams::default();
        if let Some(ref server) = opts.proxy_server {
            params.proxy_server = Some(server.clone());
        }
        if let Some(ref bypass) = opts.proxy_bypass_list {
            params.proxy_bypass_list = Some(bypass.clone());
        }
        let resp = self.handle.execute(params, None).await?;
        let ctx = crate::handler::BrowserContext::new_with_options(
            self.handle.clone(),
            Some(resp.browser_context_id),
            opts,
        );
        // Apply download path at context level immediately if set.
        let downloads_path = ctx.downloads_path.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        if let Some(path) = downloads_path {
            use crate::cdp::browser_protocol::browser as cdp_browser;
            let mut dl = cdp_browser::SetDownloadBehaviorParams::new(
                cdp_browser::SetDownloadBehaviorParamsBehavior::Allow,
            );
            dl.browser_context_id = ctx.id.clone();
            dl.download_path = path.to_str().map(String::from);
            dl.events_enabled = Some(true);
            let _ = self.handle.execute(dl, None).await;
        }
        Ok(ctx)
    }

    /// Return all browser contexts: the default context followed by all
    /// non-default (incognito) contexts currently known to the browser.
    pub async fn contexts(&self) -> crate::Result<Vec<crate::handler::BrowserContext>> {
        self.handle.browser_contexts().await
    }

    /// Create a new page in a specific browser context (incognito session).
    pub async fn new_page_in_context(
        &self,
        url: impl Into<String>,
        context_id: crate::cdp::browser_protocol::browser::BrowserContextId,
    ) -> crate::Result<crate::page::Page> {
        use crate::cdp::browser_protocol::target as cdp_target;
        let mut params = cdp_target::CreateTargetParams::new(url.into());
        params.browser_context_id = Some(context_id);
        let resp = self.handle.execute(params, None).await?;
        crate::page::Page::attach(self.handle.clone(), resp.target_id).await
    }

    /// Execute any CDP command at browser level (no session).
    pub async fn execute<C>(&self, cmd: C) -> crate::Result<C::Response>
    where
        C: chromist_types::Command + serde::Serialize + Send + 'static,
        C::Response: serde::de::DeserializeOwned + Send + 'static,
    {
        self.handle.execute(cmd, None).await
    }

    /// Clear all cookies via Storage.clearCookies.
    pub async fn clear_cookies(&self) -> crate::Result<()> {
        self.handle
            .execute(crate::cdp::browser_protocol::storage::ClearCookiesParams::default(), None)
            .await?;
        Ok(())
    }

    /// Get all cookies at browser level via network API.
    pub async fn cookies(
        &self,
    ) -> crate::Result<Vec<crate::cdp::browser_protocol::network::Cookie>> {
        let resp = self
            .handle
            .execute(crate::cdp::browser_protocol::network::GetCookiesParams::default(), None)
            .await?;
        Ok(resp.cookies)
    }

    /// A typed event stream at the browser level (no session filter).
    pub fn event_listener<T>(&self) -> crate::listeners::EventStream<T>
    where
        T: chromist_types::EventMessage + chromist_types::MethodType + Send + 'static,
    {
        self.handle.event_listener(None)
    }

    /// Stream of `Target.targetCreated` events (new tabs, workers, etc.).
    /// `SetDiscoverTargets(true)` is called during `launch`/`connect` so
    /// these events begin firing immediately.
    pub fn new_target_stream(
        &self,
    ) -> crate::listeners::EventStream<crate::cdp::browser_protocol::target::TargetCreatedEvent>
    {
        self.handle.event_listener(None)
    }

    /// Stream of `Target.targetDestroyed` events.
    pub fn target_destroyed_stream(
        &self,
    ) -> crate::listeners::EventStream<crate::cdp::browser_protocol::target::TargetDestroyedEvent>
    {
        self.handle.event_listener(None)
    }

    /// Stream of `Target.targetInfoChanged` events (URL, title changes, etc.).
    pub fn target_changed_stream(
        &self,
    ) -> crate::listeners::EventStream<crate::cdp::browser_protocol::target::TargetInfoChangedEvent>
    {
        self.handle.event_listener(None)
    }

    /// Returns the [`BrowserConfig`] this browser was launched with, when it
    /// was launched (not connected to). `None` for [`Browser::connect`] etc.
    pub fn config(&self) -> Option<&BrowserConfig> {
        self.config.as_ref()
    }

    /// Returns `true` if this browser was launched with `--incognito`.
    pub fn is_incognito(&self) -> bool {
        self.incognito
    }

    /// Returns `true` while the browser process is still alive.
    ///
    /// For a launched process this polls the child exit status; for a
    /// `connect`-ed browser (no child process) it always returns `true`.
    pub fn is_connected(&self) -> bool {
        match &self.child {
            None => true,
            Some(_) => {
                // We cannot call try_wait on &self (needs &mut), so we
                // conservatively return true. Use `try_wait` for a definitive check.
                true
            }
        }
    }

    /// Register a callback that fires once when the browser disconnects.
    ///
    /// Spawns a background task that waits for the handler's event stream to
    /// close (which happens when the CDP connection is lost or the browser exits)
    /// then calls `f`.
    pub fn on_disconnected(&self, f: impl Fn() + Send + 'static) {
        use futures::StreamExt;
        let mut stream = self.handle.subscribe(None);
        crate::runtime::spawn(async move {
            while stream.next().await.is_some() {}
            f();
        });
    }

    /// Connect to a running browser with explicit [`HandlerConfig`](crate::HandlerConfig).
    ///
    /// Use this when you need to override request timeouts, viewport, or
    /// pre-register browser-context IDs that the running browser already
    /// has.
    pub async fn connect_with_config(
        ws_url: impl Into<String>,
        config: crate::handler::HandlerConfig,
    ) -> crate::Result<Self> {
        let ws_url = ws_url.into();
        let conn: Connection<CdpJsonEventMessage> = Connection::connect(&ws_url).await?;
        let (handler, handle) = Handler::new(AnyConnection::Ws(conn));
        let handler = handler.with_timeout(config.request_timeout);
        crate::runtime::spawn(handler.run());
        let _ = handle
            .execute(
                crate::cdp::browser_protocol::target::SetDiscoverTargetsParams::new(true),
                None,
            )
            .await;
        // Auto-attach (with flatten) so renderer-process swaps for an already
        // attached page produce a fresh `Target.attachedToTarget` and the
        // handler's session cell is updated; without this every cross-process
        // navigation would leave page commands targeting a dead session id.
        let mut auto_attach =
            crate::cdp::browser_protocol::target::SetAutoAttachParams::new(true, false);
        auto_attach.flatten = Some(true);
        let _ = handle.execute(auto_attach, None).await;
        let browser = Browser {
            child: None,
            handle: handle.clone(),
            ws_url,
            config: None,
            incognito: false,
            tempdir: None,
        };
        Ok(browser)
    }

    /// Set cookies.
    pub async fn set_cookies(
        &self,
        cookies: Vec<crate::cdp::browser_protocol::network::CookieParam>,
    ) -> crate::Result<()> {
        self.handle
            .execute(crate::cdp::browser_protocol::network::SetCookiesParams::new(cookies), None)
            .await?;
        Ok(())
    }

    /// Restore cookies from a [`StorageState`] snapshot.
    ///
    /// Called internally after the browser connects when
    /// [`BrowserConfigBuilder::storage_state`] was set.  Errors are silently
    /// swallowed so a bad snapshot never prevents the browser from starting.
    async fn apply_storage_state(&self, state: StorageState) {
        if state.cookies.is_empty() {
            return;
        }
        let params: Vec<crate::cdp::browser_protocol::network::CookieParam> =
            state.cookies.into_iter().map(Into::into).collect();
        let _ = self
            .handle
            .execute(crate::cdp::browser_protocol::network::SetCookiesParams::new(params), None)
            .await;
    }

    /// Terminate the browser child process and clean up resources.
    ///
    /// Sends a `shutdown` to the handler task, kills the child process, and
    /// removes the temporary user-data directory if one was created. This
    /// also runs on `Browser::drop` automatically — call `kill` explicitly
    /// only when you want to handle the result or terminate before the
    /// `Browser` value goes out of scope.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok(())` — child-process and tempdir cleanup
    /// failures are logged but do not propagate.
    pub fn kill(&mut self) -> crate::Result<()> {
        self.handle.shutdown();
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        // Best-effort cleanup of the temporary user-data directory.  We ignore
        // errors because the directory may have already been removed by the OS
        // or the process may have done it itself on exit.
        if let Some(path) = self.tempdir.take() {
            let _ = std::fs::remove_dir_all(path);
        }
        Ok(())
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn tempdir() -> crate::Result<std::path::PathBuf> {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = base.join(format!("chromist-{pid}-{nanos}"));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

async fn fetch_ws_url_from_http(url: &str) -> crate::Result<String> {
    let conn = fetch_browser_connection(url).await?;
    Ok(conn.web_socket_debugger_url)
}

async fn fetch_browser_connection(url: &str) -> crate::Result<crate::network::BrowserConnection> {
    let url_str = url.to_string();
    let without_scheme = url_str.trim_start_matches("http://").trim_start_matches("https://");
    let slash_pos = without_scheme.find('/').unwrap_or(without_scheme.len());
    let host_port = &without_scheme[..slash_pos];
    let path = &without_scheme[slash_pos..];

    use futures::{AsyncReadExt, AsyncWriteExt};
    let mk_discovery_err =
        |reason: String| CdpError::HttpDiscovery { host: host_port.to_string(), reason };

    let stream = tokio::net::TcpStream::connect(host_port)
        .await
        .map_err(|e| mk_discovery_err(format!("connect: {e}")))?;
    let mut stream = async_tungstenite::tokio::TokioAdapter::new(stream);

    let request =
        format!("GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n", path, host_port);
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| mk_discovery_err(format!("write HTTP request: {e}")))?;

    let mut body = Vec::new();
    stream
        .read_to_end(&mut body)
        .await
        .map_err(|e| mk_discovery_err(format!("read HTTP response: {e}")))?;

    let text = String::from_utf8_lossy(&body);
    let json_start = text
        .find('{')
        .ok_or_else(|| mk_discovery_err("no JSON in /json/version response".into()))?;
    let json_str = &text[json_start..];
    serde_json::from_str(json_str)
        .map_err(|e| mk_discovery_err(format!("parse /json/version JSON: {e}")))
}

/// Build a low-level browser connection without spawning the
/// [`Handler`](crate::HandlerHandle) task.
///
/// Most callers should prefer [`Browser::launch`](crate::Browser::launch)
/// or [`Browser::connect`](crate::Browser::connect) — this helper is the
/// raw plumbing exposed for embedded / advanced use cases.
pub async fn browser_connection(
    addr: impl Into<String>,
) -> crate::Result<crate::network::BrowserConnection> {
    let addr = addr.into();
    let url = if addr.starts_with("http://") || addr.starts_with("https://") {
        format!("{}/json/version", addr.trim_end_matches('/'))
    } else {
        format!("http://{}/json/version", addr)
    };
    fetch_browser_connection(&url).await
}
