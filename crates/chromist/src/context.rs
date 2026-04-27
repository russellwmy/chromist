//! Page-creating, popup-waiting, event-waiting, and lifecycle methods for
//! [`BrowserContext`](crate::BrowserContext).
//!
//! Per-context state lives on the struct itself (see [`crate::handler::context`]);
//! the methods here apply that state to newly-opened pages and surface
//! context-scoped event streams. They live in a separate module so they can
//! import [`Page`] without introducing a circular dependency:
//! `handler` ← `page` ← `context`.

use futures::StreamExt;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::cdp::browser_protocol::target as cdp_target;
use crate::error::CdpError;
use crate::handler::{BrowserContext, ContextBindingHandler, ContextExposeHandler};
use crate::listeners::EventStream;
use crate::page::Page;

/// A pending popup / new-page waiter.
///
/// Created by [`BrowserContext::wait_for_page`]. The event subscription begins
/// synchronously at construction time so no events are missed between creating
/// the waiter and triggering the action that opens the new page.
///
/// ```no_run
/// # async fn example(ctx: chromist::BrowserContext, page: chromist::Page) -> chromist::Result<()> {
/// let waiter = ctx.wait_for_page();
/// page.locator("#open-tab").click().await?;
/// let popup = waiter.wait(|_url| true).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct PageWaiter {
    stream: EventStream<cdp_target::TargetCreatedEvent>,
    handle: crate::handler::HandlerHandle,
    context_id: Option<crate::cdp::browser_protocol::browser::BrowserContextId>,
    timeout: Duration,
}

impl PageWaiter {
    /// Override the default 30 s timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Wait for the first new page in the context for which `predicate(url)`
    /// returns `true`.  Attaches to the target and returns a [`Page`].
    pub async fn wait<F>(mut self, predicate: F) -> crate::Result<Page>
    where
        F: Fn(&str) -> bool + Send,
    {
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(CdpError::Timeout);
            }
            match crate::runtime::timeout(remaining, self.stream.next()).await {
                Ok(Some(ev)) => {
                    let in_ctx = match (&self.context_id, &ev.target_info.browser_context_id) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    };
                    if ev.target_info.r#type == "page" && in_ctx && predicate(&ev.target_info.url) {
                        return Page::attach(self.handle.clone(), ev.target_info.target_id).await;
                    }
                }
                Ok(None) => return Err(CdpError::ChannelClosed),
                Err(_) => return Err(CdpError::Timeout),
            }
        }
    }
}

impl BrowserContext {
    /// Create a [`PageWaiter`] that starts listening for new pages immediately.
    ///
    /// Call this *before* triggering the action that opens the popup so the
    /// subscription is in place before the `Target.targetCreated` event fires.
    pub fn wait_for_page(&self) -> PageWaiter {
        PageWaiter {
            stream: self.handle.event_listener(None),
            handle: self.handle.clone(),
            context_id: self.id.clone(),
            timeout: Duration::from_secs(30),
        }
    }

    // ── Offline mode ──────────────────────────────────────────────────────

    /// Enable or disable offline mode for all current pages in this context.
    ///
    /// The setting is also applied to pages opened via [`new_page`](Self::new_page)
    /// after this call.
    pub async fn set_offline(&self, offline: bool) -> crate::Result<()> {
        self.offline.store(offline, Ordering::Relaxed);
        let pages = self.pages_internal().await;
        for page in pages {
            let _ = page.emulation().set_offline(offline).await;
        }
        Ok(())
    }

    // ── Geolocation ───────────────────────────────────────────────────────

    /// Override geolocation for all current pages and every page opened via
    /// [`new_page`](Self::new_page) after this call.
    pub async fn set_geolocation(&self, lat: f64, lon: f64, accuracy: f64) -> crate::Result<()> {
        *self.geolocation.lock().map_err(|_| CdpError::LockPoisoned)? = Some((lat, lon, accuracy));
        let pages = self.pages_internal().await;
        for page in pages {
            let _ = page.emulation().set_geolocation(lat, lon, accuracy).await;
        }
        Ok(())
    }

    /// Clear the geolocation override for all current pages in this context.
    pub async fn clear_geolocation(&self) -> crate::Result<()> {
        *self.geolocation.lock().map_err(|_| CdpError::LockPoisoned)? = None;
        use crate::cdp::browser_protocol::emulation as cdp_emulation;
        let params = cdp_emulation::SetGeolocationOverrideParams::default();
        let target_ids = self.page_target_ids()?;
        for target_id in target_ids {
            if let Some(session_ref) = self.handle.session_cell(&target_id) {
                let _ = self.handle.execute(params.clone(), Some(session_ref.current())).await;
            }
        }
        Ok(())
    }

    // ── Raw CDP session ───────────────────────────────────────────────────

    /// Open a raw CDP session on any target in this context.
    ///
    /// Returns `CdpError::NotFound` if the target has no attached session yet.
    pub fn new_cdp_session(
        &self,
        target_id: &crate::cdp::browser_protocol::target::TargetId,
    ) -> crate::Result<crate::CdpSession> {
        let session = self.handle.session_cell(target_id).ok_or(CdpError::NotFound)?;
        Ok(crate::CdpSession::new(self.handle.clone(), session))
    }

    // ── Event waiter ──────────────────────────────────────────────────────

    /// Block until the next event of type `T` is received from a target
    /// belonging to this context.
    ///
    /// Events are filtered by `session_id` → target → `browser_context_id`,
    /// so events from pages in other contexts and unscoped browser-level
    /// events are skipped.
    ///
    /// Defaults to a 30 s timeout; override with `Some(duration)`.
    pub async fn wait_for_event<T>(&self, timeout: Option<Duration>) -> crate::Result<T>
    where
        T: chromist_types::EventMessage + chromist_types::MethodType + Send + 'static,
    {
        let timeout = timeout.unwrap_or(Duration::from_secs(30));
        let target_method = T::method_id();
        let mut rx = self.handle.subscribe(None);
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(CdpError::Timeout);
            }
            match crate::runtime::timeout(remaining, rx.next()).await {
                Ok(Some(frame)) => {
                    if frame.method.as_str() != target_method.as_ref() {
                        continue;
                    }
                    let sid = match &frame.session_id {
                        Some(s) => s.clone(),
                        None => continue,
                    };
                    if !self.session_belongs_to_context(&sid)? {
                        continue;
                    }
                    match T::deserialize(&frame.params) {
                        Ok(v) => return Ok(v),
                        Err(e) => tracing::debug!(
                            method = %frame.method,
                            error = %e,
                            "wait_for_event: dropping event after deserialization failure"
                        ),
                    }
                }
                Ok(None) => return Err(CdpError::ChannelClosed),
                Err(_) => return Err(CdpError::Timeout),
            }
        }
    }

    /// Returns `true` if `session_id` belongs to a target whose
    /// `browser_context_id` matches this context.
    fn session_belongs_to_context(&self, session_id: &Arc<str>) -> crate::Result<bool> {
        let tree = self.handle.tree.read().map_err(|_| CdpError::LockPoisoned)?;
        let matches = tree.all().any(|e| {
            e.session.as_ref().is_some_and(|s| s.current().as_ref() == session_id.as_ref())
                && match (&self.id, &e.info.browser_context_id) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                }
        });
        Ok(matches)
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// Close this browser context: dispose all of its pages and tear down
    /// all CDP state associated with it.
    ///
    /// Returns [`CdpError::CannotCloseDefaultContext`] when called on the
    /// default (non-incognito) context — the default context's lifetime is
    /// tied to the browser process itself.
    ///
    /// After this call returns, all [`Page`] handles obtained from this
    /// context become invalid; further CDP calls on them will fail.
    pub async fn close(&self) -> crate::Result<()> {
        let ctx_id = match &self.id {
            Some(id) => id.clone(),
            None => return Err(CdpError::CannotCloseDefaultContext),
        };
        for target_id in self.page_target_ids()? {
            let _ = self.handle.execute(cdp_target::CloseTargetParams::new(target_id), None).await;
        }
        self.handle
            .execute(
                crate::cdp::browser_protocol::target::DisposeBrowserContextParams::new(ctx_id),
                None,
            )
            .await?;
        Ok(())
    }

    // ── Context-level expose ──────────────────────────────────────────────

    /// Expose a Rust async function to every page in this context.
    ///
    /// Equivalent to calling [`Page::expose_function`](crate::Page::expose_function) on every current page
    /// and automatically applying it to pages opened via [`new_page`](Self::new_page).
    /// Returns one [`JoinHandle`](tokio::task::JoinHandle) per current page.
    pub async fn expose_function<F, Fut>(
        &self,
        name: impl Into<String>,
        handler: F,
    ) -> crate::Result<()>
    where
        F: Fn(Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = serde_json::Value> + Send + 'static,
    {
        let name = name.into();
        let handler: ContextExposeHandler = Arc::new(move |args| Box::pin(handler(args)));
        self.expose_fns
            .lock()
            .map_err(|_| CdpError::LockPoisoned)?
            .push((name.clone(), Arc::clone(&handler)));
        let pages = self.pages_internal().await;
        for page in pages {
            let h = Arc::clone(&handler);
            let n = name.clone();
            // Each page tracks the dispatcher task internally on its
            // `listener_tasks`; ignore individual failures so partial success
            // across multiple pages still applies the handler everywhere it can.
            let _ = page.expose_function(n, move |args| h(args)).await;
        }
        Ok(())
    }

    /// Expose a Rust async function with source context to every page in this context.
    ///
    /// Like [`expose_function`](Self::expose_function) but the handler also
    /// receives the call-site `source` object.
    pub async fn expose_binding<F, Fut>(
        &self,
        name: impl Into<String>,
        handler: F,
    ) -> crate::Result<()>
    where
        F: Fn(serde_json::Value, Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = serde_json::Value> + Send + 'static,
    {
        let name = name.into();
        let handler: ContextBindingHandler =
            Arc::new(move |source, args| Box::pin(handler(source, args)));
        self.expose_bindings_store
            .lock()
            .map_err(|_| CdpError::LockPoisoned)?
            .push((name.clone(), Arc::clone(&handler)));
        let pages = self.pages_internal().await;
        for page in pages {
            let h = Arc::clone(&handler);
            let n = name.clone();
            let _ = page.expose_binding(n, move |source, args| h(source, args)).await;
        }
        Ok(())
    }

    // ── Page creation ─────────────────────────────────────────────────────

    /// Open a new tab in this browser context and return a [`Page`].
    ///
    /// All per-context state is wired up before returning: init scripts
    /// (via `Page.addScriptToEvaluateOnNewDocument`), WebSocket interception
    /// routes, route registry, exposed functions and bindings, offline mode,
    /// geolocation override, CSP bypass, JavaScript-enabled flag, HTTP
    /// credentials, service-worker policy, and downloads path.
    ///
    /// Pages opened here are closed when the context is closed via
    /// [`close`](Self::close).
    ///
    /// [`add_init_script`]: BrowserContext::add_init_script
    pub async fn new_page(&self, url: impl Into<String>) -> crate::Result<Page> {
        let mut params = cdp_target::CreateTargetParams::new(url.into());
        params.browser_context_id = self.id.clone();
        let resp = self.handle.execute(params, None).await?;
        let mut page = Page::attach(self.handle.clone(), resp.target_id).await?;

        // Wire up the context back-reference.
        page.set_browser_context(self.clone());

        // Wire up the context-level route registry.
        page.set_context_route_registry(self.registry.clone());

        // Inject registered init scripts.
        let scripts = self.init_scripts.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        for script in scripts {
            let mut params =
                crate::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams::new(
                    script,
                );
            params.world_name = None;
            let _ = page.execute(params).await;
        }

        // Apply registered WebSocket interception routes.
        let ws_routes = self.ws_routes.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        for (pattern, handler) in ws_routes {
            if page.setup_ws_shim(pattern).await.is_ok() {
                page.register_listener_task(page.spawn_ws_listener(handler));
            }
        }

        // Apply offline mode if set.
        if self.offline.load(Ordering::Relaxed) {
            let _ = page.emulation().set_offline(true).await;
        }

        // Apply geolocation override if set — copy out of the guard before awaiting.
        let geo = *self.geolocation.lock().map_err(|_| CdpError::LockPoisoned)?;
        if let Some((lat, lon, acc)) = geo {
            let _ = page.emulation().set_geolocation(lat, lon, acc).await;
        }

        // Apply context-level expose functions — collect first to avoid holding the lock across awaits.
        let fns: Vec<(String, ContextExposeHandler)> =
            self.expose_fns.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        for (name, handler) in fns {
            let h = Arc::clone(&handler);
            let _ = page.expose_function(name, move |args| h(args)).await;
        }

        // Apply context-level expose bindings.
        let bindings: Vec<(String, ContextBindingHandler)> =
            self.expose_bindings_store.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        for (name, handler) in bindings {
            let h = Arc::clone(&handler);
            let _ = page.expose_binding(name, move |source, args| h(source, args)).await;
        }

        // Apply CSP bypass.
        if self.bypass_csp.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = page.emulation().set_bypass_csp(true).await;
        }

        // Apply JavaScript disabled.
        if !self.javascript_enabled.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = page.emulation().set_javascript_enabled(false).await;
        }

        // Apply HTTP credentials via extra headers.
        let creds = self.http_credentials.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        if let Some(creds) = creds {
            use crate::cdp::browser_protocol::network as cdp_network;
            let mut headers = std::collections::HashMap::new();
            headers.insert("Authorization".to_string(), creds.as_header_value());
            let _ = page
                .execute(cdp_network::SetExtraHttpHeadersParams::new(cdp_network::Headers(
                    headers.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect(),
                )))
                .await;
        }

        // Block service workers via an init script.
        let sw_policy = *self.service_workers.lock().map_err(|_| CdpError::LockPoisoned)?;
        if sw_policy == crate::context_options::ServiceWorkersPolicy::Block {
            let block_script = "\
                Object.defineProperty(navigator, 'serviceWorker', {\
                    get: () => undefined, configurable: false\
                });";
            let mut params =
                crate::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams::new(
                    block_script.to_string(),
                );
            params.world_name = None;
            let _ = page.execute(params).await;
        }

        // Apply downloads path.
        let dl_path = self.downloads_path.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        if let Some(path) = dl_path {
            let _ = page.set_download_behavior(&path).await;
        }

        Ok(page)
    }

    /// Return all live pages in this browser context.
    ///
    /// Reads from the handler's target tree and attaches to each matching page
    /// target.  Targets that fail to attach are silently skipped.
    pub async fn pages(&self) -> crate::Result<Vec<Page>> {
        let target_ids = self.page_target_ids()?;
        let mut pages = Vec::with_capacity(target_ids.len());
        for id in target_ids {
            if let Ok(page) = Page::attach(self.handle.clone(), id).await {
                pages.push(page);
            }
        }
        Ok(pages)
    }

    // ── Service workers and background pages ─────────────────────────────

    /// Return all live background pages (Chrome extension background scripts)
    /// in this browser context.
    ///
    /// Reads from the handler's target tree and attaches to each matching
    /// `"background_page"` target.  Targets that fail to attach are silently
    /// skipped.
    pub async fn background_pages(&self) -> crate::Result<Vec<Page>> {
        let target_ids = self.background_page_target_ids()?;
        let mut pages = Vec::with_capacity(target_ids.len());
        for id in target_ids {
            if let Ok(page) = Page::attach(self.handle.clone(), id).await {
                pages.push(page);
            }
        }
        Ok(pages)
    }

    /// Wait for the next service worker to be created in this browser context.
    ///
    /// Returns the new [`Worker`](crate::events::Worker).  Defaults to a 30 s
    /// timeout; override with `Some(duration)`.
    pub async fn wait_for_service_worker(
        &self,
        timeout: Option<Duration>,
    ) -> crate::Result<crate::events::Worker> {
        let timeout = timeout.unwrap_or(Duration::from_secs(30));
        let mut stream: EventStream<cdp_target::TargetCreatedEvent> =
            self.handle.event_listener(None);
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(CdpError::Timeout);
            }
            match crate::runtime::timeout(remaining, stream.next()).await {
                Ok(Some(ev)) => {
                    let in_ctx = match (&self.id, &ev.target_info.browser_context_id) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    };
                    if ev.target_info.r#type == "service_worker" && in_ctx {
                        let session_id = self
                            .handle
                            .session_cell(&ev.target_info.target_id)
                            .map(|s| s.current())
                            .unwrap_or_else(|| {
                                Arc::from(ev.target_info.target_id.inner().as_str())
                            });
                        return Ok(crate::events::Worker {
                            url: ev.target_info.url.clone(),
                            handle: self.handle.clone(),
                            session_id,
                            listener_tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
                        });
                    }
                }
                Ok(None) => return Err(CdpError::ChannelClosed),
                Err(_) => return Err(CdpError::Timeout),
            }
        }
    }

    /// Wait for the next background page to be created in this browser context.
    ///
    /// Returns a [`Page`] attached to the new background page.  Defaults to a
    /// 30 s timeout; override with `Some(duration)`.
    pub async fn wait_for_background_page(&self, timeout: Option<Duration>) -> crate::Result<Page> {
        let timeout = timeout.unwrap_or(Duration::from_secs(30));
        let mut stream: EventStream<cdp_target::TargetCreatedEvent> =
            self.handle.event_listener(None);
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(CdpError::Timeout);
            }
            match crate::runtime::timeout(remaining, stream.next()).await {
                Ok(Some(ev)) => {
                    let in_ctx = match (&self.id, &ev.target_info.browser_context_id) {
                        (None, None) => true,
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    };
                    if ev.target_info.r#type == "background_page" && in_ctx {
                        return Page::attach(self.handle.clone(), ev.target_info.target_id).await;
                    }
                }
                Ok(None) => return Err(CdpError::ChannelClosed),
                Err(_) => return Err(CdpError::Timeout),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for `BrowserContext::close` and `BrowserContext::wait_for_event`,
    //! exercised against [`MockConnection`] without a real browser.

    use super::*;
    use crate::conn::{mock::MockConnection, AnyConnection};
    use crate::handler::Handler;
    use chromist_types::{CdpJsonEventMessage, Message, Response};
    use serde_json::json;

    fn target_attached(
        target_id: &str,
        ctx_id: Option<&str>,
        session_id: &str,
    ) -> Message<CdpJsonEventMessage> {
        let mut info = json!({
            "targetId": target_id,
            "type": "page",
            "title": "",
            "url": "about:blank",
            "attached": true,
            "canAccessOpener": false,
        });
        if let Some(c) = ctx_id {
            info.as_object_mut().unwrap().insert("browserContextId".into(), json!(c));
        }
        Message::Event(CdpJsonEventMessage {
            method: "Target.attachedToTarget".into(),
            params: json!({
                "sessionId": session_id,
                "targetInfo": info,
                "waitingForDebugger": false,
            }),
            session_id: None,
        })
    }

    async fn poll_until<T>(mut pred: impl FnMut() -> Option<T>) -> T {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(v) = pred() {
                return v;
            }
            if std::time::Instant::now() >= deadline {
                panic!("timed out waiting for handler state");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn close_default_context_returns_typed_error() {
        let (conn, _mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        let ctx = BrowserContext::new_with_id(handle.clone(), None);
        let res = ctx.close().await;
        assert!(matches!(res, Err(CdpError::CannotCloseDefaultContext)), "got {:?}", res);

        handle.shutdown();
    }

    #[tokio::test]
    async fn close_disposes_pages_then_context() {
        let (conn, mut mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        let ctx_id_str = "CTX-1";
        mock.inbound.unbounded_send(Ok(target_attached("T-1", Some(ctx_id_str), "S-1"))).unwrap();

        let ctx_id = crate::cdp::browser_protocol::browser::BrowserContextId(ctx_id_str.into());
        let ctx = BrowserContext::new_with_id(handle.clone(), Some(ctx_id));

        // Wait for the tree to register the page in this context.
        poll_until(|| {
            let ids = ctx.page_target_ids().ok()?;
            if ids.iter().any(|t| t.inner() == "T-1") {
                Some(())
            } else {
                None
            }
        })
        .await;

        let task = tokio::spawn({
            let ctx = ctx.clone();
            async move { ctx.close().await }
        });

        let mut saw_close_target = false;
        let mut saw_dispose = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !(saw_close_target && saw_dispose) {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!remaining.is_zero(), "did not observe close+dispose in time");
            let cmd = match tokio::time::timeout(remaining, mock.outbound.next()).await {
                Ok(Some(c)) => c,
                _ => panic!("outbound stream ended before close+dispose"),
            };
            match cmd.method.as_ref() {
                "Target.closeTarget" => {
                    assert_eq!(cmd.params["targetId"], json!("T-1"));
                    saw_close_target = true;
                }
                "Target.disposeBrowserContext" => {
                    assert_eq!(cmd.params["browserContextId"], json!(ctx_id_str));
                    saw_dispose = true;
                }
                other => panic!("unexpected command: {other}"),
            }
            mock.inbound
                .unbounded_send(Ok(Message::Response(Box::new(Response {
                    id: cmd.id,
                    result: Some(json!({})),
                    error: None,
                }))))
                .unwrap();
        }

        let res = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("close resolved")
            .expect("no panic");
        assert!(res.is_ok(), "expected Ok, got {:?}", res);

        handle.shutdown();
    }

    /// Test event type modelled after `listeners.rs::tests::TestEvent`.
    #[derive(serde::Deserialize, Debug, PartialEq)]
    struct TestEvent {
        value: i32,
    }
    impl chromist_types::MethodType for TestEvent {
        fn method_id() -> chromist_types::MethodId {
            std::borrow::Cow::Borrowed("Test.event")
        }
    }
    impl chromist_types::Method for TestEvent {
        fn identifier(&self) -> chromist_types::MethodId {
            std::borrow::Cow::Borrowed("Test.event")
        }
    }
    impl chromist_types::EventMessage for TestEvent {}

    fn raw_event(
        method: &str,
        session_id: &str,
        params: serde_json::Value,
    ) -> Message<CdpJsonEventMessage> {
        Message::Event(CdpJsonEventMessage {
            method: method.into(),
            params,
            session_id: Some(Arc::from(session_id)),
        })
    }

    #[tokio::test]
    async fn wait_for_event_filters_other_contexts() {
        let (conn, mock) = MockConnection::pair();
        let handle = Handler::spawn(AnyConnection::Mock(conn));

        mock.inbound.unbounded_send(Ok(target_attached("T-A", Some("CTX-A"), "S-A"))).unwrap();
        mock.inbound.unbounded_send(Ok(target_attached("T-B", Some("CTX-B"), "S-B"))).unwrap();

        let ctx_a_id = crate::cdp::browser_protocol::browser::BrowserContextId("CTX-A".into());
        let ctx_a = BrowserContext::new_with_id(handle.clone(), Some(ctx_a_id));

        poll_until(|| {
            let tree = handle.tree.read().ok()?;
            let has_a =
                tree.all().any(|e| e.info.target_id.inner() == "T-A" && e.session.is_some());
            let has_b =
                tree.all().any(|e| e.info.target_id.inner() == "T-B" && e.session.is_some());
            if has_a && has_b {
                Some(())
            } else {
                None
            }
        })
        .await;

        let task = tokio::spawn({
            let ctx = ctx_a.clone();
            async move { ctx.wait_for_event::<TestEvent>(Some(std::time::Duration::from_secs(2))).await }
        });

        mock.inbound
            .unbounded_send(Ok(raw_event("Test.event", "S-B", json!({"value": 1}))))
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!task.is_finished(), "wait_for_event resolved on event from another context");

        mock.inbound
            .unbounded_send(Ok(raw_event("Test.event", "S-A", json!({"value": 42}))))
            .unwrap();

        let res = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("waiter resolved")
            .expect("no panic")
            .expect("event delivered");
        assert_eq!(res, TestEvent { value: 42 });

        handle.shutdown();
    }
}
