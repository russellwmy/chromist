//! HAR (HTTP Archive) 1.2 types for recording and playback.
//!
//! - [`HarPlayback`] / `HarPlayback*` types are used internally by
//!   [`BrowserContext::route_from_har`](crate::BrowserContext::route_from_har).
//! - [`Har`] / `Har*` types are the public recording output produced by
//!   [`HarRecorder`](crate::har_recorder::HarRecorder).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Internal playback types (used by route_from_har)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct HarPlayback {
    pub log: HarPlaybackLog,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HarPlaybackLog {
    pub entries: Vec<HarPlaybackEntry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HarPlaybackEntry {
    pub request: HarPlaybackRequest,
    pub response: HarPlaybackResponse,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HarPlaybackRequest {
    pub method: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HarPlaybackResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<HarPlaybackHeader>,
    pub content: HarPlaybackContent,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct HarPlaybackHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HarPlaybackContent {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: Option<String>,
    /// Encoding of `text` field — `"base64"` means the text is base64-encoded.
    #[serde(default)]
    pub encoding: Option<String>,
}

// ---------------------------------------------------------------------------
// Public HAR 1.2 recording output types
// ---------------------------------------------------------------------------

/// Top-level HAR 1.2 document.
///
/// `serde_json::to_string(&har)` produces a valid HAR JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Har {
    /// Top-level `log` object (HAR 1.2 spec).
    pub log: HarLog,
}

/// The `log` object inside a HAR document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarLog {
    /// HAR spec version (`"1.2"`).
    pub version: String,
    /// Tool that produced this HAR (chromist + version).
    pub creator: HarCreator,
    /// One entry per recorded HTTP request/response pair.
    pub entries: Vec<HarEntry>,
}

/// Creator metadata embedded in the HAR log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarCreator {
    /// Producer tool name (`"chromist"`).
    pub name: String,
    /// Producer tool version (CARGO_PKG_VERSION).
    pub version: String,
}

/// One HTTP transaction recorded in the HAR.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarEntry {
    /// ISO 8601 timestamp: `"2024-01-15T10:30:45.123Z"`.
    pub started_date_time: String,
    /// Total elapsed time in milliseconds.
    pub time: f64,
    /// The HTTP request as sent.
    pub request: HarRequest,
    /// The HTTP response as received.
    pub response: HarResponse,
    /// Per-phase timing breakdown in milliseconds.
    pub timings: HarTimings,
}

/// HAR request object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarRequest {
    /// HTTP method (`GET`, `POST`, …).
    pub method: String,
    /// Full request URL.
    pub url: String,
    /// Negotiated HTTP version (`"http/1.1"`, `"h2"`, …).
    pub http_version: String,
    /// Request headers as ordered `HarHeader` records.
    pub headers: Vec<HarHeader>,
    /// Query-string parameters parsed from the URL.
    pub query_string: Vec<HarHeader>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// POST/PUT body if any (HAR `postData`).
    pub post_data: Option<HarPostData>,
    /// `-1` — not computed.
    pub headers_size: i64,
    /// Byte length of POST body, or `-1`.
    pub body_size: i64,
}

/// HAR response object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarResponse {
    /// HTTP status code.
    pub status: u16,
    /// Status reason phrase (`"OK"`, `"Not Found"`).
    pub status_text: String,
    /// Negotiated HTTP version.
    pub http_version: String,
    /// Response headers as ordered `HarHeader` records.
    pub headers: Vec<HarHeader>,
    /// Response body and metadata.
    pub content: HarContent,
    /// Empty string when there is no redirect.
    pub redirect_url: String,
    /// `-1` — not computed.
    pub headers_size: i64,
    /// `encoded_data_length` from CDP, or `-1`.
    pub body_size: i64,
}

/// A single HTTP header name/value pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarHeader {
    /// Header name.
    pub name: String,
    /// Header value.
    pub value: String,
}

/// POST body recorded in a HAR request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarPostData {
    /// `Content-Type` of the request body.
    pub mime_type: String,
    /// Request body text (UTF-8).
    pub text: String,
}

/// Response content recorded in a HAR entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarContent {
    /// Decoded response body size in bytes (`-1` if unknown).
    pub size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Bytes saved by compression (decoded − encoded), if known.
    pub compression: Option<i64>,
    /// Response `Content-Type`.
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Response body text (UTF-8) or base64 — see HAR `encoding` field.
    pub text: Option<String>,
    /// `"base64"` when `text` is base64-encoded binary data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
}

/// Per-phase timing breakdown (milliseconds).
///
/// A value of `-1.0` means the phase was not applicable or unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarTimings {
    /// Time spent in queue / connection limits (`-1` = not applicable).
    pub blocked: f64,
    /// DNS resolution time in milliseconds.
    pub dns: f64,
    /// TCP connect time in milliseconds.
    pub connect: f64,
    /// TLS handshake time in milliseconds.
    pub ssl: f64,
    /// Time spent sending the request to the server in milliseconds.
    pub send: f64,
    /// Time-to-first-byte (waiting for the server) in milliseconds.
    pub wait: f64,
    /// Time spent receiving the response body in milliseconds.
    pub receive: f64,
}

// ---------------------------------------------------------------------------
// HAR playback options (route_from_har)
// ---------------------------------------------------------------------------

/// What to do when no HAR entry matches a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HarNotFound {
    /// Continue the request normally (default).
    #[default]
    Continue,
    /// Abort the request with a network error.
    Abort,
}

/// Options for [`BrowserContext::route_from_har`](crate::BrowserContext::route_from_har).
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct HarOptions {
    /// Behaviour when no HAR entry matches.  Defaults to [`HarNotFound::Continue`].
    pub not_found: HarNotFound,
    /// Optional URL glob to limit which requests are served from the HAR.
    /// When `None`, all requests are matched.
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal lookup table
// ---------------------------------------------------------------------------

/// A pre-built lookup table derived from a parsed HAR file.
///
/// Entries are keyed by uppercased method + `\0` + URL.
pub(crate) struct HarLookup {
    pub(crate) entries: Vec<(String, String, HarPlaybackResponse)>,
}

impl HarLookup {
    pub(crate) fn from_har(har: HarPlayback) -> Self {
        let entries = har
            .log
            .entries
            .into_iter()
            .map(|e| {
                let key_method = e.request.method.to_uppercase();
                let key_url = e.request.url;
                (key_method, key_url, e.response)
            })
            .collect();
        Self { entries }
    }

    /// Find the first entry whose method and URL match.
    pub(crate) fn lookup(&self, method: &str, url: &str) -> Option<&HarPlaybackResponse> {
        let m = method.to_uppercase();
        self.entries.iter().find(|(em, eu, _)| em == &m && eu == url).map(|(_, _, r)| r)
    }
}
