//! Pipe-based CDP connection using fd 3 (write) and fd 4 (read).
//!
//! Chrome supports `--remote-debugging-pipe` which creates two OS pipes:
//! - fd 3 in Chrome: Chrome reads CDP commands from here (parent writes)
//! - fd 4 in Chrome: Chrome writes CDP responses here (parent reads)
//!
//! Messages are null-byte (`\0`) delimited JSON.

#![cfg(unix)]

use std::collections::VecDeque;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use chromist_types::{CallId, EventMessage, Message, MethodCall, MethodId};
use futures::Stream;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::CdpError;

/// A CDP connection that communicates over a pair of OS pipes rather than a
/// WebSocket. Chrome must be launched with `--remote-debugging-pipe`.
#[derive(Debug)]
pub struct PipeConnection<T: EventMessage> {
    /// Parent reads Chrome responses from here (Chrome writes to fd 4).
    reader: tokio::fs::File,
    /// Parent writes CDP commands here (Chrome reads from fd 3).
    writer: tokio::fs::File,
    /// Commands waiting to be flushed to the writer.
    pending_send: VecDeque<MethodCall>,
    /// Monotonically increasing call-ID counter.
    next_id: usize,
    /// Bytes received from Chrome, buffered until a `\0` delimiter is found.
    read_buf: Vec<u8>,
    /// The serialized bytes of the command currently being written (JSON + `\0`).
    write_buf: Vec<u8>,
    /// How many bytes of `write_buf` have already been written.
    write_offset: usize,
    _marker: PhantomData<T>,
}

impl<T: EventMessage> PipeConnection<T> {
    /// Create a new `PipeConnection`.
    ///
    /// `writer` must be the write-end of the pipe that Chrome reads commands
    /// from (Chrome's fd 3), and `reader` must be the read-end of the pipe that
    /// Chrome writes responses to (Chrome's fd 4).
    pub fn new(writer: tokio::fs::File, reader: tokio::fs::File) -> Self {
        Self {
            reader,
            writer,
            pending_send: VecDeque::new(),
            next_id: 1,
            read_buf: Vec::with_capacity(4096),
            write_buf: Vec::new(),
            write_offset: 0,
            _marker: PhantomData,
        }
    }

    /// Enqueue a CDP command for sending. Returns the [`CallId`] that will be
    /// echoed back in Chrome's response.
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

// `tokio::fs::File` is `Unpin`, so the whole struct is too.
impl<T: EventMessage> Unpin for PipeConnection<T> {}

impl<T: EventMessage> Stream for PipeConnection<T> {
    type Item = crate::Result<Message<T>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // ── 1. Flush pending write buffer ──────────────────────────────────────
        //
        // If there are bytes left in `write_buf` from a previous partial write,
        // keep draining them before loading the next message.
        loop {
            if this.write_buf.is_empty() {
                // Load the next pending command into write_buf.
                let Some(call) = this.pending_send.pop_front() else {
                    break; // Nothing more to send right now.
                };
                let mut serialized = match serde_json::to_vec(&call) {
                    Ok(v) => v,
                    Err(e) => return Poll::Ready(Some(Err(e.into()))),
                };
                serialized.push(b'\0'); // null-byte delimiter
                this.write_buf = serialized;
                this.write_offset = 0;
            }

            let remaining = &this.write_buf[this.write_offset..];
            match Pin::new(&mut this.writer).poll_write(cx, remaining) {
                Poll::Ready(Ok(0)) => {
                    // Zero-byte write on a pipe means the other end is closed.
                    return Poll::Ready(None);
                }
                Poll::Ready(Ok(n)) => {
                    this.write_offset += n;
                    if this.write_offset >= this.write_buf.len() {
                        // Entire message written; clear the buffer.
                        this.write_buf.clear();
                        this.write_offset = 0;
                    }
                    // Continue the loop: either drain more bytes or load the
                    // next queued command.
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(CdpError::Io(e)))),
                Poll::Pending => break, // Writer not ready; move on to reading.
            }
        }

        // ── 2. Read incoming bytes and emit complete messages ──────────────────
        //
        // Continuously read from the pipe into `read_buf`. When we find a `\0`
        // we have a complete JSON message; parse and return it. If `poll_read`
        // returns Pending we stop and park the waker.
        loop {
            // Check if there's already a complete message sitting in read_buf.
            if let Some(pos) = this.read_buf.iter().position(|&b| b == b'\0') {
                let json_bytes = this.read_buf[..pos].to_vec();
                // Drain the consumed bytes (including the delimiter).
                this.read_buf.drain(..=pos);

                let msg = serde_json::from_slice::<Message<T>>(&json_bytes).map_err(CdpError::from);
                return Poll::Ready(Some(msg));
            }

            // Need more bytes from the pipe.
            let mut chunk = [0u8; 4096];
            let mut read_buf = ReadBuf::new(&mut chunk);
            match Pin::new(&mut this.reader).poll_read(cx, &mut read_buf) {
                Poll::Ready(Ok(())) => {
                    let filled = read_buf.filled();
                    if filled.is_empty() {
                        // EOF — Chrome closed the write end of the pipe.
                        return Poll::Ready(None);
                    }
                    this.read_buf.extend_from_slice(filled);
                    // Loop back to check for a delimiter in the newly buffered data.
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Some(Err(CdpError::Io(e)))),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
