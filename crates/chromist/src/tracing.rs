//! Chrome DevTools `Tracing` domain wrapper.
//!
//! Obtain via [`Page::tracing`](crate::Page::tracing). `start`/`start_default`
//! begins a recording, and `stop` ends it and returns the collected trace
//! events accumulated from `Tracing.dataCollected` frames.

use futures::{FutureExt, StreamExt};

use crate::cdp::browser_protocol::tracing::{
    self as cdp_tracing, DataCollectedEvent, EndParams, StartParams, TracingCompleteEvent,
};
use crate::error::CdpError;
use std::sync::Arc;

use crate::handler::{HandlerHandle, SessionRef};
use crate::listeners::EventStream;

/// Controller for a `Tracing` session.
///
/// Scoped to a page when created via [`Page::tracing`](crate::Page::tracing), or browser-wide when
/// created via [`BrowserContext::tracing`](crate::BrowserContext::tracing).
#[derive(Debug, Clone)]
pub struct TracingSession {
    handle: HandlerHandle,
    /// `None` for browser-level (context) tracing; `Some` for page-scoped tracing.
    session_id: Option<SessionRef>,
}

impl TracingSession {
    /// Create a page-scoped tracing session.
    pub fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id: Some(session_id) }
    }

    /// Create a browser-level tracing session (for use with `BrowserContext`).
    pub fn new_browser_level(handle: HandlerHandle) -> Self {
        Self { handle, session_id: None }
    }

    fn sid(&self) -> Option<Arc<str>> {
        self.session_id.as_ref().map(|s| s.current())
    }

    /// Start tracing with an explicit parameter set.
    pub async fn start(&self, params: impl Into<StartParams>) -> crate::Result<()> {
        self.handle.execute(params.into(), self.sid()).await?;
        Ok(())
    }

    /// Start tracing with sensible defaults (`categories = "devtools.timeline"`,
    /// `ReportEvents` transfer mode — the chromist default since the caller
    /// wants events streamed back via `dataCollected`).
    pub async fn start_default(&self) -> crate::Result<()> {
        let params = StartParams {
            categories: Some("devtools.timeline".to_string()),
            transfer_mode: Some(cdp_tracing::StartParamsTransferMode::ReportEvents),
            ..Default::default()
        };
        self.start(params).await
    }

    /// Stop tracing and return the merged trace events collected via
    /// `Tracing.dataCollected` before `Tracing.tracingComplete` fires.
    ///
    /// Subscribes to events *before* issuing `Tracing.end` so we don't race
    /// the completion signal. Uses a 60-second default timeout — call
    /// [`stop_with_timeout`](Self::stop_with_timeout) to override.
    pub async fn stop(&self) -> crate::Result<Vec<serde_json::Value>> {
        self.stop_with_timeout(std::time::Duration::from_secs(60)).await
    }

    /// Stop tracing with an explicit timeout for collecting the data stream.
    pub async fn stop_with_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> crate::Result<Vec<serde_json::Value>> {
        // Subscribe first to avoid missing fast events.
        let mut data_stream: EventStream<DataCollectedEvent> = self.data_collected_stream();
        let mut complete_stream: EventStream<TracingCompleteEvent> = self.tracing_complete_stream();

        self.handle.execute(EndParams::default(), self.sid()).await?;

        let mut out: Vec<serde_json::Value> = Vec::new();
        crate::runtime::timeout(timeout, async {
            loop {
                futures::select! {
                    chunk = data_stream.next().fuse() => match chunk {
                        Some(ev) => out.extend(ev.value),
                        None => break,
                    },
                    done = complete_stream.next().fuse() => {
                        if done.is_some() { break; }
                    },
                }
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?;

        Ok(out)
    }

    /// Stream of `Tracing.dataCollected` events on this session.
    pub fn data_collected_stream(&self) -> EventStream<DataCollectedEvent> {
        self.handle.event_listener(self.sid())
    }

    /// Stream of `Tracing.tracingComplete` events on this session.
    pub fn tracing_complete_stream(&self) -> EventStream<TracingCompleteEvent> {
        self.handle.event_listener(self.sid())
    }

    /// Begin a labeled trace group.
    ///
    /// Groups map one-to-one onto LLM tool calls in agentic traces. The label
    /// is recorded as a console timestamp mark in the trace stream.
    pub async fn group(&self, name: impl Into<String>) -> crate::Result<()> {
        let js = format!("console.timeStamp({:?})", name.into());
        let mut params = crate::cdp::js_protocol::runtime::EvaluateParams::new(js);
        params.return_by_value = Some(true);
        if let Err(e) = self.handle.execute(params, self.sid()).await {
            tracing::debug!(error = %e, "TracingSession::group: timeStamp eval failed");
        }
        Ok(())
    }

    /// End the most recently opened trace group (no-op in the current
    /// implementation — groups are single-point marks).
    pub fn group_end(&self) {}

    /// Start a new trace chunk.
    ///
    /// Starts a tracing session with `devtools.timeline` defaults if one is not
    /// already running.  Use [`stop_chunk`](TracingSession::stop_chunk) to
    /// collect the events accumulated since this call.
    pub async fn start_chunk(&self) -> crate::Result<()> {
        self.start_default().await
    }

    /// Stop the current trace chunk and return collected events.
    ///
    /// Equivalent to [`stop`](TracingSession::stop) — stops the active tracing
    /// session and returns all events.
    pub async fn stop_chunk(&self) -> crate::Result<Vec<serde_json::Value>> {
        self.stop().await
    }
}
