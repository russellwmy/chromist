//! Per-context state and CDP commands scoped to a single browser context.
//!
//! A [`BrowserContext`](crate::BrowserContext) is a first-class isolation
//! unit: an incognito or default browsing session that owns its own cookies,
//! `localStorage`, init scripts, exposed functions and bindings, route
//! registry, WebSocket interception routes, geolocation, HTTP credentials,
//! permissions, service-worker policy, and download path. State set on one
//! context never leaks to another, and pages created via
//! [`BrowserContext::new_page`](crate::BrowserContext::new_page) inherit the
//! context's state automatically.
//!
//! Lifecycle: non-default contexts are torn down via
//! [`BrowserContext::close`](crate::BrowserContext::close), which closes all
//! pages opened through the context and disposes the underlying CDP browser
//! context. The default context's lifetime is tied to the browser process.
//!
//! Construction routes through [`Browser::new_context`](crate::Browser::new_context),
//! [`Browser::new_context_with_options`](crate::Browser::new_context_with_options),
//! or [`HandlerHandle::default_browser_context`](crate::HandlerHandle::default_browser_context).
//! CDP commands are issued via the embedded [`HandlerHandle`](crate::HandlerHandle).
//!
//! Split out from `handler/mod.rs` to keep that module focused on the I/O
//! task and its handle. Page-creating, popup-waiting, and event-waiting
//! methods live in [`crate::context`] to avoid a circular import on
//! [`Page`](crate::Page).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::HandlerHandle;
use crate::error::CdpError;

/// Type-erased handler for [`BrowserContext::expose_function`].
pub(crate) type ContextExposeHandler = Arc<
    dyn Fn(
            Vec<serde_json::Value>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>>
        + Send
        + Sync,
>;

/// Type-erased handler for [`BrowserContext::expose_binding`].
pub(crate) type ContextBindingHandler = Arc<
    dyn Fn(
            serde_json::Value,
            Vec<serde_json::Value>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = serde_json::Value> + Send>>
        + Send
        + Sync,
>;

/// A first-class browser-context isolation unit.
///
/// Owns per-context state (init scripts, exposed functions/bindings, route
/// registry, WebSocket routes, geolocation, HTTP credentials, permissions,
/// CSP-bypass, JavaScript-enabled, service-worker policy, downloads path,
/// offline mode, extra HTTP headers) and applies it to every page created
/// through [`new_page`](crate::BrowserContext::new_page). Each `BrowserContext`
/// maps to one CDP browser context — `id == None` represents the default
/// (non-incognito) context.
///
/// **Construction.** Use [`HandlerHandle::default_browser_context`] for the
/// default context, [`Browser::new_context`](crate::Browser::new_context) for
/// a plain incognito context, or [`Browser::new_context_with_options`](crate::Browser::new_context_with_options)
/// to apply a [`BrowserContextOptions`](crate::BrowserContextOptions).
///
/// **Lifecycle.** [`close`](crate::BrowserContext::close) disposes a
/// non-default context and all of its pages. The default context cannot be
/// closed; closing the [`Browser`](crate::Browser) tears it down implicitly.
///
/// **Events.** [`wait_for_event`](crate::BrowserContext::wait_for_event),
/// [`wait_for_page`](crate::BrowserContext::wait_for_page),
/// [`wait_for_service_worker`](crate::BrowserContext::wait_for_service_worker),
/// and [`wait_for_background_page`](crate::BrowserContext::wait_for_background_page)
/// filter to events whose target belongs to this context.
#[derive(Clone)]
pub struct BrowserContext {
    /// Context identifier — `None` for the default (non-incognito) context.
    pub id: Option<crate::cdp::browser_protocol::browser::BrowserContextId>,
    /// Context-wide route registry.  Applied to pages created from this context
    /// after page-level rules are checked.
    pub registry: crate::route::RouteRegistry,
    /// Back-reference to the handler used for all CDP commands issued by the
    /// context-layer methods.  Not compared in `PartialEq`/`Eq`.
    pub(crate) handle: HandlerHandle,
    /// Scripts to inject via `Page.addScriptToEvaluateOnNewDocument` into every
    /// page opened through this context.  Shared across clones.
    pub(crate) init_scripts: Arc<Mutex<Vec<String>>>,
    /// WebSocket interception routes.  Applied to each page when it is created
    /// via [`BrowserContext::new_page`].  Shared across clones via `Arc`.
    pub(crate) ws_routes: Arc<Mutex<Vec<(String, crate::page::websocket_route::WsRouteHandler)>>>,
    /// Whether this context is in offline mode.  Applied to new pages on creation.
    pub(crate) offline: Arc<AtomicBool>,
    /// Geolocation override (latitude, longitude, accuracy).  Applied to new pages on creation.
    pub(crate) geolocation: Arc<Mutex<Option<(f64, f64, f64)>>>,
    /// Context-level `expose_function` handlers.  Applied to each page opened via
    /// [`BrowserContext::new_page`].
    pub(crate) expose_fns: Arc<Mutex<Vec<(String, ContextExposeHandler)>>>,
    /// Context-level `expose_binding` handlers.  Applied to each page opened via
    /// [`BrowserContext::new_page`].
    pub(crate) expose_bindings_store: Arc<Mutex<Vec<(String, ContextBindingHandler)>>>,
    /// Whether to bypass CSP on every page opened in this context.
    pub(crate) bypass_csp: Arc<AtomicBool>,
    /// HTTP Basic Auth credentials applied to every page via `Authorization`.
    pub(crate) http_credentials: Arc<Mutex<Option<crate::context_options::HttpCredentials>>>,
    /// Whether JavaScript is enabled (default `true`).
    pub(crate) javascript_enabled: Arc<AtomicBool>,
    /// Service worker policy for this context.
    pub(crate) service_workers: Arc<Mutex<crate::context_options::ServiceWorkersPolicy>>,
    /// Directory where downloaded files are saved, if set.
    pub(crate) downloads_path: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl std::fmt::Debug for BrowserContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserContext").field("id", &self.id).finish_non_exhaustive()
    }
}

impl PartialEq for BrowserContext {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for BrowserContext {}

impl BrowserContext {
    /// Construct a plain context with default options.
    pub(crate) fn new_with_id(
        handle: HandlerHandle,
        id: Option<crate::cdp::browser_protocol::browser::BrowserContextId>,
    ) -> Self {
        Self {
            id,
            registry: crate::route::RouteRegistry::new(),
            handle,
            init_scripts: Default::default(),
            ws_routes: Default::default(),
            offline: Arc::new(AtomicBool::new(false)),
            geolocation: Arc::new(Mutex::new(None)),
            expose_fns: Default::default(),
            expose_bindings_store: Default::default(),
            bypass_csp: Arc::new(AtomicBool::new(false)),
            http_credentials: Arc::new(Mutex::new(None)),
            javascript_enabled: Arc::new(AtomicBool::new(true)),
            service_workers: Arc::new(Mutex::new(
                crate::context_options::ServiceWorkersPolicy::Allow,
            )),
            downloads_path: Arc::new(Mutex::new(None)),
        }
    }

    /// Construct a context and apply options from [`BrowserContextOptions`](crate::BrowserContextOptions).
    pub(crate) fn new_with_options(
        handle: HandlerHandle,
        id: Option<crate::cdp::browser_protocol::browser::BrowserContextId>,
        opts: crate::context_options::BrowserContextOptions,
    ) -> Self {
        Self {
            id,
            registry: crate::route::RouteRegistry::new(),
            handle,
            init_scripts: Default::default(),
            ws_routes: Default::default(),
            offline: Arc::new(AtomicBool::new(false)),
            geolocation: Arc::new(Mutex::new(None)),
            expose_fns: Default::default(),
            expose_bindings_store: Default::default(),
            bypass_csp: Arc::new(AtomicBool::new(opts.bypass_csp)),
            http_credentials: Arc::new(Mutex::new(opts.http_credentials)),
            javascript_enabled: Arc::new(AtomicBool::new(opts.javascript_enabled)),
            service_workers: Arc::new(Mutex::new(opts.service_workers)),
            downloads_path: Arc::new(Mutex::new(opts.downloads_path)),
        }
    }

    /// Returns `true` if this is the default (non-incognito) browser context.
    pub fn is_default(&self) -> bool {
        self.id.is_none()
    }

    /// Register a script to be injected into every page opened via this context.
    ///
    /// The script is injected via `Page.addScriptToEvaluateOnNewDocument` when
    /// [`BrowserContext::new_page`](crate::BrowserContext::new_page) creates
    /// the page.  Callers can register multiple scripts; they are injected in
    /// registration order.
    pub fn add_init_script(&self, script: impl Into<String>) {
        match self.init_scripts.lock() {
            Ok(mut scripts) => scripts.push(script.into()),
            Err(_) => tracing::warn!(
                "BrowserContext init_scripts lock poisoned; init script not registered"
            ),
        }
    }

    /// Returns `true` when `info_ctx` belongs to this context.
    fn matches_context(
        &self,
        info_ctx: &Option<crate::cdp::browser_protocol::browser::BrowserContextId>,
    ) -> bool {
        match (&self.id, info_ctx) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// Collect `TargetId`s of all page targets that belong to this context.
    pub(crate) fn page_target_ids(
        &self,
    ) -> crate::Result<Vec<crate::cdp::browser_protocol::target::TargetId>> {
        let tree = self.handle.tree.read().map_err(|_| CdpError::LockPoisoned)?;
        Ok(tree
            .all()
            .filter(|e| e.info.r#type == "page" && self.matches_context(&e.info.browser_context_id))
            .map(|e| e.info.target_id.clone())
            .collect())
    }

    // ── Service workers and background pages ─────────────────────────────

    /// List all live service workers in this browser context.
    ///
    /// Reads the handler's target tree synchronously and constructs a
    /// [`Worker`](crate::events::Worker) for each entry whose type is
    /// `"service_worker"` and which has an active CDP session.
    ///
    /// Workers without a session (e.g. not yet attached) are silently skipped.
    pub fn service_workers(&self) -> crate::Result<Vec<crate::events::Worker>> {
        let tree = self.handle.tree.read().map_err(|_| CdpError::LockPoisoned)?;
        Ok(tree
            .all()
            .filter(|e| {
                e.info.r#type == "service_worker"
                    && self.matches_context(&e.info.browser_context_id)
            })
            .filter_map(|e| {
                let session = self.handle.session_cell(&e.info.target_id)?;
                Some(crate::events::Worker {
                    url: e.info.url.clone(),
                    handle: self.handle.clone(),
                    session_id: session.current(),
                    listener_tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
                })
            })
            .collect())
    }

    /// Collect `TargetId`s of all background page targets that belong to this context.
    ///
    /// Background pages are Chrome extension background scripts.  The IDs are
    /// used by the async [`background_pages`](crate::context::BrowserContext::background_pages)
    /// method in `context.rs` to attach [`Page`](crate::page::Page) objects.
    pub(crate) fn background_page_target_ids(
        &self,
    ) -> crate::Result<Vec<crate::cdp::browser_protocol::target::TargetId>> {
        let tree = self.handle.tree.read().map_err(|_| CdpError::LockPoisoned)?;
        Ok(tree
            .all()
            .filter(|e| {
                e.info.r#type == "background_page"
                    && self.matches_context(&e.info.browser_context_id)
            })
            .map(|e| e.info.target_id.clone())
            .collect())
    }

    // ── Storage state ─────────────────────────────────────────────────────

    /// Snapshot all cookies and `localStorage` for every page in this context.
    ///
    /// Cookies are fetched browser-wide via `Network.getCookies` (no session
    /// needed).  `localStorage` is read by evaluating JS in each page's main
    /// frame.
    pub async fn storage_state(&self) -> crate::Result<crate::storage::StorageState> {
        use crate::cdp::browser_protocol::network as cdp_network;
        use crate::cdp::js_protocol::runtime as cdp_runtime;

        let resp = self.handle.execute(cdp_network::GetCookiesParams::default(), None).await?;
        let cookies: Vec<crate::storage::Cookie> =
            resp.cookies.into_iter().map(crate::storage::Cookie::from).collect();

        let target_ids = self.page_target_ids()?;
        let mut origins = Vec::new();
        for target_id in target_ids {
            let session_ref = match self.handle.session_cell(&target_id) {
                Some(s) => s,
                None => continue,
            };
            let session = session_ref.current();

            let url_result = self
                .handle
                .execute(
                    cdp_runtime::EvaluateParams::new("location.origin".to_string()),
                    Some(session.clone()),
                )
                .await;
            let origin = url_result
                .ok()
                .and_then(|r| r.result.value)
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if origin.is_empty() || origin == "null" {
                continue;
            }

            let ls_result = self
                .handle
                .execute(
                    cdp_runtime::EvaluateParams::new(
                        "JSON.stringify(Object.fromEntries(Object.entries(localStorage)))"
                            .to_string(),
                    ),
                    Some(session),
                )
                .await;
            if let Ok(resp) = ls_result {
                if let Some(json_str) =
                    resp.result.value.and_then(|v| v.as_str().map(str::to_string))
                {
                    if let Ok(map) =
                        serde_json::from_str::<std::collections::HashMap<String, String>>(&json_str)
                    {
                        origins.push(crate::storage::OriginStorage {
                            origin,
                            local_storage: map.into_iter().collect(),
                        });
                    }
                }
            }
        }

        Ok(crate::storage::StorageState { cookies, origins })
    }

    // ── Cookie management ─────────────────────────────────────────────────

    /// Add cookies to this context via `Network.setCookies`.
    pub async fn add_cookies(&self, cookies: Vec<crate::storage::Cookie>) -> crate::Result<()> {
        use crate::cdp::browser_protocol::network as cdp_network;
        let params: Vec<cdp_network::CookieParam> =
            cookies.into_iter().map(cdp_network::CookieParam::from).collect();
        self.handle.execute(cdp_network::SetCookiesParams::new(params), None).await?;
        Ok(())
    }

    /// Clear all cookies in this context via `Network.clearBrowserCookies`.
    pub async fn clear_cookies(&self) -> crate::Result<()> {
        use crate::cdp::browser_protocol::network as cdp_network;
        self.handle.execute(cdp_network::ClearBrowserCookiesParams::default(), None).await?;
        Ok(())
    }

    // ── Extra HTTP headers ────────────────────────────────────────────────

    /// Apply extra HTTP headers to all current pages in this context.
    ///
    /// Headers are set via `Network.setExtraHTTPHeaders` on each page session.
    /// Pages opened *after* this call will need to be set up separately (or
    /// call this method again after navigation).
    pub async fn set_extra_http_headers(
        &self,
        headers: std::collections::HashMap<String, String>,
    ) -> crate::Result<()> {
        use crate::cdp::browser_protocol::network as cdp_network;
        let json_map: serde_json::Value =
            serde_json::Value::Object(headers.into_iter().map(|(k, v)| (k, v.into())).collect());
        let params =
            cdp_network::SetExtraHttpHeadersParams::new(cdp_network::Headers::new(json_map));
        let target_ids = self.page_target_ids()?;
        for target_id in target_ids {
            if let Some(session_ref) = self.handle.session_cell(&target_id) {
                if let Err(e) =
                    self.handle.execute(params.clone(), Some(session_ref.current())).await
                {
                    tracing::warn!(
                        target = %target_id.inner(),
                        error = %e,
                        "set_extra_http_headers: per-target execute failed"
                    );
                }
            }
        }
        Ok(())
    }

    // ── Permissions ───────────────────────────────────────────────────────

    /// Grant browser permissions to this context (optionally scoped to `origin`).
    ///
    /// Delegates to `Browser.grantPermissions`.
    ///
    /// Note: `Browser.grantPermissions` is marked deprecated in the CDP spec
    /// in favour of `Browser.setPermission`, but it remains the only API that
    /// accepts a list of permissions in a single call. The `#[allow(deprecated)]`
    /// suppresses the compiler warning that originates from the generated CDP
    /// bindings.
    #[allow(deprecated)]
    pub async fn grant_permissions(
        &self,
        permissions: Vec<crate::cdp::browser_protocol::browser::PermissionType>,
        origin: Option<String>,
    ) -> crate::Result<()> {
        use crate::cdp::browser_protocol::browser as cdp_browser;
        let mut params = cdp_browser::GrantPermissionsParams::new(permissions);
        params.origin = origin;
        params.browser_context_id = self.id.clone();
        self.handle.execute(params, None).await?;
        Ok(())
    }

    /// Reset all permission overrides for this context.
    ///
    /// Delegates to `Browser.resetPermissions`.
    pub async fn clear_permissions(&self) -> crate::Result<()> {
        use crate::cdp::browser_protocol::browser as cdp_browser;
        let params = cdp_browser::ResetPermissionsParams::builder().build();
        let params = if let Some(ref ctx_id) = self.id {
            cdp_browser::ResetPermissionsParams::builder()
                .browser_context_id(ctx_id.clone())
                .build()
        } else {
            params
        };
        self.handle.execute(params, None).await?;
        Ok(())
    }

    // ── Context-level tracing ─────────────────────────────────────────────

    /// Return a browser-level [`TracingSession`](crate::tracing::TracingSession).
    ///
    /// Unlike page-level tracing (obtained via [`Page::tracing`](crate::Page::tracing)),
    /// this session sends `Tracing.*` commands at the browser level (no page
    /// session), which captures events across all pages in the context.
    pub fn tracing(&self) -> crate::tracing::TracingSession {
        crate::tracing::TracingSession::new_browser_level(self.handle.clone())
    }

    // ── WebSocket interception ────────────────────────────────────────────

    /// Intercept WebSocket connections whose URL matches `url_pattern` across
    /// all pages in this context.
    ///
    /// Uses JS shim injection + CDP `Runtime.addBinding` to fully intercept the
    /// connection bidirectionally.  The `handler` is called once per matching
    /// `new WebSocket(url)` with a [`WebSocketRoute`](crate::page::websocket_route::WebSocketRoute)
    /// that allows accepting, sending to, and closing the fake connection.
    ///
    /// The route is stored in the context and automatically applied to pages
    /// opened via [`BrowserContext::new_page`] after this call.
    ///
    /// Each existing page tracks the listener internally; new pages opened
    /// via [`BrowserContext::new_page`] inherit the route automatically.
    pub async fn route_websocket<F, Fut>(
        &self,
        url_pattern: impl Into<String>,
        handler: F,
    ) -> crate::Result<()>
    where
        F: Fn(crate::page::websocket_route::WebSocketRoute) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        use crate::page::websocket_route::WsRouteHandler;

        let pattern = url_pattern.into();
        let handler: WsRouteHandler = Arc::new(move |r| Box::pin(handler(r)));

        self.ws_routes
            .lock()
            .map_err(|_| CdpError::LockPoisoned)?
            .push((pattern.clone(), Arc::clone(&handler)));

        let pages = self.pages_internal().await;
        for page in pages {
            if page.setup_ws_shim(pattern.clone()).await.is_ok() {
                page.register_listener_task(page.spawn_ws_listener(Arc::clone(&handler)));
            }
        }

        Ok(())
    }

    /// Collect all live pages in this context without the full `pages()` overhead.
    pub(crate) async fn pages_internal(&self) -> Vec<crate::page::Page> {
        let target_ids = match self.page_target_ids() {
            Ok(ids) => ids,
            Err(_) => return Vec::new(),
        };
        let mut pages = Vec::with_capacity(target_ids.len());
        for id in target_ids {
            if let Ok(page) = crate::page::Page::attach(self.handle.clone(), id).await {
                pages.push(page);
            }
        }
        pages
    }

    // ── HAR replay ───────────────────────────────────────────────────────

    /// Register a route that serves matching requests from a HAR file.
    ///
    /// Requests whose URL is found in the HAR archive are fulfilled with the
    /// recorded response.  Behaviour for unmatched requests is controlled by
    /// [`HarOptions::not_found`](crate::har::HarOptions::not_found).
    ///
    /// The optional `options.url` glob limits which requests are intercepted;
    /// everything else is passed through regardless of `not_found`.
    pub async fn route_from_har(
        &self,
        path: impl AsRef<std::path::Path>,
        options: crate::har::HarOptions,
    ) -> crate::Result<()> {
        use crate::har::{HarLookup, HarNotFound};
        use crate::route::{glob_matches, RouteResponse};

        let bytes = tokio::fs::read(path.as_ref()).await?;
        let har: crate::har::HarPlayback = serde_json::from_slice(&bytes)?;
        let lookup = Arc::new(HarLookup::from_har(har));
        let not_found = options.not_found;
        let url_filter = options.url;

        let handler = Arc::new(
            move |route: crate::route::Route| -> std::pin::Pin<
                Box<dyn std::future::Future<Output = ()> + Send>,
            > {
                let lookup = Arc::clone(&lookup);
                let url_filter = url_filter.clone();
                Box::pin(async move {
                    let url = route.url().to_string();
                    let method = route.method().to_string();

                    if let Some(ref pat) = url_filter {
                        if !glob_matches(pat, &url) {
                            let _ = route.continue_req().await;
                            return;
                        }
                    }

                    if let Some(entry) = lookup.lookup(&method, &url) {
                        let status = entry.status;
                        let mut resp = RouteResponse::new().status(status);
                        for h in &entry.headers {
                            resp = resp.header(h.name.clone(), h.value.clone());
                        }
                        if let Some(ref text) = entry.content.text {
                            let body: Vec<u8> =
                                if entry.content.encoding.as_deref() == Some("base64") {
                                    use base64::Engine as _;
                                    match base64::engine::general_purpose::STANDARD.decode(text) {
                                        Ok(b) => b,
                                        Err(e) => {
                                            tracing::warn!(
                                                url = %url,
                                                error = %e,
                                                "HAR replay: base64 decode failed; serving raw text bytes"
                                            );
                                            text.as_bytes().to_vec()
                                        }
                                    }
                                } else {
                                    text.as_bytes().to_vec()
                                };
                            resp = resp.body(body);
                        }
                        if let Some(ref mime) = entry.content.mime_type {
                            resp = resp.content_type(mime.clone());
                        }
                        let _ = route.fulfill(resp).await;
                    } else {
                        match not_found {
                            HarNotFound::Abort => {
                                let _ = route
                                    .abort(crate::route::AbortReason::Failed)
                                    .await;
                            }
                            HarNotFound::Continue => {
                                let _ = route.continue_req().await;
                            }
                        }
                    }
                })
            },
        );

        self.registry.add("**", handler, false);
        Ok(())
    }
}
