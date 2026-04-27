//! [`Events`](crate::Events) sub-handle: register fire-and-forget callbacks for page-level
//! events (console messages, page errors, downloads, workers, websockets).
//!
//! Obtained via [`Page::events`]. The handle holds a [`Page`] clone (the
//! cheap `Arc`-wrapped variety) so its methods can reach the page-private
//! event-sink state.
//!
//! Page methods that aren't event registration — `expect_download`,
//! `set_download_path`, `wait_for_event`, `wait_for_load_state`, `workers`
//! — remain on [`Page`] directly: they're query / config / navigation
//! primitives, not handler registration.

use std::sync::Arc;

use futures::StreamExt;

use super::{AbortOnDrop, Page};
use crate::cdp::browser_protocol::browser as cdp_browser;
use crate::cdp::browser_protocol::target as cdp_target;
use crate::error::CdpError;
use crate::events::{ConsoleMessage, Download, Worker};
use crate::lifecycle::WaitUntil;

/// Sub-handle for event-handler registration on a [`Page`]. Obtain via
/// [`Page::events`].
#[derive(Debug, Clone)]
pub struct Events {
    page: Page,
}

impl Events {
    pub(in crate::page) fn new(page: Page) -> Self {
        Self { page }
    }

    /// Register a handler called for every console message the page emits.
    ///
    /// The handler is dispatched from the single per-page background task and
    /// must be `Send + Sync`. Calling this method more than once registers
    /// additional independent handlers — all fire for each event.
    pub fn on_console(&self, f: impl Fn(ConsoleMessage) + Send + Sync + 'static) {
        match self.page.event_sinks.lock() {
            Ok(mut sinks) => sinks.console_handlers.push(Arc::new(f)),
            Err(_) => tracing::warn!("event_sinks lock poisoned; console handler not registered"),
        }
    }

    /// Register a handler called for every uncaught JavaScript exception.
    ///
    /// The handler receives a formatted error string containing the exception
    /// message and (when available) its source location.
    pub fn on_page_error(&self, f: impl Fn(String) + Send + Sync + 'static) {
        match self.page.event_sinks.lock() {
            Ok(mut sinks) => sinks.page_error_handlers.push(Arc::new(f)),
            Err(_) => {
                tracing::warn!("event_sinks lock poisoned; page-error handler not registered")
            }
        }
    }

    /// Register a handler called when a new WebSocket connection is opened.
    ///
    /// The handler receives a [`WebSocket`](crate::websocket::WebSocket)
    /// handle whose [`is_closed`](crate::websocket::WebSocket::is_closed)
    /// flag is set automatically when the connection closes. Requires the
    /// `Network` domain to be enabled.
    pub fn on_websocket(&self, f: impl Fn(crate::websocket::WebSocket) + Send + Sync + 'static) {
        match self.page.event_sinks.lock() {
            Ok(mut sinks) => sinks.websocket_handlers.push(Arc::new(f)),
            Err(_) => tracing::warn!("event_sinks lock poisoned; websocket handler not registered"),
        }
    }

    /// Register a handler called when a download starts.
    ///
    /// Automatically enables download events via `Browser.setDownloadBehavior`.
    /// Uses the path configured by [`Page::set_download_path`], or the system
    /// temp directory if none has been set. All downloads regardless of which
    /// frame initiated them are reported to the handler.
    pub fn on_download(&self, f: impl Fn(Download) + Send + Sync + 'static) {
        let handle = self.page.handle.clone();
        let f = std::sync::Arc::new(f);
        let target_id = self.page.target_id.clone();

        // Enable download events, using the configured path or temp dir.
        let enable_handle = self.page.handle.clone();
        let configured_path = self
            .page
            .download_dir
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_else(std::env::temp_dir);
        let enable_task = crate::runtime::spawn(async move {
            let mut params = cdp_browser::SetDownloadBehaviorParams::new(
                cdp_browser::SetDownloadBehaviorParamsBehavior::AllowAndName,
            );
            params.download_path = Some(configured_path.to_string_lossy().into_owned());
            params.events_enabled = Some(true);
            if let Err(e) = enable_handle.execute(params, None).await {
                tracing::warn!(error = %e, "on_download: SetDownloadBehavior failed");
            }
        });

        // Subscribe to begin + progress events (browser-level, no session).
        let mut begin_stream = handle.event_listener::<cdp_browser::DownloadWillBeginEvent>(None);
        let handle2 = self.page.handle.clone();
        let listener_tasks = Arc::clone(&self.page.listener_tasks);
        let begin_task = crate::runtime::spawn(async move {
            while let Some(ev) = begin_stream.next().await {
                let guid = ev.guid.clone();
                let mut download = Download {
                    url: ev.url.clone(),
                    suggested_filename: ev.suggested_filename.clone(),
                    path: None,
                    guid: guid.clone(),
                    handle: handle2.clone(),
                    target_id: Some(std::sync::Arc::from(target_id.inner().as_str())),
                };

                // Spin up a short-lived task to catch the completion event for
                // this particular guid and fill in the path. The task is
                // registered on the shared `listener_tasks` so it is aborted
                // when the last `Page` clone is dropped, preventing leaks if
                // the page closes while a download is in flight.
                let mut prog_stream =
                    handle2.event_listener::<cdp_browser::DownloadProgressEvent>(None);
                let f2 = f.clone();
                let guid2 = guid.clone();
                let prog_task = crate::runtime::spawn(async move {
                    while let Some(pev) = prog_stream.next().await {
                        if pev.guid != guid2 {
                            continue;
                        }
                        match pev.state {
                            cdp_browser::DownloadProgressEventState::Completed => {
                                if let Some(p) = pev.file_path {
                                    download.path = Some(std::path::PathBuf::from(p));
                                }
                                f2(download);
                                return;
                            }
                            cdp_browser::DownloadProgressEventState::Canceled => {
                                return;
                            }
                            _ => {}
                        }
                    }
                });
                if let Ok(mut tasks) = listener_tasks.lock() {
                    tasks.push(AbortOnDrop(prog_task));
                } else {
                    tracing::warn!(
                        "Page listener_tasks lock poisoned; download progress task not registered"
                    );
                }
            }
        });

        self.page.register_listener_task(AbortOnDrop(enable_task));
        self.page.register_listener_task(AbortOnDrop(begin_task));
    }

    /// Register a handler called whenever a new Web Worker is created.
    pub fn on_worker(&self, f: impl Fn(Worker) + Send + Sync + 'static) {
        let f = std::sync::Arc::new(f);
        let target_id = self.page.target_id.clone();
        let handle = self.page.handle.clone();
        // Subscribe to global Target.attachedToTarget (no session scope).
        let mut stream = self.page.handle.event_listener::<cdp_target::AttachedToTargetEvent>(None);
        let task = crate::runtime::spawn(async move {
            while let Some(ev) = stream.next().await {
                if ev.target_info.r#type != "worker" {
                    continue;
                }
                if ev.target_info.parent_id.as_ref().map(|p| p.inner()) != Some(target_id.inner()) {
                    continue;
                }
                let worker = Worker {
                    url: ev.target_info.url.clone(),
                    handle: handle.clone(),
                    session_id: std::sync::Arc::from(ev.session_id.0.as_str()),
                    listener_tasks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                };
                f(worker);
            }
        });
        self.page.register_listener_task(AbortOnDrop(task));
    }
}

impl Page {
    /// Returns the [`Events`](crate::Events) sub-handle for registering page-event callbacks
    /// (`on_console`, `on_page_error`, `on_websocket`, `on_download`,
    /// `on_worker`).
    pub fn events(&self) -> Events {
        Events::new(self.clone())
    }

    /// Configure the directory where downloaded files are saved.
    ///
    /// Call this before [`Events::on_download`](crate::Events::on_download) to override the default
    /// system temp directory. The setting is applied immediately via
    /// `Browser.setDownloadBehavior` and persists for all subsequent
    /// downloads.
    pub async fn set_download_path(&self, path: impl AsRef<std::path::Path>) -> crate::Result<()> {
        let path = path.as_ref().to_path_buf();
        let mut params = cdp_browser::SetDownloadBehaviorParams::new(
            cdp_browser::SetDownloadBehaviorParamsBehavior::AllowAndName,
        );
        params.download_path = Some(path.to_string_lossy().into_owned());
        params.events_enabled = Some(true);
        self.handle.execute(params, None).await?;
        *self.download_dir.lock().map_err(|_| CdpError::LockPoisoned)? = Some(path);
        Ok(())
    }

    /// Wait for the next download to complete and return it.
    ///
    /// Times out after `timeout` (default: 30 s). Use together with
    /// [`set_download_path`](Page::set_download_path) to control where files
    /// land, or access the `path` field on the returned [`Download`] directly.
    pub async fn expect_download(
        &self,
        timeout: Option<std::time::Duration>,
    ) -> crate::Result<crate::events::Download> {
        let timeout = timeout.unwrap_or(std::time::Duration::from_secs(30));
        let deadline = std::time::Instant::now() + timeout;

        let handle = self.handle.clone();
        let target_id = self.target_id.clone();
        let mut begin_stream = handle.event_listener::<cdp_browser::DownloadWillBeginEvent>(None);

        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let begin = match crate::runtime::timeout(remaining, begin_stream.next()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => return Err(CdpError::ChannelClosed),
            Err(_) => return Err(CdpError::Timeout),
        };

        let guid = begin.guid.clone();
        let mut prog_stream = handle.event_listener::<cdp_browser::DownloadProgressEvent>(None);

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(CdpError::Timeout);
            }
            let pev = match crate::runtime::timeout(remaining, prog_stream.next()).await {
                Ok(Some(ev)) => ev,
                Ok(None) => return Err(CdpError::ChannelClosed),
                Err(_) => return Err(CdpError::Timeout),
            };
            if pev.guid != guid {
                continue;
            }
            match pev.state {
                cdp_browser::DownloadProgressEventState::Completed => {
                    return Ok(crate::events::Download {
                        url: begin.url,
                        suggested_filename: begin.suggested_filename,
                        path: pev.file_path.map(std::path::PathBuf::from),
                        guid,
                        handle: handle.clone(),
                        target_id: Some(std::sync::Arc::from(target_id.inner().as_str())),
                    });
                }
                cdp_browser::DownloadProgressEventState::Canceled => {
                    return Err(CdpError::NotFound);
                }
                _ => continue,
            }
        }
    }

    /// Return all live Web Workers attached to this page.
    pub fn workers(&self) -> crate::Result<Vec<Worker>> {
        let children = self.handle.children_of(&self.target_id)?;
        let workers = children
            .into_iter()
            .filter(|info| info.r#type == "worker")
            .filter_map(|info| {
                let session = self.handle.session_cell(&info.target_id)?;
                Some(Worker {
                    url: info.url.clone(),
                    handle: self.handle.clone(),
                    session_id: session.current(),
                    listener_tasks: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                })
            })
            .collect();
        Ok(workers)
    }

    /// Block until the next CDP event of type `T` is received on this page's
    /// session. Defaults to a 30 s timeout; override with `Some(duration)`.
    pub async fn wait_for_event<T>(&self, timeout: Option<std::time::Duration>) -> crate::Result<T>
    where
        T: chromist_types::EventMessage + chromist_types::MethodType + Send + 'static,
    {
        let timeout = timeout.unwrap_or(std::time::Duration::from_secs(30));
        let mut stream: crate::listeners::EventStream<T> =
            self.handle.event_listener(Some(self.session_id.current()));
        match crate::runtime::timeout(timeout, stream.next()).await {
            Ok(Some(ev)) => Ok(ev),
            Ok(None) => Err(CdpError::ChannelClosed),
            Err(_) => Err(CdpError::Timeout),
        }
    }

    /// Wait until the page reaches `state` (or return immediately if already there).
    ///
    /// For `WaitUntil::Load` and `WaitUntil::DomContentLoaded` the method
    /// first checks `document.readyState` and returns without blocking if the
    /// condition is already satisfied.  `WaitUntil::NoWait` always returns
    /// immediately.
    pub async fn wait_for_load_state(&self, state: WaitUntil) -> crate::Result<()> {
        if state == WaitUntil::NoWait {
            return Ok(());
        }

        if matches!(state, WaitUntil::Load | WaitUntil::DomContentLoaded) {
            let already = self.check_ready_state(state).await.unwrap_or(false);
            if already {
                return Ok(());
            }
        }

        let waiter = crate::lifecycle::NavigationWaiter {
            sub: self.handle.subscribe(Some(self.session_id.current())),
            condition: state,
            timeout: std::time::Duration::from_secs(30),
            frame_id: None,
        };
        waiter.wait().await
    }

    /// Returns `true` when `document.readyState` already satisfies `state`.
    async fn check_ready_state(&self, state: WaitUntil) -> crate::Result<bool> {
        let mut params =
            crate::cdp::js_protocol::runtime::EvaluateParams::new("document.readyState");
        params.return_by_value = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let ready_state =
            resp.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("").to_string();
        let satisfied = match state {
            WaitUntil::Load => ready_state == "complete",
            WaitUntil::DomContentLoaded => {
                ready_state == "interactive" || ready_state == "complete"
            }
            _ => false,
        };
        Ok(satisfied)
    }
}
