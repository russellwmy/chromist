//! HAR recording via CDP Network events.
//!
//! [`HarRecorder`] subscribes to `Network.*` events, collects request/response
//! pairs, and produces a valid [`Har`](crate::har::Har) document when stopped.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use futures::{FutureExt, StreamExt};

use crate::cdp::browser_protocol::network as cdp_network;
use crate::handler::{HandlerHandle, SessionRef};
use crate::har::{
    Har, HarContent, HarCreator, HarEntry, HarHeader, HarLog, HarPostData, HarRequest, HarResponse,
    HarTimings,
};

/// Options for [`HarRecorder`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HarRecordingOptions {
    /// Fetch response bodies via `Network.getResponseBody`. Default: `true`.
    pub capture_body: bool,
}

impl Default for HarRecordingOptions {
    fn default() -> Self {
        Self { capture_body: true }
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct PendingEntry {
    /// Wall-clock seconds since Unix epoch at which the request was sent.
    started_wall_secs: f64,
    /// Monotonic timestamp (seconds) at which the request was sent.
    started_mono: f64,
    method: String,
    url: String,
    http_version: String,
    request_headers: Vec<HarHeader>,
    query_string: Vec<HarHeader>,
    post_data: Option<HarPostData>,
    body_size: i64,
    // Filled in when ResponseReceived fires.
    response: Option<PendingResponse>,
}

struct PendingResponse {
    status: u16,
    status_text: String,
    http_version: String,
    headers: Vec<HarHeader>,
    mime_type: String,
    timing: Option<cdp_network::ResourceTiming>,
    encoded_data_length: f64,
}

struct RecorderState {
    pending: HashMap<String, PendingEntry>,
    finished: Vec<HarEntry>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Records HTTP traffic as a HAR 1.2 document.
///
/// Create via [`Page::start_har_recording`](crate::page::Page::start_har_recording),
/// then call [`stop`](HarRecorder::stop) or [`content`](HarRecorder::content)
/// when done.
pub struct HarRecorder {
    state: Arc<Mutex<RecorderState>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Aborts the background event loop when the recorder is dropped without
    /// `stop()` being called, so the task does not outlive its parent.
    _task: crate::runtime::AbortOnDrop,
}

impl std::fmt::Debug for HarRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarRecorder")
            .field("running", &self.shutdown_tx.is_some())
            .finish_non_exhaustive()
    }
}

/// Lock the state mutex, recovering from poison with a logged warning.
///
/// Poison here means a panic occurred in one of the event handlers while the
/// guard was held — the inner state may be inconsistent. We log and continue
/// rather than propagate, because the recorder is a fire-and-forget task whose
/// caller has no opportunity to handle a `Result` per event.
fn lock_state<'a>(
    state: &'a Mutex<RecorderState>,
    site: &'static str,
) -> std::sync::MutexGuard<'a, RecorderState> {
    state.lock().unwrap_or_else(|e| {
        tracing::warn!(
            site,
            "har_recorder state lock poisoned; recovering with possibly inconsistent state"
        );
        e.into_inner()
    })
}

impl HarRecorder {
    /// Start recording.  Enables the Network domain and spawns a background
    /// task that listens for `Network.*` events.
    pub(crate) fn start(
        handle: HandlerHandle,
        session_id: SessionRef,
        options: HarRecordingOptions,
    ) -> Self {
        let state =
            Arc::new(Mutex::new(RecorderState { pending: HashMap::new(), finished: Vec::new() }));

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let task = crate::runtime::spawn(Self::run(
            handle,
            session_id,
            options,
            Arc::clone(&state),
            shutdown_rx,
        ));

        Self { state, shutdown_tx: Some(shutdown_tx), _task: crate::runtime::AbortOnDrop(task) }
    }

    /// Stop recording and return the collected HAR document.
    ///
    /// Consumes the recorder. The background task is signalled to stop; any
    /// in-flight entries that have not yet received `loadingFinished` are
    /// discarded.
    #[must_use]
    pub fn stop(mut self) -> Har {
        // Signal shutdown — ignore send errors (task may have already exited).
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        let mut guard = lock_state(&self.state, "stop");
        let entries = std::mem::take(&mut guard.finished);

        Har {
            log: HarLog {
                version: "1.2".to_string(),
                creator: HarCreator {
                    name: "chromist".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
                entries,
            },
        }
    }

    /// Stop recording and return the HAR document serialized as JSON.
    ///
    /// # Errors
    ///
    /// Returns [`CdpError::Json`](crate::CdpError::Json) if serialization fails (which should not
    /// happen for well-formed HAR data).
    pub fn content(self) -> crate::Result<String> {
        let har = self.stop();
        Ok(serde_json::to_string(&har)?)
    }

    /// Stop recording and write the HAR JSON to `path`.
    ///
    /// # Errors
    ///
    /// Returns an error on serialization or I/O failure.
    pub async fn save_as(self, path: impl AsRef<std::path::Path>) -> crate::Result<()> {
        let json = self.content()?;
        tokio::fs::write(path.as_ref(), json.as_bytes()).await?;
        Ok(())
    }

    // ---------------------------------------------------------------------------
    // Background task
    // ---------------------------------------------------------------------------

    async fn run(
        handle: HandlerHandle,
        session_id: SessionRef,
        options: HarRecordingOptions,
        state: Arc<Mutex<RecorderState>>,
        shutdown_rx: oneshot::Receiver<()>,
    ) {
        let sid = session_id.current();

        // Enable the Network domain so events start flowing. Without this no
        // events will arrive — surface a warning if it fails.
        if let Err(e) =
            handle.execute(cdp_network::EnableParams::default(), Some(Arc::clone(&sid))).await
        {
            tracing::warn!(error = %e, "har_recorder: Network.enable failed; recording will be empty");
        }

        // Subscribe to the four event types we care about.
        let mut will_be_sent =
            handle.event_listener::<cdp_network::RequestWillBeSentEvent>(Some(Arc::clone(&sid)));
        let mut response_received =
            handle.event_listener::<cdp_network::ResponseReceivedEvent>(Some(Arc::clone(&sid)));
        let mut loading_finished =
            handle.event_listener::<cdp_network::LoadingFinishedEvent>(Some(Arc::clone(&sid)));
        let mut loading_failed =
            handle.event_listener::<cdp_network::LoadingFailedEvent>(Some(Arc::clone(&sid)));

        let mut shutdown = shutdown_rx.fuse();

        loop {
            futures::select! {
                ev = will_be_sent.next().fuse() => {
                    let Some(ev) = ev else { break };
                    Self::handle_request_will_be_sent(ev, &state);
                }
                ev = response_received.next().fuse() => {
                    let Some(ev) = ev else { break };
                    Self::handle_response_received(ev, &state);
                }
                ev = loading_finished.next().fuse() => {
                    let Some(ev) = ev else { break };
                    if options.capture_body {
                        Self::handle_loading_finished_with_body(
                            ev,
                            &state,
                            &handle,
                            Arc::clone(&sid),
                        )
                        .await;
                    } else {
                        Self::handle_loading_finished(ev, &state, None);
                    }
                }
                ev = loading_failed.next().fuse() => {
                    let Some(ev) = ev else { break };
                    Self::handle_loading_failed(ev, &state);
                }
                _ = shutdown => break,
            }
        }
    }

    fn handle_request_will_be_sent(
        ev: cdp_network::RequestWillBeSentEvent,
        state: &Arc<Mutex<RecorderState>>,
    ) {
        let req_id = ev.request_id.0.clone();

        // If there is a redirect response attached, finalize the previous
        // pending entry for this request_id before creating the new one.
        if let Some(redirect_resp) = ev.redirect_response {
            let mut guard = lock_state(state, "request_will_be_sent_redirect");
            if let Some(mut prev) = guard.pending.remove(&req_id) {
                prev.response = Some(PendingResponse {
                    status: u16::try_from(redirect_resp.status).unwrap_or(0),
                    status_text: redirect_resp.status_text.clone(),
                    http_version: redirect_resp
                        .protocol
                        .as_deref()
                        .map(protocol_to_http_version)
                        .unwrap_or_else(|| "HTTP/1.1".to_string()),
                    headers: cdp_headers_to_har(&redirect_resp.headers),
                    mime_type: redirect_resp.mime_type.clone(),
                    timing: redirect_resp.timing,
                    encoded_data_length: redirect_resp.encoded_data_length,
                });
                // total_ms for a redirect: use 0 since we don't have a
                // loadingFinished timestamp.
                if let Some(entry) = build_har_entry(&prev, ev.timestamp.0, None) {
                    guard.finished.push(entry);
                }
            }
        }

        let url = ev.request.url.clone();
        let query_string = parse_query_string(&url);
        let method = ev.request.method.clone();
        let request_headers = cdp_headers_to_har(&ev.request.headers);
        let post_data = ev.request.post_data.as_deref().map(|body| HarPostData {
            mime_type: content_type_from_headers(&request_headers),
            text: body.to_string(),
        });
        let body_size = ev.request.post_data.as_deref().map(|b| b.len() as i64).unwrap_or(-1);

        let entry = PendingEntry {
            started_wall_secs: ev.wall_time.0,
            started_mono: ev.timestamp.0,
            method,
            url,
            http_version: "HTTP/1.1".to_string(),
            request_headers,
            query_string,
            post_data,
            body_size,
            response: None,
        };

        let mut guard = lock_state(state, "request_will_be_sent");
        guard.pending.insert(req_id, entry);
    }

    fn handle_response_received(
        ev: cdp_network::ResponseReceivedEvent,
        state: &Arc<Mutex<RecorderState>>,
    ) {
        let req_id = ev.request_id.0.clone();
        let mut guard = lock_state(state, "response_received");
        if let Some(entry) = guard.pending.get_mut(&req_id) {
            entry.http_version = ev
                .response
                .protocol
                .as_deref()
                .map(protocol_to_http_version)
                .unwrap_or_else(|| "HTTP/1.1".to_string());
            entry.response = Some(PendingResponse {
                status: u16::try_from(ev.response.status).unwrap_or(0),
                status_text: ev.response.status_text.clone(),
                http_version: ev
                    .response
                    .protocol
                    .as_deref()
                    .map(protocol_to_http_version)
                    .unwrap_or_else(|| "HTTP/1.1".to_string()),
                headers: cdp_headers_to_har(&ev.response.headers),
                mime_type: ev.response.mime_type.clone(),
                timing: ev.response.timing,
                encoded_data_length: ev.response.encoded_data_length,
            });
        }
    }

    fn handle_loading_finished(
        ev: cdp_network::LoadingFinishedEvent,
        state: &Arc<Mutex<RecorderState>>,
        body: Option<(String, bool)>,
    ) {
        let req_id = ev.request_id.0.clone();
        let mut guard = lock_state(state, "loading_finished");
        if let Some(entry) = guard.pending.remove(&req_id) {
            if let Some(har_entry) = build_har_entry(&entry, ev.timestamp.0, body) {
                guard.finished.push(har_entry);
            }
        }
    }

    async fn handle_loading_finished_with_body(
        ev: cdp_network::LoadingFinishedEvent,
        state: &Arc<Mutex<RecorderState>>,
        handle: &HandlerHandle,
        session_id: Arc<str>,
    ) {
        let req_id = ev.request_id.0.clone();

        // Fetch the body before removing from pending so we can pass it in.
        let body_result = handle
            .execute(
                cdp_network::GetResponseBodyParams::new(cdp_network::RequestId(req_id.clone())),
                Some(session_id),
            )
            .await;

        let body = match body_result {
            Ok(resp) => Some((resp.body, resp.base64_encoded)),
            Err(_) => None,
        };

        Self::handle_loading_finished(ev, state, body);
    }

    fn handle_loading_failed(
        ev: cdp_network::LoadingFailedEvent,
        state: &Arc<Mutex<RecorderState>>,
    ) {
        // Drop the pending entry — there is no response to record.
        let mut guard = lock_state(state, "loading_failed");
        guard.pending.remove(&ev.request_id.0);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert CDP `Headers` (a JSON object) to a `Vec<HarHeader>`.
fn cdp_headers_to_har(headers: &cdp_network::Headers) -> Vec<HarHeader> {
    headers
        .inner()
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| HarHeader {
                    name: k.clone(),
                    value: v.as_str().unwrap_or("").to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse `?key=value&…` query pairs from a URL into `Vec<HarHeader>`.
fn parse_query_string(url: &str) -> Vec<HarHeader> {
    let query = match url.find('?') {
        Some(pos) => &url[pos + 1..],
        None => return Vec::new(),
    };
    // Strip any fragment.
    let query = match query.find('#') {
        Some(pos) => &query[..pos],
        None => query,
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            if let Some(eq) = pair.find('=') {
                HarHeader {
                    name: percent_decode(&pair[..eq]),
                    value: percent_decode(&pair[eq + 1..]),
                }
            } else {
                HarHeader { name: percent_decode(pair), value: String::new() }
            }
        })
        .collect()
}

/// Minimal percent-decode: replace `%XX` sequences and `+` with spaces.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
        } else if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(char::from(hi << 4 | lo));
                i += 3;
            } else {
                out.push('%');
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Map CDP `protocol` strings to HTTP version strings.
fn protocol_to_http_version(protocol: &str) -> String {
    match protocol {
        "h2" => "HTTP/2.0".to_string(),
        "h3" | "h3-29" => "HTTP/3.0".to_string(),
        "http/1.0" => "HTTP/1.0".to_string(),
        _ => "HTTP/1.1".to_string(),
    }
}

/// Extract `Content-Type` value from a header list, or return `"text/plain"`.
fn content_type_from_headers(headers: &[HarHeader]) -> String {
    headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.clone())
        .unwrap_or_else(|| "text/plain".to_string())
}

/// Compute a single phase duration (ms) from start/end millisecond offsets.
///
/// Returns `-1.0` when either endpoint is unavailable (`< 0`).
fn timing_ms(start: f64, end: f64) -> f64 {
    if start < 0.0 || end < 0.0 {
        -1.0
    } else {
        (end - start).max(0.0)
    }
}

/// Build a [`HarTimings`] from a CDP [`ResourceTiming`] and a total elapsed
/// millisecond count.
fn build_timings(timing: &cdp_network::ResourceTiming, total_ms: f64) -> HarTimings {
    let dns = timing_ms(timing.dns_start, timing.dns_end);
    let connect = timing_ms(timing.connect_start, timing.connect_end);
    let ssl = timing_ms(timing.ssl_start, timing.ssl_end);
    let send = timing_ms(timing.send_start, timing.send_end);
    let wait = timing_ms(timing.send_end, timing.receive_headers_start);

    // receive = total − sum of all known-positive phases.
    let known: f64 = [dns, connect, ssl, send, wait].iter().filter(|&&v| v >= 0.0).sum();
    let receive = (total_ms - known).max(0.0);

    HarTimings { blocked: -1.0, dns, connect, ssl, send, wait, receive }
}

/// Assemble a finished [`HarEntry`] from the pending state.
///
/// Returns `None` when the response has not yet been recorded (should not
/// happen in practice but guards against partial state).
fn build_har_entry(
    entry: &PendingEntry,
    finished_mono: f64,
    body: Option<(String, bool)>,
) -> Option<HarEntry> {
    let resp = entry.response.as_ref()?;

    let total_ms = (finished_mono - entry.started_mono) * 1000.0;
    let total_ms = total_ms.max(0.0);

    let timings = resp.timing.as_ref().map(|t| build_timings(t, total_ms)).unwrap_or(HarTimings {
        blocked: -1.0,
        dns: -1.0,
        connect: -1.0,
        ssl: -1.0,
        send: 0.0,
        wait: total_ms,
        receive: 0.0,
    });

    let (body_text, body_encoding) = match body {
        Some((text, true)) => (Some(text), Some("base64".to_string())),
        Some((text, false)) => (Some(text), None),
        None => (None, None),
    };

    let body_size =
        if resp.encoded_data_length > 0.0 { resp.encoded_data_length as i64 } else { -1 };

    let content = HarContent {
        size: body_size,
        compression: None,
        mime_type: resp.mime_type.clone(),
        text: body_text,
        encoding: body_encoding,
    };

    let har_request = HarRequest {
        method: entry.method.clone(),
        url: entry.url.clone(),
        http_version: entry.http_version.clone(),
        headers: entry.request_headers.clone(),
        query_string: entry.query_string.clone(),
        post_data: entry.post_data.clone(),
        headers_size: -1,
        body_size: entry.body_size,
    };

    let har_response = HarResponse {
        status: resp.status,
        status_text: resp.status_text.clone(),
        http_version: resp.http_version.clone(),
        headers: resp.headers.clone(),
        content,
        redirect_url: String::new(),
        headers_size: -1,
        body_size,
    };

    Some(HarEntry {
        started_date_time: epoch_secs_to_iso8601(entry.started_wall_secs),
        time: total_ms,
        request: har_request,
        response: har_response,
        timings,
    })
}

// ---------------------------------------------------------------------------
// ISO 8601 formatting without chrono
// ---------------------------------------------------------------------------

/// Format a Unix timestamp (seconds, fractional) as `"YYYY-MM-DDTHH:MM:SS.mmmZ"`.
fn epoch_secs_to_iso8601(secs: f64) -> String {
    let secs_i = secs.floor() as i64;
    let ms = ((secs - secs.floor()) * 1000.0).round() as u32;

    let (year, month, day) = civil_from_days(secs_i / 86400);
    let day_secs = secs_i.rem_euclid(86400) as u32;
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year, month, day, hour, minute, second, ms
    )
}

/// Howard Hinnant's civil_from_days algorithm.
///
/// Converts a number of days since the Unix epoch (1970-01-01) to
/// `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era: i64 = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_secs_to_iso8601_unix_epoch() {
        assert_eq!(epoch_secs_to_iso8601(0.0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn epoch_secs_to_iso8601_with_ms() {
        // 2024-01-15 10:30:45.123 UTC
        // date -d "2024-01-15 10:30:45 UTC" +%s → 1705314645
        let secs = 1_705_314_645.123_f64;
        let result = epoch_secs_to_iso8601(secs);
        assert_eq!(result, "2024-01-15T10:30:45.123Z");
    }

    #[test]
    fn parse_query_string_basic() {
        let pairs = parse_query_string("https://example.com/path?foo=bar&baz=qux");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].name, "foo");
        assert_eq!(pairs[0].value, "bar");
        assert_eq!(pairs[1].name, "baz");
        assert_eq!(pairs[1].value, "qux");
    }

    #[test]
    fn parse_query_string_empty() {
        let pairs = parse_query_string("https://example.com/path");
        assert!(pairs.is_empty());
    }

    #[test]
    fn parse_query_string_with_fragment() {
        let pairs = parse_query_string("https://example.com/?key=val#section");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].name, "key");
        assert_eq!(pairs[0].value, "val");
    }

    #[test]
    fn timing_ms_basic() {
        assert!((timing_ms(0.0, 10.0) - 10.0).abs() < f64::EPSILON);
        assert!((timing_ms(-1.0, 10.0) - (-1.0)).abs() < f64::EPSILON);
        assert!((timing_ms(10.0, -1.0) - (-1.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn timing_ms_no_negative_result() {
        // end < start should clamp to 0, not go negative.
        assert!((timing_ms(10.0, 5.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn cdp_headers_to_har_basic() {
        let json = serde_json::json!({ "Content-Type": "text/html", "X-Foo": "bar" });
        let headers = cdp_network::Headers::new(json);
        let har = cdp_headers_to_har(&headers);
        assert_eq!(har.len(), 2);
        let ct = har.iter().find(|h| h.name == "Content-Type").unwrap();
        assert_eq!(ct.value, "text/html");
    }

    #[test]
    fn cdp_headers_to_har_non_object() {
        let json = serde_json::json!(null);
        let headers = cdp_network::Headers::new(json);
        let har = cdp_headers_to_har(&headers);
        assert!(har.is_empty());
    }

    #[test]
    fn har_recorder_options_default() {
        let opts = HarRecordingOptions::default();
        assert!(opts.capture_body);
    }

    #[test]
    fn percent_decode_plus_space() {
        assert_eq!(percent_decode("hello+world"), "hello world");
    }

    #[test]
    fn percent_decode_encoded() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("%41"), "A");
    }

    #[test]
    fn percent_decode_invalid_escape_passthrough() {
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn protocol_to_http_version_mapping() {
        assert_eq!(protocol_to_http_version("h2"), "HTTP/2.0");
        assert_eq!(protocol_to_http_version("h3"), "HTTP/3.0");
        assert_eq!(protocol_to_http_version("http/1.0"), "HTTP/1.0");
        assert_eq!(protocol_to_http_version("unknown"), "HTTP/1.1");
    }
}
