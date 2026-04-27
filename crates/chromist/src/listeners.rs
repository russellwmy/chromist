//! Typed event streams over raw [`EventFrame`](crate::cmd::EventFrame) channels.
//!
//! [`EventStream<T>`] wraps an `mpsc::Receiver<Arc<EventFrame>>` and implements
//! `Stream<Item = T>`. Each `poll_next` loops over inbound frames, filtering
//! by `T::method_id()` and deserializing `params` to `T`; frames for other
//! methods are silently discarded. This lets callers subscribe once to all
//! events on a session and then view the stream as strongly typed.

use std::sync::Arc;

use chromist_types::{EventMessage, MethodType};
use futures::channel::mpsc;
use futures::Stream;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::cmd::EventFrame;
use crate::handler::HandlerHandle;

/// Marker trait for custom (non-generated) CDP events.
///
/// Implement this alongside [`chromist_types::Method`], [`chromist_types::MethodType`],
/// and [`chromist_types::EventMessage`] on a `#[derive(Deserialize)]` struct to subscribe
/// to any CDP event string not covered by the generated bindings:
///
/// ```ignore
/// #[derive(serde::Deserialize)]
/// struct MyEvent { value: String }
///
/// impl chromist_types::MethodType for MyEvent {
///     fn method_id() -> chromist_types::MethodId { "Domain.myEvent".into() }
/// }
/// impl chromist_types::Method for MyEvent {
///     fn identifier(&self) -> chromist_types::MethodId { Self::method_id() }
/// }
/// impl chromist_types::EventMessage for MyEvent {}
/// impl chromist::CustomEvent for MyEvent {}
///
/// let stream: chromist::EventStream<MyEvent> = page.event_listener();
/// ```
pub trait CustomEvent:
    chromist_types::MethodType + chromist_types::EventMessage + std::any::Any + Send + Sync + 'static
{
}

/// Typed event stream that filters and deserialises an underlying
/// raw [`EventFrame`](crate::EventFrame) channel.
///
/// Yields events of method `T::method_id()`; non-matching frames are
/// skipped silently. Deserialisation failures are logged at `trace!` and
/// the stream advances rather than erroring out.
pub struct EventStream<T> {
    inner: mpsc::Receiver<Arc<EventFrame>>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> std::fmt::Debug for EventStream<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}

impl<T> EventStream<T>
where
    T: EventMessage + MethodType + Send + 'static,
{
    /// Wrap an existing raw event receiver in a typed stream.
    pub fn new(inner: mpsc::Receiver<Arc<EventFrame>>) -> Self {
        Self { inner, _marker: std::marker::PhantomData }
    }
}

impl<T> Unpin for EventStream<T> {}

impl<T> Stream for EventStream<T>
where
    T: EventMessage + MethodType,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let target = T::method_id();
        loop {
            match Pin::new(&mut this.inner).poll_next(cx) {
                Poll::Ready(Some(frame)) => {
                    if frame.method.as_str() == target.as_ref() {
                        match T::deserialize(&frame.params) {
                            Ok(v) => return Poll::Ready(Some(v)),
                            Err(e) => {
                                tracing::debug!(
                                    method = %frame.method,
                                    error = %e,
                                    "EventStream: dropping event after deserialization failure"
                                );
                            }
                        }
                    }
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl HandlerHandle {
    /// Subscribe to a typed event stream, optionally filtered by session.
    pub fn event_listener<T>(&self, session_id: Option<Arc<str>>) -> EventStream<T>
    where
        T: EventMessage + MethodType + Send + 'static,
    {
        let rx = self.subscribe(session_id);
        EventStream::new(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use std::sync::Arc;

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

    fn make_frame(method: &str, params: serde_json::Value) -> Arc<EventFrame> {
        Arc::new(EventFrame { method: method.to_string(), params, session_id: None })
    }

    #[test]
    fn matching_event_is_deserialized() {
        let (mut tx, rx) = futures::channel::mpsc::channel(8);
        let mut stream: EventStream<TestEvent> = EventStream::new(rx);

        tx.try_send(make_frame("Test.event", serde_json::json!({"value": 42}))).unwrap();
        drop(tx);

        let result = futures::executor::block_on(stream.next());
        assert_eq!(result, Some(TestEvent { value: 42 }));
    }

    #[test]
    fn non_matching_method_is_skipped() {
        let (mut tx, rx) = futures::channel::mpsc::channel(8);
        let mut stream: EventStream<TestEvent> = EventStream::new(rx);

        tx.try_send(make_frame("Other.event", serde_json::json!({"value": 99}))).unwrap();
        tx.try_send(make_frame("Test.event", serde_json::json!({"value": 7}))).unwrap();
        drop(tx);

        let result = futures::executor::block_on(stream.next());
        assert_eq!(result, Some(TestEvent { value: 7 }));
    }

    #[test]
    fn deserialization_failure_is_skipped() {
        let (mut tx, rx) = futures::channel::mpsc::channel(8);
        let mut stream: EventStream<TestEvent> = EventStream::new(rx);

        // missing required field "value" — deserialization will fail
        tx.try_send(make_frame("Test.event", serde_json::json!({}))).unwrap();
        tx.try_send(make_frame("Test.event", serde_json::json!({"value": 5}))).unwrap();
        drop(tx);

        let result = futures::executor::block_on(stream.next());
        assert_eq!(result, Some(TestEvent { value: 5 }));
    }

    #[test]
    fn channel_closed_returns_none() {
        let (tx, rx) = futures::channel::mpsc::channel::<Arc<EventFrame>>(8);
        let mut stream: EventStream<TestEvent> = EventStream::new(rx);
        drop(tx);

        let result = futures::executor::block_on(stream.next());
        assert_eq!(result, None);
    }

    #[test]
    fn empty_channel_returns_pending() {
        use std::task::{Context, Poll};

        let (_tx, rx) = futures::channel::mpsc::channel::<Arc<EventFrame>>(8);
        let mut stream: EventStream<TestEvent> = EventStream::new(rx);

        let waker = futures::task::noop_waker_ref();
        let mut cx = Context::from_waker(waker);
        let result = std::pin::Pin::new(&mut stream).poll_next(&mut cx);
        assert!(matches!(result, Poll::Pending));
    }
}
