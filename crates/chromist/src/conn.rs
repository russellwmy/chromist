//! Transport abstraction over WebSocket and OS-pipe CDP connections.
//!
//! [`AnyConnection`] is an enum — not a trait object — so that the hot send
//! and receive paths avoid dynamic dispatch. Exactly one variant is live per
//! browser session: `Ws` (WebSocket, the default and only option on non-Unix
//! targets) or `Pipe` (OS pipes, Unix-only; avoids opening a TCP port).
//!
//! The [`Connection`] newtype wraps the underlying WebSocket stream with a
//! small outbound buffer (`VecDeque`) so the handler can pipeline sends
//! without blocking on the socket. Pipe transport lives in [`crate::pipe_conn`].

use std::collections::VecDeque;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_tungstenite::tungstenite::protocol::WebSocketConfig;
use async_tungstenite::tungstenite::Message as WsMessage;
use chromist_types::{CallId, CdpJsonEventMessage, EventMessage, Message, MethodCall, MethodId};
use futures::{Sink, Stream};
use serde_json::Value;

use crate::error::CdpError;

type ConnectStream = async_tungstenite::tokio::ConnectStream;

type Ws = async_tungstenite::WebSocketStream<ConnectStream>;

/// WebSocket-backed CDP connection.
///
/// Wraps an `async_tungstenite` stream with a small outbound queue so the
/// handler can pipeline sends without blocking on the socket. Used as the
/// `Ws` arm of [`AnyConnection`].
pub struct Connection<T: EventMessage> {
    ws: Ws,
    pending_send: VecDeque<MethodCall>,
    next_id: usize,
    needs_flush: bool,
    _marker: PhantomData<T>,
}

impl<T: EventMessage> std::fmt::Debug for Connection<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("pending_send", &self.pending_send.len())
            .field("next_id", &self.next_id)
            .field("needs_flush", &self.needs_flush)
            .finish_non_exhaustive()
    }
}

impl<T: EventMessage> Connection<T> {
    /// Connect to `url` (typically `ws://localhost:PORT/devtools/browser/...`)
    /// and return a ready-to-use [`Connection`].
    pub async fn connect(url: &str) -> crate::Result<Self> {
        // CDP responses can far exceed tungstenite's default 64 MiB message
        // and 16 MiB frame caps — Page.captureScreenshot of long pages,
        // Page.captureSnapshot (MHTML), Network.getResponseBody for large
        // assets, HeapProfiler.takeHeapSnapshot, and full tracing buffer
        // dumps all hit Capacity errors otherwise. Lift both limits.
        let config = WebSocketConfig::default().max_message_size(None).max_frame_size(None);
        let (ws, _) =
            async_tungstenite::tokio::connect_async_with_config(url, Some(config)).await?;
        Ok(Self {
            ws,
            pending_send: VecDeque::new(),
            next_id: 1,
            needs_flush: false,
            _marker: PhantomData,
        })
    }

    /// Enqueue a CDP command for transmission. Returns the assigned
    /// [`CallId`] which the dispatcher will use to correlate the response.
    pub fn submit_command(
        &mut self,
        method: MethodId,
        session_id: Option<Arc<str>>,
        params: Value,
    ) -> CallId {
        let id = CallId::new(self.next_id);
        self.next_id += 1;
        self.pending_send.push_back(MethodCall { id, method, session_id, params });
        id
    }
}

impl<T: EventMessage> Unpin for Connection<T> {}

/// A CDP connection that is either WebSocket-based or pipe-based.
///
/// Using an enum here avoids boxing and dynamic dispatch while still letting
/// `Handler` accept both transport variants through a
/// single field type. Variants are intentionally unboxed — only one is ever
/// held per connection, so the size disparity is paid at most once.
#[allow(clippy::large_enum_variant)]
#[non_exhaustive]
#[derive(Debug)]
pub enum AnyConnection {
    /// Standard WebSocket transport (`--remote-debugging-port`).
    Ws(Connection<CdpJsonEventMessage>),
    /// OS-pipe transport (`--remote-debugging-pipe`), unix only.
    #[cfg(unix)]
    Pipe(crate::pipe_conn::PipeConnection<CdpJsonEventMessage>),
    /// In-process mock transport, used by handler unit tests.
    #[cfg(test)]
    Mock(mock::MockConnection),
}

impl AnyConnection {
    /// Enqueue a CDP command on whichever underlying transport is active.
    pub fn submit_command(
        &mut self,
        method: MethodId,
        session_id: Option<Arc<str>>,
        params: Value,
    ) -> CallId {
        match self {
            AnyConnection::Ws(c) => c.submit_command(method, session_id, params),
            #[cfg(unix)]
            AnyConnection::Pipe(c) => c.submit_command(method, session_id, params),
            #[cfg(test)]
            AnyConnection::Mock(c) => c.submit_command(method, session_id, params),
        }
    }
}

impl Unpin for AnyConnection {}

impl Stream for AnyConnection {
    type Item = crate::Result<Message<CdpJsonEventMessage>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            AnyConnection::Ws(c) => Pin::new(c).poll_next(cx),
            #[cfg(unix)]
            AnyConnection::Pipe(c) => Pin::new(c).poll_next(cx),
            #[cfg(test)]
            AnyConnection::Mock(c) => Pin::new(c).poll_next(cx),
        }
    }
}

impl<T: EventMessage> Stream for Connection<T> {
    type Item = crate::Result<Message<T>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        // Drain outbound commands first so that a blocked read never starves
        // the send path — CDP requires commands to be sent promptly.
        while !this.pending_send.is_empty() {
            match Pin::new(&mut this.ws).poll_ready(cx) {
                Poll::Ready(Ok(())) => {
                    let Some(call) = this.pending_send.pop_front() else { break };
                    let text = match serde_json::to_string(&call) {
                        Ok(t) => t,
                        Err(e) => return Poll::Ready(Some(Err(e.into()))),
                    };
                    if let Err(e) = Pin::new(&mut this.ws).start_send(WsMessage::Text(text.into()))
                    {
                        return Poll::Ready(Some(Err(e.into())));
                    }
                    this.needs_flush = true;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                Poll::Pending => break,
            }
        }

        if this.needs_flush {
            match Pin::new(&mut this.ws).poll_flush(cx) {
                Poll::Ready(Ok(())) => this.needs_flush = false,
                Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                Poll::Pending => {}
            }
        }

        loop {
            match futures::ready!(Pin::new(&mut this.ws).poll_next(cx)) {
                Some(Ok(WsMessage::Text(t))) => {
                    return Poll::Ready(Some(
                        serde_json::from_str::<Message<T>>(t.as_str()).map_err(CdpError::from),
                    ));
                }
                Some(Ok(WsMessage::Binary(_)))
                | Some(Ok(WsMessage::Ping(_)))
                | Some(Ok(WsMessage::Pong(_)))
                | Some(Ok(WsMessage::Frame(_))) => continue,
                Some(Ok(WsMessage::Close(_))) => return Poll::Ready(None),
                Some(Err(e)) => return Poll::Ready(Some(Err(e.into()))),
                None => return Poll::Ready(None),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! In-process mock transport for handler unit tests.
    //!
    //! [`MockConnection`] looks like an [`AnyConnection`] from the handler's
    //! perspective: it implements `Stream` and exposes `submit_command`. A
    //! paired [`MockHandle`] lets a test inject inbound messages into the
    //! handler's read loop and observe the commands it sends back.

    use super::*;
    use chromist_types::MethodCall;
    use futures::channel::mpsc;

    /// Mock transport. Pair via [`MockConnection::pair`].
    #[derive(Debug)]
    pub struct MockConnection {
        inbound: mpsc::UnboundedReceiver<crate::Result<Message<CdpJsonEventMessage>>>,
        outbound: mpsc::UnboundedSender<MethodCall>,
        next_id: usize,
    }

    /// Test-side handle for a [`MockConnection`]. Sending on `inbound`
    /// delivers a message to the handler; dropping it simulates a
    /// disconnect. `outbound` receives every command the handler submits.
    #[derive(Debug)]
    pub struct MockHandle {
        pub inbound: mpsc::UnboundedSender<crate::Result<Message<CdpJsonEventMessage>>>,
        pub outbound: mpsc::UnboundedReceiver<MethodCall>,
    }

    impl MockConnection {
        pub fn pair() -> (Self, MockHandle) {
            let (in_tx, in_rx) = mpsc::unbounded();
            let (out_tx, out_rx) = mpsc::unbounded();
            let conn = MockConnection { inbound: in_rx, outbound: out_tx, next_id: 1 };
            let handle = MockHandle { inbound: in_tx, outbound: out_rx };
            (conn, handle)
        }

        pub fn submit_command(
            &mut self,
            method: MethodId,
            session_id: Option<Arc<str>>,
            params: Value,
        ) -> CallId {
            let id = CallId::new(self.next_id);
            self.next_id += 1;
            let _ = self.outbound.unbounded_send(MethodCall { id, method, session_id, params });
            id
        }
    }

    impl Stream for MockConnection {
        type Item = crate::Result<Message<CdpJsonEventMessage>>;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Pin::new(&mut self.get_mut().inbound).poll_next(cx)
        }
    }
}
