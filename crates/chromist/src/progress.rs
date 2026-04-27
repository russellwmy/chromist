//! Deadline-scoped progress token for time-bounded CDP operations.
//!
//! [`Progress`] carries a wall-clock deadline and a shared cancellation flag.
//! Action loops (Phase 4 actionability) call [`Progress::check`] before each
//! retry iteration; if either the deadline has passed or the flag has been set
//! the call returns [`CdpError::Timeout`](crate::CdpError::Timeout) immediately.
//!
//! The cancel flag is set by [`HandlerHandle`](crate::handler::HandlerHandle)
//! when the underlying browser connection closes, so in-flight retry loops fail
//! fast rather than waiting out their full deadline after the browser exits.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::CdpError;

/// A deadline + cancellation token for one CDP action.
#[derive(Debug, Clone)]
pub struct Progress {
    deadline: Instant,
    cancel: Arc<AtomicBool>,
}

impl Progress {
    /// Create a fresh `Progress` with a timeout relative to now and a private cancel flag.
    pub fn new(timeout: Duration) -> Self {
        Self { deadline: Instant::now() + timeout, cancel: Arc::new(AtomicBool::new(false)) }
    }

    /// Create a `Progress` that shares a cancel flag with the calling [`HandlerHandle`](crate::HandlerHandle).
    ///
    /// When the handler closes, the shared flag is set and every live `Progress`
    /// created this way returns `Err(Timeout)` from [`check`](Self::check).
    pub fn with_shared_cancel(timeout: Duration, cancel: Arc<AtomicBool>) -> Self {
        Self { deadline: Instant::now() + timeout, cancel }
    }

    /// Time remaining before the deadline.
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// `true` if the deadline has passed.
    pub fn is_expired(&self) -> bool {
        self.remaining().is_zero()
    }

    /// `true` if the cancel flag has been set.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// Returns `Err(Timeout)` if the deadline has passed or the operation has been cancelled.
    pub fn check(&self) -> crate::Result<()> {
        if self.is_expired() || self.is_cancelled() {
            Err(CdpError::Timeout)
        } else {
            Ok(())
        }
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn fresh_progress_is_alive() {
        let p = Progress::new(Duration::from_secs(30));
        assert!(!p.is_expired());
        assert!(!p.is_cancelled());
        assert!(p.check().is_ok());
    }

    #[test]
    fn expired_deadline_returns_timeout() {
        let p = Progress::new(Duration::ZERO);
        assert!(p.is_expired());
        assert!(matches!(p.check(), Err(CdpError::Timeout)));
    }

    #[test]
    fn cancel_flag_triggers_timeout() {
        let p = Progress::new(Duration::from_secs(30));
        p.cancel.store(true, Ordering::Release);
        assert!(p.is_cancelled());
        assert!(matches!(p.check(), Err(CdpError::Timeout)));
    }

    #[test]
    fn shared_cancel_propagates_to_clone() {
        let cancel = Arc::new(AtomicBool::new(false));
        let p1 = Progress::with_shared_cancel(Duration::from_secs(30), Arc::clone(&cancel));
        let p2 = p1.clone();
        cancel.store(true, Ordering::Release);
        assert!(p1.is_cancelled());
        assert!(p2.is_cancelled());
    }

    #[test]
    fn remaining_decreases_over_time() {
        let p = Progress::new(Duration::from_secs(60));
        let r = p.remaining();
        assert!(r <= Duration::from_secs(60));
        assert!(r > Duration::ZERO);
    }
}
