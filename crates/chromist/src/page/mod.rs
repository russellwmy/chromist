use futures::StreamExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use crate::cdp::browser_protocol::network as cdp_network;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::browser_protocol::target as cdp_target;
use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::error::CdpError;
use crate::events::{ConsoleLocation, ConsoleMessage};
use crate::frame_tree::{FrameTree, UTILITY_WORLD_NAME};
use crate::handler::{HandlerHandle, SessionRef};
use crate::route::RouteRegistry;

pub(in crate::page) use crate::runtime::AbortOnDrop;

/// Registered page-level event callbacks, shared between `Page` and the single
/// background event-dispatch task spawned per page.
///
/// Handlers are pushed here via `on_console`, `on_page_error`, and
/// `on_websocket`; the background task clones the relevant `Arc`s and calls
/// them without holding the lock.
#[derive(Default)]
pub(in crate::page) struct PageEventSinks {
    pub(in crate::page) console_handlers: Vec<Arc<dyn Fn(ConsoleMessage) + Send + Sync>>,
    pub(in crate::page) page_error_handlers: Vec<Arc<dyn Fn(String) + Send + Sync>>,
    pub(in crate::page) websocket_handlers:
        Vec<Arc<dyn Fn(crate::websocket::WebSocket) + Send + Sync>>,
}

mod accessibility;
mod capture;
mod content;
pub(crate) mod cookies;
mod device;
pub(crate) mod dom;
pub(crate) mod emulation;
mod evaluation;
pub(crate) mod events;
mod expose;
mod har_recording;
pub(crate) mod input;
mod intercept;
mod navigation;
pub(crate) mod raw_cdp;
mod route;
pub(crate) mod websocket_route;

/// The emulated CSS media type for a page.
///
/// # Examples
///
/// ```
/// use chromist::MediaType;
/// assert_eq!(MediaType::Screen.to_string(), "screen");
/// assert_eq!(MediaType::Print.to_string(), "print");
/// assert_eq!(MediaType::None.to_string(), "");
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum MediaType {
    /// Emulate the `screen` media type — the page renders as it would on a
    /// regular display.
    Screen,
    /// Emulate the `print` media type — the page renders with print stylesheets.
    Print,
    /// Clear the media-type override; the page reverts to the device default.
    None,
}

impl std::fmt::Display for MediaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaType::Screen => f.write_str("screen"),
            MediaType::Print => f.write_str("print"),
            MediaType::None => f.write_str(""),
        }
    }
}

/// Source of a `<script>` tag injected by [`Page::add_script_tag`].
///
/// Construct with [`ScriptSource::url`] for an external file loaded via `src`,
/// or [`ScriptSource::inline`] for a literal source string set as
/// `textContent`. The variants are mutually exclusive — a single call cannot
/// both load and inline.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ScriptSource {
    /// Load `<script src="…">`. The future returned by
    /// [`Page::add_script_tag`] resolves only after the script's `load` event.
    Url(String),
    /// Inline source as `<script>…</script>` via `textContent`. The future
    /// resolves as soon as the tag is appended.
    Inline(String),
}

impl ScriptSource {
    /// Construct a [`ScriptSource::Url`].
    pub fn url(url: impl Into<String>) -> Self {
        ScriptSource::Url(url.into())
    }

    /// Construct a [`ScriptSource::Inline`].
    pub fn inline(source: impl Into<String>) -> Self {
        ScriptSource::Inline(source.into())
    }
}

/// A handle to a browser tab.
///
/// `Page` is the primary entry point for navigation, evaluation, screenshots,
/// PDFs, and locator-based DOM interaction. Cheaply cloneable — every clone
/// shares the same target session, frame tree, route registry, and listener
/// tasks. Dropping the last clone aborts background event handlers tied to
/// the page.
///
/// Obtain a page from [`Browser::new_page`](crate::Browser::new_page) or
/// [`BrowserContext::new_page`](crate::BrowserContext::new_page).
///
/// Sub-handle accessors group related functionality that would otherwise
/// pollute the top-level method list:
///
/// | Accessor | Returns | Purpose |
/// |---|---|---|
/// | [`page.dom()`](Self::dom) | [`Dom`](crate::Dom) | `find_element`, `find_xpath`, `wait_for_selector`, … |
/// | [`page.emulation()`](Self::emulation) | [`Emulation`](crate::Emulation) | viewport, user-agent, geolocation, network conditions |
/// | [`page.cookies()`](Self::cookies) | [`Cookies`](crate::Cookies) | cookie reads / writes / deletes |
/// | [`page.events()`](Self::events) | [`Events`](crate::Events) | `on_console`, `on_download`, `on_worker`, … |
/// | [`page.mouse()`](Self::mouse) | [`Mouse`](crate::Mouse) | pointer events |
/// | [`page.keyboard()`](Self::keyboard) | [`Keyboard`](crate::Keyboard) | keyboard events |
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> chromist::Result<()> {
/// use chromist::Browser;
/// let browser = Browser::launch_default().await?;
/// let page = browser.new_page("https://example.com").await?;
/// let title: String = page.evaluate("document.title").await?.into_value()?;
/// assert!(!title.is_empty());
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Page {
    pub(in crate::page) handle: HandlerHandle,
    pub(in crate::page) target_id: cdp_target::TargetId,
    /// Live cell that always resolves to the current session id for this
    /// target. The handler updates the cell on `Target.attachedToTarget` so
    /// that cross-process navigations (which detach the old session and
    /// attach a new one) are transparent to callers.
    pub(in crate::page) session_id: SessionRef,
    /// Set to `true` when a `Target.targetDestroyed` event fires for this page's
    /// target ID.  Shared via `Arc` so clones and the background watcher task
    /// all observe the same flag.
    pub(in crate::page) closed: Arc<AtomicBool>,
    /// Live frame state: URL, loading status, and execution context IDs per frame.
    /// Updated by the background subscription started in [`Page::attach`].
    pub(in crate::page) frame_tree: Arc<RwLock<FrameTree>>,
    /// Stateful mouse handle; tracks the current cursor position.
    pub(in crate::page) mouse: input::Mouse,
    /// Stateful keyboard handle; tracks currently-held modifier keys.
    pub(in crate::page) keyboard: input::Keyboard,
    /// Page-level route registry. Evaluated before `context_route_registry`.
    pub(in crate::page) route_registry: RouteRegistry,
    /// Optional context-level route registry. Evaluated after page-level rules.
    pub(in crate::page) context_route_registry: Option<RouteRegistry>,
    /// Handle to the `Fetch.requestPaused` background listener task.
    /// `None` when no routes are registered.
    pub(in crate::page) route_task: Arc<Mutex<Option<AbortOnDrop>>>,
    /// Default timeout for all locator / element actions (milliseconds).
    pub(in crate::page) default_timeout_ms: Arc<AtomicU64>,
    /// Default timeout for navigation operations (milliseconds).
    pub(in crate::page) default_navigation_timeout_ms: Arc<AtomicU64>,
    /// The browser context this page belongs to, if created via one.
    pub(in crate::page) browser_context: Option<crate::handler::BrowserContext>,
    /// Lazily-initialised network manager. Created on first call to
    /// `Page::network()` or `Page::requests()`.
    pub(in crate::page) network_manager: Arc<Mutex<Option<crate::network::NetworkManager>>>,
    /// Registered page-level callbacks dispatched by the single background task.
    pub(in crate::page) event_sinks: Arc<Mutex<PageEventSinks>>,
    /// Download directory configured via [`Page::set_download_path`].
    /// `None` means use the system temp directory (the default).
    pub(in crate::page) download_dir: Arc<Mutex<Option<std::path::PathBuf>>>,
    /// Aborts the per-page event-dispatch task when the last `Page` clone is
    /// dropped, preventing the task from leaking past the page's lifetime.
    pub(in crate::page) _event_task: Arc<AbortOnDrop>,
    /// Tasks spawned by fire-and-forget listener registration methods
    /// ([`Page::on_download`], [`Page::on_worker`]). Stored here so they are
    /// aborted automatically when the last `Page` clone is dropped instead of
    /// running until the underlying event channel closes.
    pub(crate) listener_tasks: Arc<Mutex<Vec<AbortOnDrop>>>,
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("target_id", &self.target_id)
            .field("session_id", &self.session_id.current())
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Page {
    /// Attach `task` to the page's lifetime: it is aborted when the last
    /// `Page` clone is dropped. Used by listener-registration methods that
    /// follow the chainable, fire-and-forget pattern.
    pub(crate) fn register_listener_task(&self, task: AbortOnDrop) {
        if let Ok(mut tasks) = self.listener_tasks.lock() {
            tasks.push(task);
        } else {
            tracing::warn!("Page listener_tasks lock poisoned; spawned task not registered");
        }
    }
}

impl Page {
    /// Attach to an existing target and return a Page.
    ///
    /// If the browser-level `setAutoAttach` has already attached to this
    /// target, the auto-attach event populates the handler's session cell;
    /// this constructor reuses it. Otherwise (e.g. the initial `about:blank`
    /// that predates the auto-attach configuration) it issues an explicit
    /// `Target.attachToTarget(flatten=true)` and seeds the cell.
    #[tracing::instrument(skip_all, fields(target_id = %target_id.inner()))]
    pub async fn attach(
        handle: HandlerHandle,
        target_id: cdp_target::TargetId,
    ) -> crate::Result<Self> {
        let session_id = match handle.session_cell(&target_id) {
            Some(cell) => cell,
            None => {
                let mut params = cdp_target::AttachToTargetParams::new(target_id.clone());
                params.flatten = Some(true);
                let resp = handle.execute(params, None).await?;
                let initial: Arc<str> = Arc::from(resp.session_id.0.as_str());
                handle.ensure_session_cell(&target_id, initial)
            }
        };

        // Enable the Page domain so navigation events are emitted. Failures
        // here are non-fatal but log a warning — without Page.enable, no
        // navigation/lifecycle events will fire and downstream waiters will
        // hang or time out.
        if let Err(e) =
            handle.execute(cdp_page::EnableParams::default(), Some(session_id.current())).await
        {
            tracing::warn!(error = %e, "Page.enable failed during attach; lifecycle events may not fire");
        }

        // Enable fine-grained lifecycle events (load, DOMContentLoaded, networkIdle, …).
        if let Err(e) = handle
            .execute(
                cdp_page::SetLifecycleEventsEnabledParams::new(true),
                Some(session_id.current()),
            )
            .await
        {
            tracing::warn!(error = %e, "Page.setLifecycleEventsEnabled failed during attach");
        }

        // Enable the Runtime domain so executionContext events fire.
        if let Err(e) =
            handle.execute(cdp_runtime::EnableParams::default(), Some(session_id.current())).await
        {
            tracing::warn!(error = %e, "Runtime.enable failed during attach; evaluation may not work");
        }

        // Prime the utility world so it exists before any page script on every
        // future navigation. The browser fires executionContextCreated for it
        // automatically; our subscription below will catch those events.
        let mut init_params = cdp_page::AddScriptToEvaluateOnNewDocumentParams::new(String::new());
        init_params.world_name = Some(UTILITY_WORLD_NAME.to_string());
        if let Err(e) = handle.execute(init_params, Some(session_id.current())).await {
            tracing::debug!(error = %e, "AddScriptToEvaluateOnNewDocument (utility world) failed");
        }

        // Fetch the current frame tree to seed local state and create the
        // utility world for the main frame before any navigation event.
        let frame_tree = Arc::new(RwLock::new(FrameTree::new()));
        let cdp_tree_resp = handle
            .execute(cdp_page::GetFrameTreeParams::default(), Some(session_id.current()))
            .await;
        if let Ok(ref resp) = cdp_tree_resp {
            if let Ok(mut tree) = frame_tree.write() {
                tree.seed(&resp.frame_tree);
            }
            let mut params =
                cdp_page::CreateIsolatedWorldParams::new(resp.frame_tree.frame.id.clone());
            params.world_name = Some(UTILITY_WORLD_NAME.to_string());
            params.grant_univeral_access = Some(true);
            if let Err(e) = handle.execute(params, Some(session_id.current())).await {
                tracing::debug!(error = %e, "CreateIsolatedWorld for main frame failed");
            }
        }

        // Allocate the destroy flag *before* spawning the event task so the
        // task can react to target destruction and exit without waiting for
        // the global event channel to close.
        let closed = Arc::new(AtomicBool::new(false));

        // Single background task per page: feeds frame-tree events AND
        // dispatches registered page-level event callbacks (console,
        // page-error, WebSocket).  Consolidating into one task eliminates the
        // N-tasks-per-page growth that occurred when callers registered
        // multiple `on_xxx` handlers.
        let event_sinks: Arc<Mutex<PageEventSinks>> =
            Arc::new(Mutex::new(PageEventSinks::default()));
        let event_task = {
            use serde::Deserialize as _;
            let frame_tree = Arc::clone(&frame_tree);
            let sinks = Arc::clone(&event_sinks);
            let closed = Arc::clone(&closed);
            let mut raw = handle.subscribe(Some(session_id.current()));
            crate::runtime::spawn(async move {
                let mut ws_close_flags: std::collections::HashMap<
                    crate::cdp::browser_protocol::network::RequestId,
                    Arc<AtomicBool>,
                > = std::collections::HashMap::new();
                while let Some(frame) = raw.next().await {
                    // Exit early once the target is destroyed so the task
                    // does not outlive the page it serves.
                    if closed.load(Ordering::Acquire) {
                        break;
                    }
                    let method = frame.method.as_str();
                    let params = &frame.params;

                    // --- Frame tree maintenance ---
                    let is_page_frame = matches!(
                        method,
                        "Page.frameAttached"
                            | "Page.frameNavigated"
                            | "Page.frameDetached"
                            | "Page.frameStartedLoading"
                            | "Page.frameStoppedLoading"
                    );
                    let is_runtime_ctx = matches!(
                        method,
                        "Runtime.executionContextCreated"
                            | "Runtime.executionContextDestroyed"
                            | "Runtime.executionContextsCleared"
                    );
                    if is_page_frame || is_runtime_ctx {
                        match frame_tree.write() {
                            Ok(mut tree) => {
                                if is_page_frame {
                                    tree.apply_page_event(method, params);
                                } else {
                                    tree.apply_runtime_event(method, params);
                                }
                            }
                            Err(_) => tracing::warn!(
                                method,
                                "page frame_tree lock poisoned; frame-tree event lost"
                            ),
                        }
                        continue;
                    }

                    // --- Registered event callbacks ---
                    // Snapshot the relevant handlers under a brief lock, then
                    // drop the lock before calling them.
                    match method {
                        "Runtime.consoleAPICalled" => {
                            let handlers: Vec<Arc<dyn Fn(ConsoleMessage) + Send + Sync>> =
                                match sinks.lock() {
                                    Ok(g) => g.console_handlers.clone(),
                                    Err(_) => {
                                        tracing::warn!(
                                            "page event sink lock poisoned; dropping console event"
                                        );
                                        continue;
                                    }
                                };
                            if handlers.is_empty() {
                                continue;
                            }
                            match cdp_runtime::ConsoleApiCalledEvent::deserialize(params) {
                                Err(e) => tracing::trace!(
                                    method = "Runtime.consoleAPICalled",
                                    error = %e,
                                    "dropping event: deserialization failed"
                                ),
                                Ok(ev) => {
                                    let args: Vec<serde_json::Value> = ev
                                        .args
                                        .iter()
                                        .map(|a| {
                                            a.value.clone().unwrap_or_else(|| {
                                                a.description
                                                    .as_deref()
                                                    .map(serde_json::Value::from)
                                                    .unwrap_or(serde_json::Value::Null)
                                            })
                                        })
                                        .collect();
                                    let text = ev
                                        .args
                                        .iter()
                                        .filter_map(|a| {
                                            a.description.clone().or_else(|| {
                                                a.value.as_ref().map(|v| match v {
                                                    serde_json::Value::String(s) => s.clone(),
                                                    other => other.to_string(),
                                                })
                                            })
                                        })
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    let location = ev
                                        .stack_trace
                                        .as_ref()
                                        .and_then(|st| st.call_frames.first())
                                        .map(|cf| ConsoleLocation {
                                            url: cf.url.clone(),
                                            line_number: cf.line_number,
                                            column_number: cf.column_number,
                                        });
                                    let msg = ConsoleMessage {
                                        kind: ev.r#type.as_ref().to_string(),
                                        text,
                                        args,
                                        location,
                                    };
                                    for h in &handlers {
                                        h(msg.clone());
                                    }
                                }
                            }
                        }
                        "Runtime.exceptionThrown" => {
                            let lock = sinks.lock();
                            let handlers: Vec<Arc<dyn Fn(String) + Send + Sync>> = match lock {
                                Ok(g) => g.page_error_handlers.clone(),
                                Err(_) => {
                                    tracing::warn!(
                                        "page event sink lock poisoned; dropping page-error event"
                                    );
                                    continue;
                                }
                            };
                            if handlers.is_empty() {
                                continue;
                            }
                            match cdp_runtime::ExceptionThrownEvent::deserialize(params) {
                                Err(e) => tracing::trace!(
                                    method = "Runtime.exceptionThrown",
                                    error = %e,
                                    "dropping event: deserialization failed"
                                ),
                                Ok(ev) => {
                                    let details = &ev.exception_details;
                                    let msg = details
                                        .exception
                                        .as_ref()
                                        .and_then(|e| e.description.clone())
                                        .or_else(|| Some(details.text.clone()))
                                        .unwrap_or_default();
                                    for h in &handlers {
                                        h(msg.clone());
                                    }
                                }
                            }
                        }
                        "Network.webSocketCreated" => {
                            let handlers: Vec<
                                Arc<dyn Fn(crate::websocket::WebSocket) + Send + Sync>,
                            > = match sinks.lock() {
                                Ok(g) => g.websocket_handlers.clone(),
                                Err(_) => {
                                    tracing::warn!(
                                        "page event sink lock poisoned; dropping websocket event"
                                    );
                                    continue;
                                }
                            };
                            if handlers.is_empty() {
                                continue;
                            }
                            match cdp_network::WebSocketCreatedEvent::deserialize(params) {
                                Err(e) => tracing::trace!(
                                    method = "Network.webSocketCreated",
                                    error = %e,
                                    "dropping event: deserialization failed"
                                ),
                                Ok(ev) => {
                                    let is_closed = Arc::new(AtomicBool::new(false));
                                    ws_close_flags
                                        .insert(ev.request_id.clone(), Arc::clone(&is_closed));
                                    let ws = crate::websocket::WebSocket::new(
                                        ev.url.clone(),
                                        ev.request_id,
                                        is_closed,
                                    );
                                    for h in &handlers {
                                        h(ws.clone());
                                    }
                                }
                            }
                        }
                        "Network.webSocketClosed" => {
                            match cdp_network::WebSocketClosedEvent::deserialize(params) {
                                Err(e) => tracing::trace!(
                                    method = "Network.webSocketClosed",
                                    error = %e,
                                    "dropping event: deserialization failed"
                                ),
                                Ok(ev) => {
                                    if let Some(flag) = ws_close_flags.remove(&ev.request_id) {
                                        flag.store(true, Ordering::Relaxed);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            })
        };

        // Wire the destroy flag into the handler so `is_closed()` is a
        // zero-cost atomic read and the per-page event task above will exit
        // promptly once the browser destroys the target.
        handle.register_destroy_watcher(&target_id, &closed);

        let mouse = input::Mouse::new(handle.clone(), session_id.clone());
        let keyboard = input::Keyboard::new(handle.clone(), session_id.clone());

        Ok(Page {
            handle,
            target_id,
            session_id,
            closed,
            frame_tree,
            mouse,
            keyboard,
            route_registry: RouteRegistry::new(),
            context_route_registry: None,
            route_task: Arc::new(Mutex::new(None)),
            default_timeout_ms: Arc::new(AtomicU64::new(30_000)),
            default_navigation_timeout_ms: Arc::new(AtomicU64::new(30_000)),
            browser_context: None,
            network_manager: Arc::new(Mutex::new(None)),
            event_sinks,
            download_dir: Arc::new(Mutex::new(None)),
            _event_task: Arc::new(AbortOnDrop(event_task)),
            listener_tasks: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Returns the CDP target ID for this page.
    pub fn target_id(&self) -> &cdp_target::TargetId {
        &self.target_id
    }

    /// Returns the current CDP session ID used to scope commands to this page.
    ///
    /// This may change across cross-process navigations — the value reflects
    /// whatever session is live at the moment of the call.
    pub fn session_id(&self) -> Arc<str> {
        self.session_id.current()
    }

    /// Execute any CDP command scoped to this page's session.
    ///
    /// This is the **raw CDP** entry point — pass a typed
    /// [`Command`](chromist_types::Command) (e.g. from
    /// [`crate::cdp`]) and get back its `Response`. For evaluating
    /// JavaScript source, use [`Page::evaluate`](crate::Page::evaluate) /
    /// [`Page::evaluate`](crate::page::Page::evaluate) instead — they handle
    /// the `Runtime.evaluate` plumbing and exception unwrapping for you.
    pub async fn execute<C>(&self, cmd: C) -> crate::Result<C::Response>
    where
        C: chromist_types::Command + serde::Serialize + Send + 'static,
        C::Response: serde::de::DeserializeOwned + Send + 'static,
    {
        self.handle.execute(cmd, Some(self.session_id.current())).await
    }

    /// Returns a typed event stream for events on this page's session.
    pub fn event_listener<T>(&self) -> crate::listeners::EventStream<T>
    where
        T: chromist_types::EventMessage + chromist_types::MethodType + Send + 'static,
    {
        self.handle.event_listener(Some(self.session_id.current()))
    }

    /// Returns `true` if the target for this page has been destroyed.
    ///
    /// This is a zero-cost synchronous read of an atomic flag set by a
    /// background task watching `Target.targetDestroyed` events — no CDP
    /// round-trip is required.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Returns the [`BrowserContext`](crate::BrowserContext) that owns this page,
    /// or `None` when the page was created directly from a `Browser`.
    pub fn context(&self) -> Option<&crate::handler::BrowserContext> {
        self.browser_context.as_ref()
    }

    pub(crate) fn set_browser_context(&mut self, ctx: crate::handler::BrowserContext) {
        self.browser_context = Some(ctx);
    }

    /// Returns the target ID of the page that opened this one, if any.
    pub async fn opener_id(&self) -> crate::Result<Option<cdp_target::TargetId>> {
        let resp = self.handle.execute(cdp_target::GetTargetsParams::default(), None).await?;
        for info in resp.target_infos {
            if info.target_id == self.target_id {
                return Ok(info.opener_id);
            }
        }
        Ok(None)
    }

    /// Subscribe to `Page.javascriptDialogOpening` events on this page.
    pub fn dialog_stream(
        &self,
    ) -> crate::listeners::EventStream<cdp_page::JavascriptDialogOpeningEvent> {
        self.event_listener()
    }

    /// Alias for [`Page::dialog_stream`].
    pub fn on_dialog(
        &self,
    ) -> crate::listeners::EventStream<cdp_page::JavascriptDialogOpeningEvent> {
        self.dialog_stream()
    }

    /// Accept the currently open JavaScript dialog, optionally supplying
    /// `prompt_text` for `prompt()` dialogs.
    pub async fn accept_dialog(&self, prompt_text: Option<String>) -> crate::Result<()> {
        let mut params = cdp_page::HandleJavaScriptDialogParams::new(true);
        params.prompt_text = prompt_text;
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Dismiss (cancel) the currently open JavaScript dialog.
    pub async fn dismiss_dialog(&self) -> crate::Result<()> {
        let params = cdp_page::HandleJavaScriptDialogParams::new(false);
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Wait for the next file chooser to open, returning an interceptor that
    /// can accept or cancel the selection.
    ///
    /// Internally enables `Page.setInterceptFileChooserDialog`; the browser
    /// will suppress the native chooser until `accept` / `cancel` is called.
    pub async fn wait_for_file_chooser(&self) -> crate::Result<crate::file_chooser::FileChooser> {
        // Subscribe *before* enabling interception so we don't miss a fast event.
        let mut stream: crate::listeners::EventStream<cdp_page::FileChooserOpenedEvent> =
            self.event_listener();
        self.handle
            .execute(
                cdp_page::SetInterceptFileChooserDialogParams::new(true),
                Some(self.session_id.current()),
            )
            .await?;
        let event =
            crate::runtime::timeout(Duration::from_secs(30), async move { stream.next().await })
                .await
                .map_err(|_| CdpError::Timeout)?
                .ok_or(CdpError::ChannelClosed)?;
        Ok(crate::file_chooser::FileChooser::new(
            self.handle.clone(),
            self.session_id.clone(),
            event,
        ))
    }

    /// Returns a [`JsCoverage`](crate::coverage::JsCoverage) controller for
    /// this page.
    pub fn js_coverage(&self) -> crate::coverage::JsCoverage {
        crate::coverage::JsCoverage::new(self.handle.clone(), self.session_id.clone())
    }

    /// Returns a [`CssCoverage`](crate::coverage::CssCoverage) controller for
    /// this page.
    pub fn css_coverage(&self) -> crate::coverage::CssCoverage {
        crate::coverage::CssCoverage::new(self.handle.clone(), self.session_id.clone())
    }

    /// Returns a [`TracingSession`](crate::tracing::TracingSession) controller
    /// for this page.
    pub fn tracing(&self) -> crate::tracing::TracingSession {
        crate::tracing::TracingSession::new(self.handle.clone(), self.session_id.clone())
    }

    /// Create a [`Locator`](crate::locator::Locator) bound to this page's main
    /// frame. The locator re-resolves the element on every action and operates
    /// in strict mode by default (more than one match → error).
    pub fn locator(&self, selector: impl Into<String>) -> crate::locator::Locator {
        let frame_id = self
            .frame_tree
            .read()
            .ok()
            .and_then(|t| t.main_frame().map(|f| f.id.clone()))
            .unwrap_or_else(|| crate::cdp::browser_protocol::page::FrameId::new(""));
        let frame = crate::frame::Frame::new(
            self.handle.clone(),
            self.session_id.clone(),
            frame_id,
            std::sync::Arc::clone(&self.frame_tree),
        )
        .with_page(self.clone());
        crate::locator::Locator::new(frame, selector)
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements containing the
    /// given exact text.
    pub fn get_by_text(&self, text: impl Into<String>) -> crate::locator::Locator {
        self.main_frame_locator_by_js(crate::locator::js_get_by_text(&text.into()))
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// ARIA `role`, optionally filtered by accessible name (`aria-label` or text content).
    pub fn get_by_role(
        &self,
        role: impl Into<String>,
        name: Option<&str>,
    ) -> crate::locator::Locator {
        self.main_frame_locator_by_js(crate::locator::js_get_by_role(&role.into(), name))
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds form controls associated
    /// with the given label text.
    pub fn get_by_label(&self, label: impl Into<String>) -> crate::locator::Locator {
        self.main_frame_locator_by_js(crate::locator::js_get_by_label(&label.into()))
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `placeholder` attribute value.
    pub fn get_by_placeholder(&self, text: impl Into<String>) -> crate::locator::Locator {
        self.main_frame_locator_by_js(crate::locator::js_get_by_placeholder(&text.into()))
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `data-testid` attribute value.
    pub fn get_by_test_id(&self, id: impl Into<String>) -> crate::locator::Locator {
        self.main_frame_locator_by_js(crate::locator::js_get_by_test_id(&id.into()))
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `alt` attribute value (e.g. image buttons).
    pub fn get_by_alt_text(&self, text: impl Into<String>) -> crate::locator::Locator {
        self.main_frame_locator_by_js(crate::locator::js_get_by_alt_text(&text.into()))
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `title` attribute value.
    pub fn get_by_title(&self, text: impl Into<String>) -> crate::locator::Locator {
        self.main_frame_locator_by_js(crate::locator::js_get_by_title(&text.into()))
    }

    /// Returns a [`Touchscreen`](input::Touchscreen) handle for synthesising touch events.
    pub fn touchscreen(&self) -> input::Touchscreen {
        input::Touchscreen::new(self.handle.clone(), self.session_id.clone())
    }

    /// Set the default timeout for all locator / element actions on this page.
    ///
    /// The timeout applies to `wait_for`, `click`, `fill`, and similar retrying
    /// operations. Default: 30 000 ms.
    pub fn set_default_timeout(&self, ms: u64) {
        self.default_timeout_ms.store(ms, Ordering::Relaxed);
    }

    /// Set the default timeout for navigation operations (`goto`, `reload`, …).
    ///
    /// Default: 30 000 ms.
    pub fn set_default_navigation_timeout(&self, ms: u64) {
        self.default_navigation_timeout_ms.store(ms, Ordering::Relaxed);
    }

    /// Returns the current default action timeout as a [`Duration`](std::time::Duration).
    pub fn default_timeout(&self) -> Duration {
        Duration::from_millis(self.default_timeout_ms.load(Ordering::Relaxed))
    }

    /// Returns the current default navigation timeout as a [`Duration`](std::time::Duration).
    pub fn default_navigation_timeout(&self) -> Duration {
        Duration::from_millis(self.default_navigation_timeout_ms.load(Ordering::Relaxed))
    }

    /// Return a raw CDP session handle for this page.
    ///
    /// The handle allows calling any CDP command directly via [`CdpSession::send`](crate::CdpSession::send).
    pub fn new_cdp_session(&self) -> crate::cdp_session::CdpSession {
        crate::cdp_session::CdpSession::new(self.handle.clone(), self.session_id.clone())
    }

    /// Returns a low-level handle for enabling/disabling raw CDP domains.
    ///
    /// Most users never need this — the high-level API enables the domains it
    /// needs at attach time. Use `page.raw_cdp().enable_dom()` etc. only when
    /// you're issuing raw CDP commands via [`Page::execute`] and need a domain
    /// that isn't already on.
    pub fn raw_cdp(&self) -> raw_cdp::RawCdp<'_> {
        raw_cdp::RawCdp { page: self }
    }

    /// Return (or lazily create) the [`NetworkManager`](crate::network::NetworkManager) for
    /// this page.
    ///
    /// On the first call this enables the `Network` CDP domain so that request
    /// lifecycle events begin flowing.  Subsequent calls return the same manager.
    pub async fn network(&self) -> crate::Result<crate::network::NetworkManager> {
        let existing = self.network_manager.lock().map_err(|_| CdpError::LockPoisoned)?.clone();
        if let Some(mgr) = existing {
            return Ok(mgr);
        }
        let mgr =
            crate::network::NetworkManager::new(self.handle.clone(), self.session_id.current());
        mgr.enable().await?;
        *self.network_manager.lock().map_err(|_| CdpError::LockPoisoned)? = Some(mgr.clone());
        Ok(mgr)
    }

    /// Snapshot of all currently in-flight HTTP requests for this page.
    ///
    /// Enables network tracking on the first call (this starts the `Network`
    /// domain; requests sent before the first call are not included).
    pub async fn requests(
        &self,
    ) -> crate::Result<Vec<std::sync::Arc<crate::network::HttpRequest>>> {
        Ok(self.network().await?.requests_snapshot())
    }

    /// Register a handler that is invoked whenever `locator` becomes visible.
    ///
    /// Spawns a background task that polls the locator every 200 ms. When the
    /// element is found and visible the handler is called. The task is owned
    /// by the page and aborted when the last `Page` clone is dropped or the
    /// target is destroyed — no caller-side handle to manage.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// let cookie_banner = page.locator("#cookie-accept");
    /// page.add_locator_handler(cookie_banner, || async {
    ///     // dismiss the banner — will be retried on every appearance
    /// });
    /// // … run your test; handler is auto-aborted when the page closes …
    /// # Ok(())
    /// # }
    /// ```
    pub fn add_locator_handler<F, Fut>(&self, locator: crate::locator::Locator, handler: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        // Tie the polling loop's lifetime to the page's destroy flag so the
        // task exits when the page is closed, and register on the page so it
        // is aborted with the last `Page` clone.
        let closed = Arc::clone(&self.closed);
        let task = crate::runtime::spawn(async move {
            while !closed.load(Ordering::Acquire) {
                if locator.is_visible().await.unwrap_or(false) {
                    handler().await;
                }
                crate::runtime::sleep(Duration::from_millis(200)).await;
            }
        });
        self.register_listener_task(AbortOnDrop(task));
    }

    /// Return a [`FrameLocator`](crate::frame_locator::FrameLocator) that scopes
    /// queries to the `<iframe>` matched by `selector`.
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// let heading = page.frame_locator("iframe.content").locator("h1");
    /// let text = heading.text_content().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn frame_locator(&self, selector: impl Into<String>) -> crate::frame_locator::FrameLocator {
        let frame_id = self
            .frame_tree
            .read()
            .ok()
            .and_then(|t| t.main_frame().map(|f| f.id.clone()))
            .unwrap_or_else(|| crate::cdp::browser_protocol::page::FrameId::new(""));
        let frame = crate::frame::Frame::new(
            self.handle.clone(),
            self.session_id.clone(),
            frame_id,
            std::sync::Arc::clone(&self.frame_tree),
        )
        .with_page(self.clone());
        crate::frame_locator::FrameLocator::new(frame, selector)
    }

    pub(crate) fn main_frame_locator_by_js(&self, js: String) -> crate::locator::Locator {
        let frame_id = self
            .frame_tree
            .read()
            .ok()
            .and_then(|t| t.main_frame().map(|f| f.id.clone()))
            .unwrap_or_else(|| crate::cdp::browser_protocol::page::FrameId::new(""));
        let frame = crate::frame::Frame::new(
            self.handle.clone(),
            self.session_id.clone(),
            frame_id,
            std::sync::Arc::clone(&self.frame_tree),
        )
        .with_page(self.clone());
        crate::locator::Locator::new_with_js(frame, js)
    }

    /// Attach this page to a context-level route registry.
    ///
    /// Called by [`BrowserContext::new_page`] so that context-wide routes are
    /// applied to pages created through the context.
    pub(crate) fn set_context_route_registry(&mut self, registry: crate::route::RouteRegistry) {
        self.context_route_registry = Some(registry);
    }

    /// Returns the page that opened this one via `window.open` / `target="_blank"`,
    /// if any.  Reads `TargetInfo.opener_id` from the local target tree; returns
    /// `None` when no opener is recorded.
    pub async fn opener(&self) -> crate::Result<Option<Page>> {
        let info = self.handle.target(self.target_id.clone())?;
        match info.opener_id {
            Some(opener_id) => Ok(Some(Page::attach(self.handle.clone(), opener_id).await?)),
            None => Ok(None),
        }
    }

    /// Close this page's target.
    pub async fn close(self) -> crate::Result<()> {
        let _ = self
            .handle
            .execute(cdp_page::CloseParams::default(), Some(self.session_id.current()))
            .await;
        Ok(())
    }
}
