//! High-level event types emitted by a [`Page`](crate::Page).

use std::sync::Arc;

use futures::StreamExt;

use crate::error::CdpError;
use crate::evaluate::EvaluationResult;
use crate::handler::HandlerHandle;

/// Source location of a console message.
#[derive(Debug, Clone)]
pub struct ConsoleLocation {
    /// Script URL where the console call originated.
    pub url: String,
    /// Line number (0-based) of the call site.
    pub line_number: i64,
    /// Column number (0-based) of the call site.
    pub column_number: i64,
}

/// A message emitted by the page's JavaScript console.
///
/// The `kind` mirrors the `Runtime.consoleAPICalled` type field
/// (`"log"`, `"warn"`, `"error"`, `"debug"`, …).
#[derive(Debug, Clone)]
pub struct ConsoleMessage {
    /// The console method that was called (`"log"`, `"warn"`, `"error"`, …).
    pub kind: String,
    /// Human-readable concatenation of all arguments.
    pub text: String,
    /// Raw serialized argument values.
    pub args: Vec<serde_json::Value>,
    /// Source location from the first call frame in the stack trace.
    pub location: Option<ConsoleLocation>,
}

/// A download that was initiated by the page.
///
/// Produced when `Browser.downloadWillBegin` fires.  The `path` field is
/// populated once the download completes (i.e., when the accompanying
/// `Browser.downloadProgress` event with `state == "completed"` arrives).
#[derive(Debug, Clone)]
pub struct Download {
    /// The URL being downloaded.
    pub url: String,
    /// The browser-suggested filename.
    pub suggested_filename: String,
    /// Path on disk once the file is fully written; `None` while in-progress.
    pub path: Option<std::path::PathBuf>,
    /// Browser-assigned GUID for this download (used by [`cancel`](Download::cancel)).
    pub guid: String,
    pub(crate) handle: HandlerHandle,
    pub(crate) target_id: Option<Arc<str>>,
}

impl Download {
    /// Copy the downloaded file to `destination`.
    ///
    /// Returns `Err(CdpError::NotFound)` if the download is not yet complete.
    pub async fn save_as(&self, destination: impl AsRef<std::path::Path>) -> crate::Result<()> {
        let src = self.path.as_ref().ok_or(CdpError::NotFound)?;
        tokio::fs::copy(src, destination.as_ref()).await.map_err(CdpError::Io)?;
        Ok(())
    }

    /// Delete the downloaded file from disk.
    ///
    /// Returns `Err(CdpError::NotFound)` if the download is not yet complete.
    pub async fn delete(&self) -> crate::Result<()> {
        let path = self.path.as_ref().ok_or(CdpError::NotFound)?;
        tokio::fs::remove_file(path).await.map_err(CdpError::Io)?;
        Ok(())
    }

    /// Always returns `None` in the current implementation.
    ///
    /// `Download` instances are only emitted from
    /// [`Events::on_download`](crate::Events::on_download) and
    /// [`Page::expect_download`](crate::Page::expect_download) after a
    /// `state == Completed` progress event, so failure information is never
    /// attached. Do not rely on this method to detect failures — failed
    /// downloads are surfaced via `expect_download` returning
    /// `Err(CdpError::NotFound)` instead.
    #[deprecated(note = "always returns None; failed downloads are reported via expect_download \
                         returning Err(CdpError::NotFound)")]
    pub fn failure(&self) -> Option<&str> {
        None
    }

    /// Cancel an in-progress download.
    ///
    /// Uses `Browser.cancelDownload` with the download's [`guid`](Download::guid).
    pub async fn cancel(&self) -> crate::Result<()> {
        use crate::cdp::browser_protocol::browser as cdp_browser;
        self.handle.execute(cdp_browser::CancelDownloadParams::new(&self.guid), None).await?;
        Ok(())
    }

    /// Returns the [`Page`](crate::page::Page) that initiated this download.
    ///
    /// Requires the download to have been captured via [`Events::on_download`](crate::Events::on_download).
    pub async fn page(&self) -> crate::Result<crate::page::Page> {
        use crate::cdp::browser_protocol::target::TargetId;
        let target_id_str = self.target_id.as_ref().ok_or(CdpError::NotFound)?;
        let target_id = TargetId(target_id_str.to_string());
        crate::page::Page::attach(self.handle.clone(), target_id).await
    }

    /// Open the downloaded file as a streaming reader.
    ///
    /// Returns `Err(CdpError::NotFound)` if the download is not yet complete.
    pub async fn create_read_stream(&self) -> crate::Result<tokio::fs::File> {
        let path = self.path.as_ref().ok_or(CdpError::NotFound)?;
        tokio::fs::File::open(path).await.map_err(CdpError::Io)
    }
}

/// A dedicated Web Worker attached to the page.
///
/// Returned by [`Page::workers`](crate::Page::workers) and emitted via
/// [`Events::on_worker`](crate::Events::on_worker).
#[derive(Clone)]
pub struct Worker {
    /// The URL of the worker script.
    pub url: String,
    pub(crate) handle: HandlerHandle,
    pub(crate) session_id: Arc<str>,
    /// Tasks spawned by [`Worker::on_close`] / [`Worker::on_console`], shared
    /// across `Worker` clones so the spawned listeners are aborted when the
    /// last clone is dropped instead of running until the underlying event
    /// channel closes.
    pub(crate) listener_tasks: Arc<std::sync::Mutex<Vec<crate::AbortOnDrop>>>,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("url", &self.url)
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}

impl Worker {
    /// Evaluate a JavaScript expression in the worker's context.
    pub async fn evaluate(&self, expr: impl Into<String>) -> crate::Result<EvaluationResult> {
        use crate::cdp::js_protocol::runtime as cdp_runtime;
        let mut params = cdp_runtime::EvaluateParams::new(expr.into());
        params.return_by_value = Some(true);
        let inner = self.handle.execute(params, Some(self.session_id.clone())).await?;
        Ok(EvaluationResult { inner })
    }

    /// Register a handler called when this worker is destroyed.
    ///
    /// Watches for `Target.detachedFromTarget` events matching this worker's
    /// session ID and calls `f` once when the worker closes.
    pub fn on_close(&self, f: impl Fn() + Send + Sync + 'static) {
        use crate::cdp::browser_protocol::target as cdp_target;
        let f = Arc::new(f);
        let worker_session = self.session_id.clone();
        let mut stream = self.handle.event_listener::<cdp_target::DetachedFromTargetEvent>(None);
        let task = crate::runtime::spawn(async move {
            while let Some(ev) = stream.next().await {
                if ev.session_id.0.as_str() == worker_session.as_ref() {
                    f();
                    return;
                }
            }
        });
        if let Ok(mut tasks) = self.listener_tasks.lock() {
            tasks.push(crate::AbortOnDrop(task));
        }
    }

    /// Register a handler called for every console message the worker emits.
    pub fn on_console(&self, f: impl Fn(ConsoleMessage) + Send + Sync + 'static) {
        use crate::cdp::js_protocol::runtime as cdp_runtime;
        let f = Arc::new(f);
        let mut stream = self
            .handle
            .event_listener::<cdp_runtime::ConsoleApiCalledEvent>(Some(self.session_id.clone()));
        let task = crate::runtime::spawn(async move {
            while let Some(ev) = stream.next().await {
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
                let location =
                    ev.stack_trace.as_ref().and_then(|st| st.call_frames.first()).map(|cf| {
                        ConsoleLocation {
                            url: cf.url.clone(),
                            line_number: cf.line_number,
                            column_number: cf.column_number,
                        }
                    });
                f(ConsoleMessage { kind: ev.r#type.as_ref().to_string(), text, args, location });
            }
        });
        if let Ok(mut tasks) = self.listener_tasks.lock() {
            tasks.push(crate::AbortOnDrop(task));
        }
    }

    /// Evaluate a JavaScript expression in the worker's context and return a
    /// [`JsHandle`](crate::JsHandle) wrapping the remote object.
    pub async fn evaluate_handle(
        &self,
        expr: impl Into<String>,
    ) -> crate::Result<crate::js_handle::JsHandle> {
        use crate::cdp::js_protocol::runtime as cdp_runtime;
        let session_ref = crate::handler::SessionRef::new(self.session_id.clone());
        let mut params = cdp_runtime::EvaluateParams::new(expr.into());
        params.return_by_value = Some(false);
        let inner = self.handle.execute(params, Some(self.session_id.clone())).await?;
        let remote_object_id = inner.result.object_id.ok_or(CdpError::NotFound)?;
        Ok(crate::js_handle::JsHandle::new(self.handle.clone(), session_ref, remote_object_id))
    }
}
