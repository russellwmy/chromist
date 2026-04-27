//! Navigation lifecycle waiters.
//!
//! [`WaitUntil`] names a condition (`Load`, `DOMContentLoaded`, `NetworkIdle`,
//! `NetworkAlmostIdle`, `NoWait`). [`NavigationWaiter`] is pre-armed — the
//! caller subscribes to `Page.lifecycleEvent` *before* issuing the action
//! that triggers navigation, then awaits [`NavigationWaiter::wait`]. This
//! avoids the race where a fast load fires between the navigate command and
//! the subscribe call.
//!
//! For `WaitUntil::Load`, the waiter also accepts the legacy
//! `Page.loadEventFired` event so it works on Chrome builds where
//! fine-grained lifecycle events aren't enabled.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;

use crate::cdp::browser_protocol::page as cdp_page;
use crate::cmd::EventFrame;
use crate::error::CdpError;

/// Navigation wait condition used by `Page::goto` and `NavigationWaiter`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaitUntil {
    /// Wait until `Page.loadEventFired` (the `"load"` lifecycle name).
    #[default]
    Load,
    /// Wait until `"DOMContentLoaded"` lifecycle event.
    DomContentLoaded,
    /// Wait until `"networkIdle"` lifecycle event (0 in-flight requests for 500 ms).
    NetworkIdle,
    /// Wait until `"networkAlmostIdle"` lifecycle event (≤2 in-flight requests for 500 ms).
    NetworkAlmostIdle,
    /// Don't wait — return immediately after the navigation command is sent.
    NoWait,
}

impl std::fmt::Display for WaitUntil {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WaitUntil::Load => f.write_str("load"),
            WaitUntil::DomContentLoaded => f.write_str("domcontentloaded"),
            WaitUntil::NetworkIdle => f.write_str("networkidle"),
            WaitUntil::NetworkAlmostIdle => f.write_str("networkalmostidle"),
            WaitUntil::NoWait => f.write_str("nowait"),
        }
    }
}

impl WaitUntil {
    /// Returns the `Page.lifecycleEvent` name string for this condition,
    /// or `None` for `NoWait`.
    pub fn lifecycle_name(self) -> Option<&'static str> {
        match self {
            WaitUntil::Load => Some("load"),
            WaitUntil::DomContentLoaded => Some("DOMContentLoaded"),
            WaitUntil::NetworkIdle => Some("networkIdle"),
            WaitUntil::NetworkAlmostIdle => Some("networkAlmostIdle"),
            WaitUntil::NoWait => None,
        }
    }
}

/// A pre-armed waiter for a navigation lifecycle condition.
///
/// Create it via `Page::navigation_waiter()` or `Page::wait_for_navigation_with()`
/// *before* triggering navigation, then call `.wait()` after.
#[derive(Debug)]
pub struct NavigationWaiter {
    pub(crate) sub: futures::channel::mpsc::Receiver<Arc<EventFrame>>,
    pub(crate) condition: WaitUntil,
    pub(crate) timeout: Duration,
    /// When set, only `Page.lifecycleEvent` events whose `frameId` matches are
    /// considered.  `None` means accept events for any frame (pre-Phase-3
    /// behaviour; kept as fallback).
    pub(crate) frame_id: Option<cdp_page::FrameId>,
}

impl NavigationWaiter {
    /// Override the default 30-second timeout.
    pub fn with_timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Returns the configured wait condition.
    pub fn condition(&self) -> WaitUntil {
        self.condition
    }

    /// Block until the lifecycle condition is satisfied or the timeout elapses.
    pub async fn wait(mut self) -> crate::Result<()> {
        let condition = self.condition;

        // `NoWait` — caller doesn't want to block.
        let Some(lifecycle_name) = condition.lifecycle_name() else {
            return Ok(());
        };

        // For `Load` we also accept the raw `Page.loadEventFired` event so we
        // work even when lifecycle events aren't enabled on older Chrome builds.
        let accept_load_fired = condition == WaitUntil::Load;

        let target_frame_id = self.frame_id.take();
        crate::runtime::timeout(self.timeout, async move {
            while let Some(frame) = self.sub.next().await {
                if frame.method == "Page.lifecycleEvent" {
                    // When a target frame is set, skip events for other frames.
                    if let Some(ref tid) = target_frame_id {
                        if let Some(fid) = frame.params.get("frameId").and_then(|v| v.as_str()) {
                            if fid != tid.inner() {
                                continue;
                            }
                        }
                    }
                    if let Some(n) = frame.params.get("name").and_then(|v| v.as_str()) {
                        if n == lifecycle_name {
                            return;
                        }
                    }
                } else if accept_load_fired && frame.method == "Page.loadEventFired" {
                    return;
                }
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?;

        Ok(())
    }
}

/// Options for [`Page::goto`](crate::page::Page::goto_with) and
/// [`Frame::goto`](crate::frame::Frame::goto_with).
///
/// Construct with `GotoOptions::new()` and chain builder methods. The default
/// is `WaitUntil::Load` with a 30-second timeout.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct GotoOptions {
    pub(crate) wait_until: WaitUntil,
    pub(crate) timeout: Duration,
    pub(crate) referer: Option<String>,
}

impl Default for GotoOptions {
    fn default() -> Self {
        Self { wait_until: WaitUntil::Load, timeout: Duration::from_secs(30), referer: None }
    }
}

impl GotoOptions {
    /// Construct a new options struct with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the lifecycle condition to wait for.
    pub fn wait_until(mut self, condition: WaitUntil) -> Self {
        self.wait_until = condition;
        self
    }

    /// Set the navigation timeout.
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }

    /// Set the `Referer` header sent with the navigation request.
    pub fn referer(mut self, referer: impl Into<String>) -> Self {
        self.referer = Some(referer.into());
        self
    }
}

/// Options for [`Page::reload_with`](crate::page::Page::reload_with).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ReloadOptions {
    pub(crate) wait_until: WaitUntil,
    pub(crate) timeout: Duration,
}

impl Default for ReloadOptions {
    fn default() -> Self {
        Self { wait_until: WaitUntil::Load, timeout: Duration::from_secs(30) }
    }
}

impl ReloadOptions {
    /// Construct a new options struct with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the lifecycle condition to wait for.
    pub fn wait_until(mut self, condition: WaitUntil) -> Self {
        self.wait_until = condition;
        self
    }

    /// Set the reload timeout.
    pub fn timeout(mut self, dur: Duration) -> Self {
        self.timeout = dur;
        self
    }
}
