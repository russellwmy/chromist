//! Fan-out of inbound CDP events to registered subscribers.
//!
//! Subscribers are bounded `mpsc::Sender<Arc<EventFrame>>` channels grouped
//! into two pools: *global* (receives every event) and *per-session* (only
//! events tagged with a matching `session_id`). Both pools are consulted on
//! each [`publish`](EventBus::publish) call.
//!
//! Backpressure policy: if a subscriber's channel is full, the *event* is
//! dropped but the *subscriber* is retained — so a slow consumer loses data
//! but doesn't get silently disconnected. If the receiver has been dropped,
//! the sender is pruned on the next publish.
//!
//! Event frames are wrapped in `Arc` so fan-out to N subscribers is a cheap
//! refcount bump rather than a clone of the payload.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::channel::mpsc;

use crate::cmd::{EventFrame, EVENT_CHANNEL_CAP};

/// Manages event fan-out to all registered subscribers.
///
/// Global subscribers receive every event regardless of session.
/// Session subscribers only receive events tagged with their session ID.
#[cfg_attr(feature = "_bench", doc(hidden))]
#[cfg(feature = "_bench")]
pub struct EventBus {
    global: Vec<mpsc::Sender<Arc<EventFrame>>>,
    sessions: HashMap<Arc<str>, Vec<mpsc::Sender<Arc<EventFrame>>>>,
    /// Cumulative count of events dropped because a subscriber's bounded
    /// channel was full. Subscribers are retained — only the event is lost.
    /// Useful for observability (e.g. detecting a slow consumer).
    dropped: Arc<AtomicU64>,
}

#[cfg(not(feature = "_bench"))]
pub(crate) struct EventBus {
    global: Vec<mpsc::Sender<Arc<EventFrame>>>,
    sessions: HashMap<Arc<str>, Vec<mpsc::Sender<Arc<EventFrame>>>>,
    /// Cumulative count of events dropped because a subscriber's bounded
    /// channel was full. Subscribers are retained — only the event is lost.
    pub(crate) dropped: Arc<AtomicU64>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub(crate) fn new() -> Self {
        Self { global: Vec::new(), sessions: HashMap::new(), dropped: Arc::new(AtomicU64::new(0)) }
    }

    /// Total number of events dropped since the bus was created because a
    /// subscriber's channel was full. A non-zero, monotonically increasing
    /// value is a strong signal that a consumer is too slow.
    #[allow(dead_code)]
    pub(crate) fn dropped_event_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Register a new subscriber. Returns a bounded receiver.
    ///
    /// This is a convenience wrapper around [`Self::subscribe_with_sender`] for
    /// callers that want the bus to create the channel.  Currently the handler
    /// creates channels on the caller side and calls `subscribe_with_sender`
    /// directly, but this method is retained for future use and testing.
    #[allow(dead_code)]
    pub(crate) fn subscribe(
        &mut self,
        session_id: Option<Arc<str>>,
    ) -> mpsc::Receiver<Arc<EventFrame>> {
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAP);
        self.subscribe_with_sender(session_id, tx);
        rx
    }

    /// Add an existing sender to the bus (used when the caller already holds the receiver).
    pub(crate) fn subscribe_with_sender(
        &mut self,
        session_id: Option<Arc<str>>,
        tx: mpsc::Sender<Arc<EventFrame>>,
    ) {
        match session_id {
            None => self.global.push(tx),
            Some(sid) => self.sessions.entry(sid).or_default().push(tx),
        }
    }

    /// Fan-out an event to all matching subscribers.
    ///
    /// Backpressure policy: if a channel is full the event is dropped for that
    /// subscriber but the subscriber itself is kept.  Disconnected subscribers
    /// are pruned immediately.  Empty session vecs are removed after fan-out.
    pub(crate) fn publish(&mut self, frame: Arc<EventFrame>) {
        Self::fan_out(&mut self.global, &frame, &self.dropped);

        if let Some(sid) = &frame.session_id {
            if let Some(subs) = self.sessions.get_mut(sid) {
                Self::fan_out(subs, &frame, &self.dropped);
                if subs.is_empty() {
                    self.sessions.remove(sid);
                }
            }
        }
    }

    /// Drop all subscribers — called when the underlying connection closes.
    pub(crate) fn close_all(&mut self) {
        self.global.clear();
        self.sessions.clear();
    }

    fn fan_out(
        subs: &mut Vec<mpsc::Sender<Arc<EventFrame>>>,
        frame: &Arc<EventFrame>,
        dropped: &AtomicU64,
    ) {
        subs.retain_mut(|tx| {
            match tx.try_send(Arc::clone(frame)) {
                Ok(()) => true,
                // Channel full: drop this event but keep the subscriber. Bump
                // the counter so callers polling `dropped_event_count` can
                // detect the slow consumer.
                Err(e) if e.is_full() => {
                    let prev = dropped.fetch_add(1, Ordering::Relaxed);
                    // Log at debug for the first drop and every 1024th drop
                    // thereafter — enough to surface a stuck consumer without
                    // flooding the logs at high event rates.
                    if prev == 0 || prev.is_multiple_of(1024) {
                        tracing::debug!(
                            method = %frame.method,
                            total_dropped = prev + 1,
                            "event dropped: subscriber channel full"
                        );
                    }
                    true
                }
                // Receiver gone: prune the sender.
                Err(_) => false,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::EventFrame;

    fn make_frame(method: &str, session_id: Option<Arc<str>>) -> Arc<EventFrame> {
        Arc::new(EventFrame {
            method: method.to_string(),
            params: serde_json::Value::Null,
            session_id,
        })
    }

    #[test]
    fn test_global_subscriber_receives_event() {
        let mut bus = EventBus::new();
        let mut rx = bus.subscribe(None);
        let frame = make_frame("Page.loadEventFired", None);
        bus.publish(Arc::clone(&frame));
        let received = rx.try_recv().expect("expected a frame");
        assert_eq!(received.method, "Page.loadEventFired");
    }

    #[test]
    fn test_session_subscriber_receives_matching() {
        let mut bus = EventBus::new();
        let sid: Arc<str> = Arc::from("abc");
        let mut rx = bus.subscribe(Some(Arc::clone(&sid)));
        let frame = make_frame("Network.requestWillBeSent", Some(Arc::clone(&sid)));
        bus.publish(Arc::clone(&frame));
        let received = rx.try_recv().expect("expected a frame");
        assert_eq!(received.method, "Network.requestWillBeSent");
    }

    #[test]
    fn test_session_subscriber_skips_other_session() {
        let mut bus = EventBus::new();
        let sid_abc: Arc<str> = Arc::from("abc");
        let sid_xyz: Arc<str> = Arc::from("xyz");
        let mut rx = bus.subscribe(Some(Arc::clone(&sid_abc)));
        let frame = make_frame("Network.requestWillBeSent", Some(Arc::clone(&sid_xyz)));
        bus.publish(frame);
        // rx should be empty — event was for a different session
        assert!(rx.try_recv().is_err(), "should not receive event for different session");
    }

    #[test]
    fn test_pruning_disconnected_subscriber() {
        let mut bus = EventBus::new();
        // Subscribe then immediately drop the receiver.
        let rx = bus.subscribe(None);
        drop(rx);
        assert_eq!(bus.global.len(), 1, "sender still present before publish");
        let frame = make_frame("Page.loadEventFired", None);
        bus.publish(frame);
        assert_eq!(bus.global.len(), 0, "disconnected sender should be pruned after publish");
    }

    #[test]
    fn multi_subscribers_global_fanout() {
        let mut bus = EventBus::new();
        let mut a = bus.subscribe(None);
        let mut b = bus.subscribe(None);
        let mut c = bus.subscribe(None);
        let frame = make_frame("Page.loadEventFired", None);
        bus.publish(Arc::clone(&frame));
        assert_eq!(a.try_recv().expect("a").method, "Page.loadEventFired");
        assert_eq!(b.try_recv().expect("b").method, "Page.loadEventFired");
        assert_eq!(c.try_recv().expect("c").method, "Page.loadEventFired");
    }

    #[test]
    fn multiple_session_ids_are_isolated() {
        let mut bus = EventBus::new();
        let sid_a: Arc<str> = Arc::from("a");
        let sid_b: Arc<str> = Arc::from("b");
        let mut rx_a = bus.subscribe(Some(Arc::clone(&sid_a)));
        let mut rx_b = bus.subscribe(Some(Arc::clone(&sid_b)));
        bus.publish(make_frame("X.y", Some(Arc::clone(&sid_a))));
        assert!(rx_a.try_recv().is_ok());
        assert!(rx_b.try_recv().is_err());
        bus.publish(make_frame("X.y", Some(Arc::clone(&sid_b))));
        assert!(rx_a.try_recv().is_err());
        assert!(rx_b.try_recv().is_ok());
    }

    #[test]
    fn global_subscriber_also_gets_session_events() {
        let mut bus = EventBus::new();
        let sid: Arc<str> = Arc::from("s");
        let mut rx_global = bus.subscribe(None);
        let mut rx_session = bus.subscribe(Some(Arc::clone(&sid)));
        bus.publish(make_frame("E", Some(Arc::clone(&sid))));
        assert_eq!(rx_global.try_recv().expect("global").method, "E");
        assert_eq!(rx_session.try_recv().expect("session").method, "E");
    }

    #[test]
    fn full_channel_drops_event_but_retains_subscriber() {
        use futures::channel::mpsc;
        let mut bus = EventBus::new();
        // Tiny bounded channel so we can overflow it.
        let (tx, mut rx) = mpsc::channel::<Arc<EventFrame>>(1);
        bus.subscribe_with_sender(None, tx);
        bus.publish(make_frame("one", None));
        bus.publish(make_frame("two", None));
        bus.publish(make_frame("three", None));
        // Only the first event fits; later ones are dropped silently.
        let first = rx.try_recv().expect("first");
        assert_eq!(first.method, "one");
        // Receiver only buffered one slot; subscriber is still registered.
        assert_eq!(bus.global.len(), 1, "subscriber must not be pruned on full");
    }

    #[test]
    fn publish_without_session_id_skips_session_lookup() {
        let mut bus = EventBus::new();
        let sid: Arc<str> = Arc::from("orphan");
        let mut rx = bus.subscribe(Some(Arc::clone(&sid)));
        // No session_id on the frame — session subscriber must not fire.
        bus.publish(make_frame("E", None));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_close_all() {
        let mut bus = EventBus::new();
        let sid: Arc<str> = Arc::from("s1");
        let mut rx_global = bus.subscribe(None);
        let mut rx_session = bus.subscribe(Some(Arc::clone(&sid)));
        bus.close_all();
        // After close_all, senders are dropped so receivers observe channel termination.
        // Publishing should not crash even with no subscribers.
        let frame = make_frame("Page.loadEventFired", Some(Arc::clone(&sid)));
        bus.publish(frame);
        // Receivers should see the channel as disconnected (try_recv returns Err).
        assert!(rx_global.try_recv().is_err(), "global rx should be disconnected");
        assert!(rx_session.try_recv().is_err(), "session rx should be disconnected");
    }
}
