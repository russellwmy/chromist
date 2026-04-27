//! Request/response correlation for in-flight CDP commands.
//!
//! Each outbound `MethodCall` carries a monotonic [`CallId`]. The
//! [`CommandDispatcher`] maps that id to the caller's `oneshot::Sender` plus
//! its per-request deadline. The handler loop calls [`sweep_timeouts`] every
//! 500 ms; expired senders receive [`CdpError::Timeout`](crate::CdpError::Timeout).
//!
//! Resolved entries are removed from the map immediately, so their deadlines
//! never pollute the sweep. Late responses for expired or unknown ids are
//! silently dropped. On connection teardown [`fail_all`] delivers
//! [`CdpError::ChannelClosed`] to every pending caller so no one hangs.
//!
//! [`sweep_timeouts`]: CommandDispatcher::sweep_timeouts
//! [`fail_all`]: CommandDispatcher::fail_all
//! [`CallId`]: chromist_types::CallId
//! [`CdpError::Timeout`](crate::CdpError::Timeout): crate::error::CdpError::Timeout
//! [`CdpError::ChannelClosed`]: crate::error::CdpError::ChannelClosed

use std::time::{Duration, Instant};

use chromist_types::CallId;
use fnv::FnvHashMap;
use futures::channel::oneshot;
use serde_json::Value;

use crate::error::CdpError;

/// Correlates in-flight CDP commands with their response channels and enforces
/// per-request timeouts.
///
/// Each entry stores the sender and its registration timestamp together, so
/// [`resolve`] removing an entry also discards the deadline — no stale heap
/// entries accumulate on sessions with many fast responses. The registration
/// timestamp enables round-trip latency logging on resolution.
///
/// [`resolve`]: CommandDispatcher::resolve
#[cfg(feature = "_bench")]
pub struct CommandDispatcher {
    /// Active pending calls: id → (sender, registered_at).
    pending: FnvHashMap<CallId, (oneshot::Sender<crate::Result<Value>>, Instant)>,
    request_timeout: Duration,
}

#[cfg(not(feature = "_bench"))]
pub(crate) struct CommandDispatcher {
    /// Active pending calls: id → (sender, registered_at).
    pending: FnvHashMap<CallId, (oneshot::Sender<crate::Result<Value>>, Instant)>,
    request_timeout: Duration,
}

impl CommandDispatcher {
    pub(crate) fn new(request_timeout: Duration) -> Self {
        Self { pending: FnvHashMap::default(), request_timeout }
    }

    /// Register a pending call with its response channel and record the registration time.
    pub(crate) fn register(&mut self, id: CallId, tx: oneshot::Sender<crate::Result<Value>>) {
        self.pending.insert(id, (tx, Instant::now()));
    }

    /// Resolve a pending call with its result (success or CDP error response).
    /// Silently ignores unknown IDs (response arrived after timeout removal).
    pub(crate) fn resolve(&mut self, id: CallId, result: crate::Result<Value>) {
        if let Some((tx, registered_at)) = self.pending.remove(&id) {
            tracing::debug!(
                call_id = %id,
                elapsed_ms = registered_at.elapsed().as_millis(),
                ok = result.is_ok(),
                "CDP command resolved"
            );
            let _ = tx.send(result);
        }
    }

    /// Sweep pending calls and fail any whose deadlines have passed.
    ///
    /// O(n) in the number of *currently active* calls, which is bounded by
    /// the concurrency level — not by historical throughput.
    pub(crate) fn sweep_timeouts(&mut self) {
        let now = Instant::now();
        let expired: Vec<CallId> = self
            .pending
            .iter()
            .filter(|(_, (_, registered_at))| {
                now.saturating_duration_since(*registered_at) >= self.request_timeout
            })
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some((tx, _)) = self.pending.remove(&id) {
                tracing::debug!(call_id = %id, "CDP call timed out");
                let _ = tx.send(Err(CdpError::Timeout));
            }
        }
    }

    /// Fail all pending calls with `CdpError::ChannelClosed` — called on
    /// connection teardown so callers receive an error rather than hanging.
    pub(crate) fn fail_all(&mut self) {
        for (_, (tx, _)) in self.pending.drain() {
            let _ = tx.send(Err(CdpError::ChannelClosed));
        }
    }

    /// Number of currently pending calls (exposed for tests).
    #[cfg(test)]
    fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;
    use serde_json::Value;

    // `oneshot::Receiver::try_recv` returns `Result<Option<T>, Canceled>`.
    // When the sender has already sent, it returns `Ok(Some(value))`.
    fn recv_now<T>(mut rx: oneshot::Receiver<T>) -> T {
        match rx.try_recv() {
            Ok(Some(v)) => v,
            other => panic!("expected Ok(Some(_)), got: {:?}", other.map(|o| o.map(|_| "<val>"))),
        }
    }

    #[test]
    fn test_register_and_resolve_success() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        let (tx, rx) = oneshot::channel();
        disp.register(CallId::new(1), tx);
        disp.resolve(CallId::new(1), Ok(Value::Null));
        let result = recv_now(rx);
        assert!(result.is_ok(), "expected Ok but got: {:?}", result);
    }

    #[test]
    fn test_register_and_resolve_error() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        let (tx, rx) = oneshot::channel();
        disp.register(CallId::new(2), tx);
        disp.resolve(CallId::new(2), Err(CdpError::Timeout));
        let result = recv_now(rx);
        assert!(matches!(result, Err(CdpError::Timeout)));
    }

    #[test]
    fn test_fail_all() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        disp.register(CallId::new(3), tx1);
        disp.register(CallId::new(4), tx2);
        disp.fail_all();
        let r1 = recv_now(rx1);
        let r2 = recv_now(rx2);
        assert!(matches!(r1, Err(CdpError::ChannelClosed)));
        assert!(matches!(r2, Err(CdpError::ChannelClosed)));
    }

    #[test]
    fn test_sweep_timeouts_with_zero_duration() {
        let mut disp = CommandDispatcher::new(Duration::ZERO);
        let (tx, rx) = oneshot::channel();
        disp.register(CallId::new(5), tx);
        disp.sweep_timeouts();
        let result = recv_now(rx);
        assert!(matches!(result, Err(CdpError::Timeout)));
    }

    #[test]
    fn sweep_does_not_timeout_future_deadlines() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(60));
        let (tx, mut rx) = oneshot::channel();
        disp.register(CallId::new(1), tx);
        disp.sweep_timeouts();
        // The call has not yet timed out; the sender must still be pending.
        assert!(matches!(rx.try_recv(), Ok(None)));
        disp.resolve(CallId::new(1), Ok(Value::from(7)));
        let v = recv_now(rx).expect("resolve must deliver");
        assert_eq!(v, Value::from(7));
    }

    #[test]
    fn sweep_mixed_deadlines_only_expires_past() {
        let mut fast = CommandDispatcher::new(Duration::ZERO);
        let (tx_fast, rx_fast) = oneshot::channel();
        fast.register(CallId::new(1), tx_fast);
        fast.sweep_timeouts();
        assert!(matches!(recv_now(rx_fast), Err(CdpError::Timeout)));

        // A fresh dispatcher with a live deadline is unaffected.
        let mut slow = CommandDispatcher::new(Duration::from_secs(60));
        let (tx_slow, mut rx_slow) = oneshot::channel();
        slow.register(CallId::new(2), tx_slow);
        slow.sweep_timeouts();
        assert!(matches!(rx_slow.try_recv(), Ok(None)));
    }

    #[test]
    fn resolve_unknown_id_is_noop() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        // Must not panic when the id was never registered (late response).
        disp.resolve(CallId::new(42), Ok(Value::Null));
    }

    #[test]
    fn resolve_after_receiver_dropped_is_silent() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        let (tx, rx) = oneshot::channel();
        disp.register(CallId::new(9), tx);
        drop(rx);
        // Send result to a dropped receiver — must not panic.
        disp.resolve(CallId::new(9), Ok(Value::Null));
    }

    #[test]
    fn resolve_removes_pending_so_timeout_sweep_is_noop() {
        let mut disp = CommandDispatcher::new(Duration::ZERO);
        let (tx, rx) = oneshot::channel();
        disp.register(CallId::new(1), tx);
        disp.resolve(CallId::new(1), Ok(Value::Null));
        // Even though the deadline has passed, sweep must not double-send.
        disp.sweep_timeouts();
        assert!(recv_now(rx).is_ok());
    }

    #[test]
    fn fail_all_leaves_no_pending() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        let (tx1, _rx1) = oneshot::channel();
        let (tx2, _rx2) = oneshot::channel();
        disp.register(CallId::new(1), tx1);
        disp.register(CallId::new(2), tx2);
        disp.fail_all();
        // After fail_all, a subsequent sweep must not panic or re-process.
        disp.sweep_timeouts();
        assert_eq!(disp.pending_count(), 0);
    }

    #[test]
    fn resolve_removes_entry_so_sweep_never_sees_it() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        let (tx, rx) = oneshot::channel();
        disp.register(CallId::new(77), tx);
        assert_eq!(disp.pending_count(), 1);
        disp.resolve(CallId::new(77), Ok(Value::Null));
        assert_eq!(disp.pending_count(), 0, "resolved entry must be removed immediately");
        recv_now(rx).unwrap();
    }

    #[test]
    fn many_concurrent_registrations_all_resolve() {
        let mut disp = CommandDispatcher::new(Duration::from_secs(30));
        let mut rxs = Vec::new();
        for i in 0..128 {
            let (tx, rx) = oneshot::channel();
            disp.register(CallId::new(i), tx);
            rxs.push((i, rx));
        }
        // Resolve in reverse order.
        for (i, _) in rxs.iter().rev() {
            disp.resolve(CallId::new(*i), Ok(Value::from(*i as u64)));
        }
        for (i, rx) in rxs {
            let v = recv_now(rx).expect("ok");
            assert_eq!(v, Value::from(i as u64));
        }
    }
}
