//! Message types exchanged between [`HandlerHandle`](crate::handler::HandlerHandle)
//! and the handler task.
//!
//! Callers send [`HandlerMessage`] variants (command, subscribe, shutdown) and
//! receive command results via a `oneshot::Sender` or event streams via an
//! `mpsc::Sender<Arc<EventFrame>>`. Event fan-out channels are bounded at
//! [`EVENT_CHANNEL_CAP`] — full channels drop events, not subscribers.

use std::sync::Arc;

use chromist_types::MethodId;
use futures::channel::{mpsc, oneshot};
use serde_json::Value;

pub(crate) const EVENT_CHANNEL_CAP: usize = 512;

#[derive(Debug)]
#[non_exhaustive]
pub(crate) enum HandlerMessage {
    Command {
        method: MethodId,
        params: Value,
        session_id: Option<Arc<str>>,
        tx: oneshot::Sender<crate::Result<Value>>,
    },
    Subscribe {
        session_id: Option<Arc<str>>,
        tx: mpsc::Sender<Arc<EventFrame>>,
    },
    Shutdown,
}

/// One CDP event delivered to a subscriber.
///
/// Held in an `Arc<EventFrame>` so fan-out is zero-copy: every subscriber
/// receives the same allocation. Use [`EventFrame::try_decode`] or
/// [`EventFrame::is`] for type-safe handling.
#[derive(Debug)]
pub struct EventFrame {
    /// CDP method name as it appears on the wire (e.g. `"Page.loadEventFired"`).
    pub method: String,
    /// Raw event payload — already-parsed JSON, ready to deserialize into
    /// the matching CDP event type.
    pub params: Value,
    /// Session ID for events scoped to a target session, `None` for
    /// browser-level events.
    pub session_id: Option<Arc<str>>,
}

impl EventFrame {
    /// Attempt to decode this event frame as the typed CDP event `T`.
    ///
    /// Returns `Some(T)` only when the frame's method name matches `T`'s CDP
    /// identifier *and* the params deserialize successfully. Returns `None`
    /// for any other event method, and propagates a [`CdpError::Json`](crate::CdpError::Json) when
    /// the method matches but params fail to deserialize.
    ///
    /// This is the type-safe, IDE-discoverable alternative to the
    /// [`consume_event!`](crate::consume_event) macro:
    ///
    /// ```ignore
    /// use chromist::cdp::browser_protocol::page::LoadEventFiredEvent;
    ///
    /// while let Some(frame) = sub.next().await {
    ///     if let Some(ev) = frame.try_decode::<LoadEventFiredEvent>()? {
    ///         println!("page loaded at {}", ev.timestamp);
    ///     }
    /// }
    /// ```
    pub fn try_decode<T>(&self) -> crate::Result<Option<T>>
    where
        T: chromist_types::MethodType + serde::de::DeserializeOwned,
    {
        if self.method.as_str() != T::method_id().as_ref() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(self.params.clone())?))
    }

    /// Returns `true` when this frame's method matches `T`'s CDP identifier.
    ///
    /// Cheaper than [`try_decode`](Self::try_decode) when you only need to
    /// branch on the event kind without consuming the payload.
    pub fn is<T: chromist_types::MethodType>(&self) -> bool {
        self.method.as_str() == T::method_id().as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp::browser_protocol::page::LoadEventFiredEvent;
    use serde_json::json;

    fn frame(method: &str, params: Value) -> EventFrame {
        EventFrame { method: method.to_string(), params, session_id: None }
    }

    #[test]
    fn try_decode_matches_method_and_payload() {
        let f = frame("Page.loadEventFired", json!({ "timestamp": 1.5 }));
        let ev: LoadEventFiredEvent = f.try_decode().expect("decode ok").expect("matched method");
        assert_eq!(ev.timestamp.0, 1.5);
    }

    #[test]
    fn try_decode_returns_none_on_mismatched_method() {
        let f = frame("Network.requestWillBeSent", json!({}));
        let ev = f.try_decode::<LoadEventFiredEvent>().expect("ok");
        assert!(ev.is_none(), "non-matching method must return None, not Err");
    }

    #[test]
    fn is_returns_true_for_matching_type() {
        let f = frame("Page.loadEventFired", json!({ "timestamp": 0.0 }));
        assert!(f.is::<LoadEventFiredEvent>());
    }

    #[test]
    fn is_returns_false_for_other_type() {
        let f = frame("Network.requestWillBeSent", json!({}));
        assert!(!f.is::<LoadEventFiredEvent>());
    }
}
