//! WebSocket observation layer.
//!
//! [`WebSocketEvent`] represents a CDP-observed WebSocket lifecycle event.
//! Frame modification is not possible via public CDP; this module provides
//! observation only.
//!
//! [`WebSocket`] is a live handle to an open WebSocket connection, obtained
//! via [`Events::on_websocket`](crate::Events::on_websocket).

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::cdp::browser_protocol::network::RequestId;

/// The kind of WebSocket event.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum WebSocketEventKind {
    /// A new WebSocket connection was created.
    Created,
    /// A frame was received from the server.
    FrameReceived {
        /// WebSocket opcode (1 = text, 2 = binary).
        opcode: f64,
        /// Frame payload. Text frames are UTF-8; binary frames are base64.
        payload: String,
    },
    /// A frame was sent by the page.
    FrameSent {
        /// WebSocket frame opcode (e.g. `1` = text, `2` = binary).
        opcode: f64,
        /// Frame payload (UTF-8 text or stringified binary).
        payload: String,
    },
    /// The connection was closed.
    Closed,
}

/// A CDP-observed WebSocket lifecycle event.
#[derive(Debug, Clone)]
pub struct WebSocketEvent {
    /// CDP request ID for this WebSocket connection.
    pub request_id: RequestId,
    /// WebSocket URL.
    pub url: String,
    /// The specific event kind.
    pub kind: WebSocketEventKind,
}

/// A live WebSocket connection handle.
///
/// Obtained via [`Events::on_websocket`](crate::Events::on_websocket). Tracks
/// close state for the connection identified by `request_id`.
pub struct WebSocket {
    /// The WebSocket URL.
    pub url: String,
    /// The CDP request ID for this connection.
    pub request_id: RequestId,
    is_closed: Arc<AtomicBool>,
}

impl std::fmt::Debug for WebSocket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocket")
            .field("url", &self.url)
            .field("is_closed", &self.is_closed.load(Ordering::Relaxed))
            .finish()
    }
}

impl Clone for WebSocket {
    fn clone(&self) -> Self {
        Self {
            url: self.url.clone(),
            request_id: self.request_id.clone(),
            is_closed: Arc::clone(&self.is_closed),
        }
    }
}

impl WebSocket {
    pub(crate) fn new(url: String, request_id: RequestId, is_closed: Arc<AtomicBool>) -> Self {
        Self { url, request_id, is_closed }
    }

    /// Returns `true` if the WebSocket connection has been closed.
    pub fn is_closed(&self) -> bool {
        self.is_closed.load(Ordering::Relaxed)
    }
}
