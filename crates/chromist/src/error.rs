use thiserror::Error;

/// Coarse classification of a [`CdpError`](crate::CdpError) for retry/abort decisions.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Transient: re-resolving the element or execution context may succeed.
    Retry,
    /// Connection-level failure: the browser is gone; abort the entire operation.
    Fatal,
    /// Caller contract violation: surface immediately, never swallow.
    Strict,
}

/// Every fallible operation in chromist returns this error type.
///
/// Use [`CdpError::kind`] to classify a variant for retry/abort decisions
/// rather than matching on individual variants — `kind` returns one of three
/// abstract categories ([`ErrorKind::Retry`], [`ErrorKind::Fatal`],
/// [`ErrorKind::Strict`]) that's stable across CDP-level changes.
///
/// `#[non_exhaustive]` — new variants may be added in patch releases.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum CdpError {
    /// WebSocket transport error wrapping `tungstenite::Error`. Boxed so the
    /// enum stays small.
    #[error("WebSocket error: {0}")]
    WebSocket(Box<async_tungstenite::tungstenite::Error>),
    /// `serde_json` failure during request serialisation or response
    /// deserialisation.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// CDP protocol-level error: the browser reported a numeric code and message.
    #[error("CDP protocol error: {code} {message}")]
    Cdp {
        /// CDP protocol error code (e.g. `-32601` for "method not found").
        code: i64,
        /// Human-readable error message returned by the browser.
        message: String,
    },
    /// Generic timeout — a CDP command, navigation, or wait exceeded its
    /// deadline. Classified as [`ErrorKind::Fatal`] by default; callers
    /// using a [`Progress`](crate::Progress) deadline see this when the
    /// deadline elapses.
    #[error("Timeout")]
    Timeout,
    /// Chrome/Chromium executable not found on the system.
    #[error("Chrome/Chromium executable not found")]
    LaunchNotFound,
    /// The requested transport (pipe) is not supported on this platform.
    #[error("unsupported transport: {0}")]
    UnsupportedTransport(&'static str),
    /// Page navigation rejected by the browser (the renderer reported an error
    /// before the load completed). Retryable: re-issuing the navigation may
    /// succeed under different network conditions.
    #[error("navigation failed: {reason}")]
    NavigationFailed {
        /// Renderer-supplied reason text for the navigation failure.
        reason: String,
    },
    /// A filesystem path was not valid UTF-8 in a context that requires it
    /// (e.g. the CDP `Browser.setDownloadBehavior.downloadPath` field).
    #[error("invalid path (not valid UTF-8): {0}")]
    InvalidPath(std::path::PathBuf),
    /// `pipe()` syscall failed while setting up the OS-pipe transport.
    #[error("pipe() syscall failed: {source}")]
    LaunchPipeError {
        /// Underlying `pipe(2)` errno wrapped as an [`std::io::Error`].
        source: std::io::Error,
    },
    /// Failed to spawn the Chrome process.
    #[error("failed to spawn Chrome: {source}")]
    LaunchSpawnError {
        /// Underlying spawn failure (typically `ENOENT` or `EACCES`).
        source: std::io::Error,
    },
    /// Browser process exited before the WebSocket debug URL was found.
    #[error("Browser exited during launch (status={exit_status}): {stderr}")]
    LaunchExit {
        /// Stringified exit status of the browser child process.
        exit_status: String,
        /// Captured stderr lines (most recent 200) from the browser process.
        stderr: String,
    },
    /// Timed out waiting for the browser's WebSocket debug URL.
    #[error("Timeout waiting for browser WebSocket URL: {stderr}")]
    LaunchTimeout {
        /// Captured stderr lines (most recent 200) from the browser process.
        stderr: String,
    },
    /// Generic `std::io` error from any I/O operation.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// The handler task is gone — the connection has closed and any future
    /// CDP calls will not complete. Classified as [`ErrorKind::Fatal`].
    #[error("Channel closed")]
    ChannelClosed,
    /// JavaScript exception with full CDP `ExceptionDetails` (stack trace, location, object).
    #[error("JavaScript exception at {}:{} — {}", _0.line_number, _0.column_number, _0.text)]
    JavascriptException(Box<crate::cdp::js_protocol::runtime::ExceptionDetails>),
    /// Element / target / frame did not exist when the operation ran. Often
    /// retryable: re-resolving the locator may succeed.
    #[error("Not found")]
    NotFound,
    /// A resolved DOM node was missing its `objectId` — usually means the node
    /// was detached from the document between resolution and access.
    #[error("Resolved node has no objectId")]
    MissingObjectId,
    /// The browser process handle is absent (already killed or never started).
    #[error("No child process")]
    NoChildProcess,
    /// HTTP-over-TCP connection to the browser's DevTools endpoint failed.
    #[error("DevTools HTTP discovery error ({host}): {reason}")]
    HttpDiscovery {
        /// Hostname or address of the DevTools endpoint that failed.
        host: String,
        /// Human-readable failure reason.
        reason: String,
    },
    /// `Element.scrollIntoViewIfNeeded` reported the element could not be
    /// scrolled into view (detached, hidden, or unreachable).
    #[error("Scrolling failed: {0}")]
    ScrollingFailed(String),
    /// A `FrameId` referenced a frame that is no longer in the page's frame
    /// tree (the iframe was navigated away or detached).
    #[error("Frame not found: {}", _0.inner())]
    FrameNotFound(crate::cdp::browser_protocol::page::FrameId),
    /// URL parsing failed (e.g. when constructing a request from a
    /// malformed string).
    #[error("URL parse error: {0}")]
    Url(#[from] url::ParseError),
    /// Base64 decoding failed (typically when reading a CDP response that
    /// promised binary data).
    #[error("Decode error: {0}")]
    Decode(#[from] base64::DecodeError),
    /// A CDP message arrived that could not be deserialized.
    #[error("Invalid CDP message '{0}': {1}")]
    InvalidMessage(String, #[source] serde_json::Error),
    /// An internal `RwLock` or `Mutex` was poisoned by a previous panic.
    /// Surfaced as a typed error rather than panicking again.
    #[error("Internal lock poisoned")]
    LockPoisoned,
    /// A CSS selector matched more than one element in strict mode.
    #[error("Strict mode: selector matched more than one element")]
    StrictViolation,
    /// HTTP method string passed to `APIRequestContext` was not a valid HTTP
    /// method per RFC 7230.
    #[error("Invalid HTTP method: {0}")]
    InvalidMethod(String),
    /// `reqwest` reported a transport-level failure (connect, TLS, body
    /// read) when the request context was used outside CDP.
    #[error("HTTP request failed: {0}")]
    RequestFailed(String),
    /// HTTP response has not yet finished loading.
    #[error("HTTP response has not yet finished loading")]
    ResponseNotReady,
    /// Attempted to close the default browser context. Only non-default
    /// (incognito) contexts can be disposed; the default context's lifetime
    /// is tied to the browser process itself.
    #[error("cannot close the default browser context; close the browser instead")]
    CannotCloseDefaultContext,
}

impl CdpError {
    /// Classify this error for retry/abort logic in action loops.
    ///
    /// - `Retry`: the element or context was detached; re-resolve and try again.
    /// - `Fatal`: the browser connection is gone; bail out of the entire operation.
    /// - `Strict`: a caller bug or internal invariant violation; surface immediately.
    pub fn kind(&self) -> ErrorKind {
        match self {
            // Detached node / missing context — re-resolving the element may fix it.
            CdpError::NotFound | CdpError::MissingObjectId | CdpError::FrameNotFound(_) => {
                ErrorKind::Retry
            }
            CdpError::ScrollingFailed(_)
            | CdpError::ResponseNotReady
            | CdpError::NavigationFailed { .. } => ErrorKind::Retry,
            // CDP protocol errors: codes -32000 and -32001 are transient runtime
            // errors ("node not in document", "session not found" after renderer swap).
            // -32602 is always a caller bug (invalid params). Everything else is fatal.
            CdpError::Cdp { code, .. } => match *code {
                -32000 | -32001 => ErrorKind::Retry,
                -32602 => ErrorKind::Strict,
                _ => ErrorKind::Fatal,
            },
            // Connection-level failures — browser is gone or couldn't start.
            CdpError::WebSocket(_)
            | CdpError::Io(_)
            | CdpError::ChannelClosed
            | CdpError::Timeout
            | CdpError::LaunchExit { .. }
            | CdpError::LaunchTimeout { .. }
            | CdpError::LaunchPipeError { .. }
            | CdpError::LaunchSpawnError { .. }
            | CdpError::NoChildProcess => ErrorKind::Fatal,
            // Internal invariant violations and caller bugs.
            CdpError::LockPoisoned
            | CdpError::JavascriptException(_)
            | CdpError::Json(_)
            | CdpError::Decode(_)
            | CdpError::InvalidMessage(_, _)
            | CdpError::LaunchNotFound
            | CdpError::UnsupportedTransport(_)
            | CdpError::InvalidPath(_)
            | CdpError::Url(_)
            | CdpError::HttpDiscovery { .. }
            | CdpError::StrictViolation
            | CdpError::InvalidMethod(_)
            | CdpError::RequestFailed(_)
            | CdpError::CannotCloseDefaultContext => ErrorKind::Strict,
        }
    }

    /// Returns `true` if retrying the operation (after re-resolving the element)
    /// might succeed.
    pub fn is_retryable(&self) -> bool {
        self.kind() == ErrorKind::Retry
    }
}

impl From<async_tungstenite::tungstenite::Error> for CdpError {
    fn from(e: async_tungstenite::tungstenite::Error) -> Self {
        CdpError::WebSocket(Box::new(e))
    }
}

impl From<chromist_types::CdpError> for CdpError {
    fn from(e: chromist_types::CdpError) -> Self {
        CdpError::Cdp { code: e.code, message: e.message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_messages() {
        let e = CdpError::Timeout;
        assert_eq!(e.to_string(), "Timeout");

        let e = CdpError::ChannelClosed;
        assert_eq!(e.to_string(), "Channel closed");

        let e = CdpError::NotFound;
        assert_eq!(e.to_string(), "Not found");

        let e = CdpError::UnsupportedTransport("pipe transport is only supported on Unix");
        assert!(e.to_string().contains("pipe transport"));

        let e = CdpError::NavigationFailed { reason: "net::ERR_ABORTED".into() };
        assert!(e.to_string().contains("net::ERR_ABORTED"));

        let e = CdpError::InvalidPath(std::path::PathBuf::from("/tmp/x"));
        assert!(e.to_string().contains("/tmp/x"));

        let e = CdpError::MissingObjectId;
        assert_eq!(e.to_string(), "Resolved node has no objectId");

        let e = CdpError::NoChildProcess;
        assert_eq!(e.to_string(), "No child process");

        let e = CdpError::HttpDiscovery {
            host: "localhost:9222".into(),
            reason: "connection refused".into(),
        };
        assert!(e.to_string().contains("localhost:9222"));
        assert!(e.to_string().contains("connection refused"));

        let e = CdpError::Cdp { code: -32000, message: "oops".to_string() };
        assert_eq!(e.to_string(), "CDP protocol error: -32000 oops");
    }

    #[test]
    fn test_from_serde_json_error() {
        fn parse_int(s: &str) -> crate::Result<i32> {
            let v: i32 = serde_json::from_str(s)?;
            Ok(v)
        }
        let result = parse_int("bad");
        assert!(matches!(result, Err(CdpError::Json(_))));
    }

    #[test]
    fn from_wire_cdp_error() {
        let wire = chromist_types::CdpError::new(-32602, "invalid params");
        let e: CdpError = wire.into();
        match e {
            CdpError::Cdp { code, message } => {
                assert_eq!(code, -32602);
                assert_eq!(message, "invalid params");
            }
            other => panic!("expected Cdp, got {:?}", other),
        }
    }

    #[test]
    fn from_io_error() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let e: CdpError = io.into();
        assert!(matches!(e, CdpError::Io(_)));
        assert!(e.to_string().contains("missing"));
    }

    #[test]
    fn from_url_parse_error() {
        let err = url::Url::parse("not a url").unwrap_err();
        let e: CdpError = err.into();
        assert!(matches!(e, CdpError::Url(_)));
    }

    #[test]
    fn from_base64_decode_error() {
        use base64::Engine as _;
        let err = base64::engine::general_purpose::STANDARD.decode("!!!").unwrap_err();
        let e: CdpError = err.into();
        assert!(matches!(e, CdpError::Decode(_)));
    }

    #[test]
    fn launch_exit_and_timeout_display_contains_stderr() {
        let e = CdpError::LaunchExit { exit_status: "signal: 9".into(), stderr: "killed".into() };
        let s = e.to_string();
        assert!(s.contains("signal: 9"));
        assert!(s.contains("killed"));

        let e = CdpError::LaunchTimeout { stderr: "no ws url".into() };
        assert!(e.to_string().contains("no ws url"));
    }

    #[test]
    fn invalid_message_display_includes_raw() {
        let err = serde_json::from_str::<i32>("bad").unwrap_err();
        let e = CdpError::InvalidMessage("{malformed}".into(), err);
        let s = e.to_string();
        assert!(s.contains("{malformed}"));
    }

    #[test]
    fn frame_not_found_display_contains_id() {
        use crate::cdp::browser_protocol::page::FrameId;
        let fid = FrameId::new("frame-123");
        let e = CdpError::FrameNotFound(fid);
        assert!(e.to_string().contains("frame-123"));
    }

    #[test]
    fn error_kind_retry_variants() {
        assert_eq!(CdpError::NotFound.kind(), ErrorKind::Retry);
        assert_eq!(CdpError::MissingObjectId.kind(), ErrorKind::Retry);
        assert_eq!(CdpError::ScrollingFailed("x".into()).kind(), ErrorKind::Retry);
        assert_eq!(CdpError::Cdp { code: -32000, message: String::new() }.kind(), ErrorKind::Retry);
        assert_eq!(CdpError::Cdp { code: -32001, message: String::new() }.kind(), ErrorKind::Retry);
    }

    #[test]
    fn error_kind_fatal_variants() {
        assert_eq!(CdpError::ChannelClosed.kind(), ErrorKind::Fatal);
        assert_eq!(CdpError::Timeout.kind(), ErrorKind::Fatal);
        assert_eq!(
            CdpError::LaunchExit { exit_status: String::new(), stderr: String::new() }.kind(),
            ErrorKind::Fatal
        );
        assert_eq!(CdpError::NoChildProcess.kind(), ErrorKind::Fatal);
    }

    #[test]
    fn error_kind_strict_variants() {
        assert_eq!(CdpError::LockPoisoned.kind(), ErrorKind::Strict);
        assert_eq!(CdpError::UnsupportedTransport("pipe").kind(), ErrorKind::Strict);
        assert_eq!(CdpError::InvalidPath(std::path::PathBuf::from("/x")).kind(), ErrorKind::Strict);
        assert_eq!(
            CdpError::NavigationFailed { reason: "net::ERR_ABORTED".into() }.kind(),
            ErrorKind::Retry
        );
        assert_eq!(
            CdpError::Cdp { code: -32602, message: String::new() }.kind(),
            ErrorKind::Strict
        );
        assert!(CdpError::NotFound.is_retryable());
        assert!(!CdpError::ChannelClosed.is_retryable());
    }

    #[test]
    fn response_not_ready_is_retry() {
        assert_eq!(CdpError::ResponseNotReady.kind(), ErrorKind::Retry);
    }

    #[test]
    fn response_not_ready_display() {
        assert!(CdpError::ResponseNotReady.to_string().contains("finished loading"));
    }

    #[test]
    fn launch_not_found_is_strict() {
        assert_eq!(CdpError::LaunchNotFound.kind(), ErrorKind::Strict);
        assert!(CdpError::LaunchNotFound.to_string().contains("not found"));
    }

    #[test]
    fn launch_spawn_error_is_fatal() {
        let e = CdpError::LaunchSpawnError {
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        };
        assert_eq!(e.kind(), ErrorKind::Fatal);
        assert!(e.to_string().contains("spawn"));
    }

    #[test]
    fn launch_pipe_error_is_fatal() {
        let e = CdpError::LaunchPipeError { source: std::io::Error::other("too many open files") };
        assert_eq!(e.kind(), ErrorKind::Fatal);
        assert!(e.to_string().contains("pipe()"));
    }
}
