//! Route interception layer.
//!
//! [`RouteRegistry`] holds a list of URL-pattern / handler pairs. When a
//! `Fetch.requestPaused` event fires the registry is consulted in registration
//! order; the first matching handler is called. Unmatched requests are
//! automatically continued.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::cdp::browser_protocol::fetch as cdp_fetch;
use crate::error::CdpError;
use crate::handler::{HandlerHandle, SessionRef};

// ---------------------------------------------------------------------------
// AbortReason
// ---------------------------------------------------------------------------

/// Network error reason for [`Route::abort`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    /// Generic request failure (`net::ERR_FAILED`).
    Failed,
    /// User cancelled the request before it completed (`net::ERR_ABORTED`).
    Aborted,
    /// The request timed out at the network layer (`net::ERR_TIMED_OUT`).
    TimedOut,
    /// Server replied 403 / equivalent (`net::ERR_ACCESS_DENIED`).
    AccessDenied,
    /// Connection closed cleanly mid-request (`net::ERR_CONNECTION_CLOSED`).
    ConnectionClosed,
    /// TCP connection was reset by the peer (`net::ERR_CONNECTION_RESET`).
    ConnectionReset,
    /// Server actively refused the connection (`net::ERR_CONNECTION_REFUSED`).
    ConnectionRefused,
    /// Connection aborted by the client (`net::ERR_CONNECTION_ABORTED`).
    ConnectionAborted,
    /// Generic connection failure (`net::ERR_CONNECTION_FAILED`).
    ConnectionFailed,
    /// DNS resolution failed (`net::ERR_NAME_NOT_RESOLVED`).
    NameNotResolved,
    /// Network is offline (`net::ERR_INTERNET_DISCONNECTED`).
    InternetDisconnected,
    /// Address could not be reached (`net::ERR_ADDRESS_UNREACHABLE`).
    AddressUnreachable,
    /// Blocked by client policy / extension (`net::ERR_BLOCKED_BY_CLIENT`).
    BlockedByClient,
    /// Blocked by response headers like CSP (`net::ERR_BLOCKED_BY_RESPONSE`).
    BlockedByResponse,
}

impl From<AbortReason> for crate::cdp::browser_protocol::network::ErrorReason {
    fn from(r: AbortReason) -> Self {
        use crate::cdp::browser_protocol::network::ErrorReason;
        match r {
            AbortReason::Failed => ErrorReason::Failed,
            AbortReason::Aborted => ErrorReason::Aborted,
            AbortReason::TimedOut => ErrorReason::TimedOut,
            AbortReason::AccessDenied => ErrorReason::AccessDenied,
            AbortReason::ConnectionClosed => ErrorReason::ConnectionClosed,
            AbortReason::ConnectionReset => ErrorReason::ConnectionReset,
            AbortReason::ConnectionRefused => ErrorReason::ConnectionRefused,
            AbortReason::ConnectionAborted => ErrorReason::ConnectionAborted,
            AbortReason::ConnectionFailed => ErrorReason::ConnectionFailed,
            AbortReason::NameNotResolved => ErrorReason::NameNotResolved,
            AbortReason::InternetDisconnected => ErrorReason::InternetDisconnected,
            AbortReason::AddressUnreachable => ErrorReason::AddressUnreachable,
            AbortReason::BlockedByClient => ErrorReason::BlockedByClient,
            AbortReason::BlockedByResponse => ErrorReason::BlockedByResponse,
        }
    }
}

// ---------------------------------------------------------------------------
// RouteResponse
// ---------------------------------------------------------------------------

/// A synthetic HTTP response used to fulfill an intercepted request.
#[derive(Debug, Default)]
pub struct RouteResponse {
    /// HTTP status code (default: 200).
    pub status: u16,
    /// Response headers as name/value pairs.
    pub headers: Vec<(String, String)>,
    /// Optional response body bytes.
    pub body: Option<Vec<u8>>,
    /// Optional `content-type` header value (appended to `headers` on fulfil).
    pub content_type: Option<String>,
}

impl RouteResponse {
    /// Create a new `RouteResponse` with status 200 and no headers or body.
    pub fn new() -> Self {
        Self { status: 200, ..Default::default() }
    }

    /// Set the HTTP status code.
    #[must_use]
    pub fn status(mut self, s: u16) -> Self {
        self.status = s;
        self
    }

    /// Append a response header.
    #[must_use]
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }

    /// Set the response body.
    #[must_use]
    pub fn body(mut self, b: impl Into<Vec<u8>>) -> Self {
        self.body = Some(b.into());
        self
    }

    /// Set the `content-type` header (convenience wrapper over `header`).
    #[must_use]
    pub fn content_type(mut self, ct: impl Into<String>) -> Self {
        self.content_type = Some(ct.into());
        self
    }
}

// ---------------------------------------------------------------------------
// RouteOverride
// ---------------------------------------------------------------------------

/// Selective overrides applied when continuing a paused request.
///
/// Only the fields you set are sent to the browser — unset fields keep their
/// original values.  Build with the chaining helpers or set fields directly.
///
/// # Example
///
/// ```no_run
/// # async fn example(route: chromist::Route) -> chromist::Result<()> {
/// // Inject an Authorization header and continue.
/// route.continue_with(
///     chromist::RouteOverride::new()
///         .header("Authorization", "Bearer token123")
/// ).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Default)]
pub struct RouteOverride {
    /// Override the request URL.
    pub url: Option<String>,
    /// Override the HTTP method.
    pub method: Option<String>,
    /// Override request headers (replaces the full header set when set).
    pub headers: Option<Vec<(String, String)>>,
    /// Override the POST/PUT body.
    pub post_data: Option<Vec<u8>>,
}

impl RouteOverride {
    /// Create an empty override (no fields set).
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the request URL.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    /// Override the HTTP method.
    #[must_use]
    pub fn method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    /// Append a header to the override set.
    ///
    /// Calling this once switches the request to use the override header list,
    /// so all headers not added here will be dropped.
    #[must_use]
    pub fn header(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.headers.get_or_insert_with(Vec::new).push((k.into(), v.into()));
        self
    }

    /// Override the request body.
    #[must_use]
    pub fn post_data(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.post_data = Some(data.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Route
// ---------------------------------------------------------------------------

/// A handle to a paused `Fetch.requestPaused` request.
///
/// Each method issues a single CDP command that either fulfils, continues, or
/// aborts the request. Only one may be called per paused request; subsequent
/// calls will receive a CDP error.
#[derive(Debug)]
pub struct Route {
    pub(crate) event: cdp_fetch::RequestPausedEvent,
    pub(crate) handle: HandlerHandle,
    pub(crate) session_id: SessionRef,
}

impl Route {
    /// The request URL.
    pub fn url(&self) -> &str {
        &self.event.request.url
    }

    /// The HTTP method (`GET`, `POST`, …).
    pub fn method(&self) -> &str {
        &self.event.request.method
    }

    /// The request headers as a plain `HashMap`.
    pub fn headers(&self) -> HashMap<String, String> {
        self.event
            .request
            .headers
            .inner()
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The raw POST body, if any.
    pub fn post_data(&self) -> Option<&str> {
        self.event.request.post_data.as_deref()
    }

    /// Fulfil the paused request with a synthetic response.
    pub async fn fulfill(&self, response: RouteResponse) -> crate::Result<()> {
        let mut params = cdp_fetch::FulfillRequestParams::new(
            self.event.request_id.clone(),
            i64::from(response.status),
        );

        let mut hdrs: Vec<cdp_fetch::HeaderEntry> =
            response.headers.into_iter().map(|(k, v)| cdp_fetch::HeaderEntry::new(k, v)).collect();
        if let Some(ct) = response.content_type {
            hdrs.push(cdp_fetch::HeaderEntry::new("content-type", ct));
        }
        if !hdrs.is_empty() {
            params.response_headers = Some(hdrs);
        }
        if let Some(body) = response.body {
            params.body = Some(chromist_types::Binary::from_bytes(body));
        }

        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Continue the paused request without modification.
    pub async fn continue_req(&self) -> crate::Result<()> {
        let params = cdp_fetch::ContinueRequestParams::new(self.event.request_id.clone());
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Continue the paused request, applying selective overrides.
    ///
    /// Only the fields set on `overrides` are sent; unset fields retain their
    /// original values.
    ///
    /// # Example — inject an Authorization header
    ///
    /// ```no_run
    /// # async fn example(route: chromist::Route) -> chromist::Result<()> {
    /// route.continue_with(
    ///     chromist::RouteOverride::new()
    ///         .header("Authorization", "Bearer secret")
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn continue_with(&self, overrides: RouteOverride) -> crate::Result<()> {
        let mut params = cdp_fetch::ContinueRequestParams::new(self.event.request_id.clone());
        params.url = overrides.url;
        params.method = overrides.method;
        if let Some(headers) = overrides.headers {
            params.headers =
                Some(headers.into_iter().map(|(k, v)| cdp_fetch::HeaderEntry::new(k, v)).collect());
        }
        if let Some(data) = overrides.post_data {
            params.post_data = Some(chromist_types::Binary::from_bytes(data));
        }
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Abort the paused request with a network error.
    pub async fn abort(&self, reason: AbortReason) -> crate::Result<()> {
        use crate::cdp::browser_protocol::network::ErrorReason;
        let error_reason: ErrorReason = reason.into();
        let params = cdp_fetch::FailRequestParams::new(self.event.request_id.clone(), error_reason);
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Pass this request to the next matching route handler (or continue it if
    /// there are no further handlers).
    ///
    /// In chromist's single-handler architecture this is equivalent to
    /// [`continue_req`](Route::continue_req) — it forwards the request unchanged.
    pub async fn fallback(self) -> crate::Result<()> {
        self.continue_req().await
    }

    /// Perform the original request using an independent HTTP client and return
    /// the response, so you can inspect or modify it before calling
    /// [`fulfill`](Route::fulfill).
    pub async fn fetch(&self) -> crate::Result<RouteResponse> {
        let url = self.url().to_string();
        let method = self.method().to_string();
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|_| CdpError::InvalidMethod(method))?;
        let client = reqwest::Client::new();
        let mut builder = client.request(method, &url);
        for (k, v) in self.headers() {
            builder = builder.header(&k, &v);
        }
        if let Some(body) = self.post_data() {
            builder = builder.body(body.to_string());
        }
        let resp = builder.send().await.map_err(|e| CdpError::RequestFailed(e.to_string()))?;
        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|s| (k.to_string(), s.to_string())))
            .collect();
        let body = resp.bytes().await.map_err(|e| CdpError::RequestFailed(e.to_string()))?.to_vec();
        Ok(RouteResponse { status, headers, body: Some(body), content_type: None })
    }
}

// ---------------------------------------------------------------------------
// RouteHandler type alias
// ---------------------------------------------------------------------------

/// A type-erased async handler for a matched route.
///
/// Using `Arc` rather than `Box` so it can be cloned out of the `Mutex`
/// before the `await` point — holding a `Mutex` guard across `.await` is
/// unsound in async code.
pub(crate) type RouteHandler =
    Arc<dyn Fn(Route) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

// ---------------------------------------------------------------------------
// RouteEntry
// ---------------------------------------------------------------------------

pub(crate) struct RouteEntry {
    pub pattern: String,
    pub handler: RouteHandler,
    pub once: bool,
}

impl std::fmt::Debug for RouteEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RouteEntry")
            .field("pattern", &self.pattern)
            .field("once", &self.once)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// RouteRegistry
// ---------------------------------------------------------------------------

/// A registry of URL-pattern / handler pairs.
///
/// Rules are evaluated in registration order; the first match wins.
/// Unmatched requests must be continued by the caller.
///
/// Cheaply cloneable — all clones share the same underlying `Vec`.
#[derive(Clone, Default)]
pub struct RouteRegistry(Arc<Mutex<Vec<RouteEntry>>>);

impl std::fmt::Debug for RouteRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.0.lock().map(|g| g.len()).unwrap_or(0);
        f.debug_struct("RouteRegistry").field("rules", &count).finish()
    }
}

impl RouteRegistry {
    /// Create a fresh empty registry.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    /// Append a new rule.
    pub fn add(&self, pattern: impl Into<String>, handler: RouteHandler, once: bool) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(RouteEntry {
            pattern: pattern.into(),
            handler,
            once,
        });
    }

    /// Remove all rules whose pattern equals `pattern`.
    pub fn remove(&self, pattern: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).retain(|e| e.pattern != pattern);
    }

    /// Return `true` when no rules are registered.
    pub fn is_empty(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// Find the first matching rule and call its handler.
    ///
    /// Returns `true` if a handler was found and invoked, `false` if no rule
    /// matched.  The `Mutex` is released before the handler is awaited.
    pub async fn dispatch(&self, route: Route) -> bool {
        let url = route.url().to_string();

        // Extract handler + index while holding the lock, then drop the lock
        // before the await so we never hold a Mutex guard across an await point.
        let (handler, idx, once) = {
            let entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
            match entries.iter().enumerate().find(|(_, e)| glob_matches(&e.pattern, &url)) {
                Some((i, e)) => (Arc::clone(&e.handler), i, e.once),
                None => return false,
            }
        };

        if once {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(idx);
        }

        handler(route).await;
        true
    }
}

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

/// Match `url` against a glob `pattern`.
///
/// - `*`  — wildcard within a single path segment (no `/`)
/// - `**` — wildcard across zero or more path segments
/// - If `pattern` contains no scheme (`://`), the scheme is stripped from
///   `url` before matching.
pub(crate) fn glob_matches(pattern: &str, url: &str) -> bool {
    match (pattern.split_once("://"), url.split_once("://")) {
        // Both have a scheme: match scheme exactly then match the rest.
        (Some((pscheme, prest)), Some((uscheme, urest))) => {
            pscheme == uscheme && glob_match_recursive(prest, urest)
        }
        // Pattern has no scheme: strip the scheme from the url and match.
        (None, Some((_uscheme, urest))) => glob_match_recursive(pattern, urest),
        // No scheme in either (or scheme in pattern but not url): direct match.
        _ => glob_match_recursive(pattern, url),
    }
}

fn glob_match_recursive(pat: &str, s: &str) -> bool {
    if pat.is_empty() {
        return s.is_empty();
    }
    if pat == "**" {
        return true;
    }

    // Handle leading `**/`
    if let Some(rest_pat) = pat.strip_prefix("**/") {
        if glob_match_recursive(rest_pat, s) {
            return true;
        }
        // Try consuming one segment at a time from `s`
        for (i, ch) in s.char_indices() {
            if ch == '/' && glob_match_recursive(rest_pat, &s[i + 1..]) {
                return true;
            }
        }
        return false;
    }

    // Split both sides at the first `/`
    let (pat_seg, pat_rest) = match pat.find('/') {
        Some(i) => (&pat[..i], &pat[i + 1..]),
        None => (pat, ""),
    };
    let (s_seg, s_rest) = match s.find('/') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };

    if pat_seg == "**" {
        // `**` as a bare segment: try matching 0 or more URL segments.
        if glob_match_recursive(pat_rest, s) {
            return true;
        }
        if !s_rest.is_empty() {
            return glob_match_recursive(pat, s_rest);
        }
        return false;
    }

    if segment_matches(pat_seg, s_seg) {
        if pat_rest.is_empty() && s_rest.is_empty() {
            return true;
        }
        if pat_rest.is_empty() {
            // Pattern is exhausted but URL still has segments.
            return false;
        }
        if s_rest.is_empty() {
            // URL is exhausted; only match if the rest of the pattern can match empty.
            return glob_match_recursive(pat_rest, s_rest);
        }
        return glob_match_recursive(pat_rest, s_rest);
    }

    false
}

/// Match a single path segment (no `/`) against a pattern that may contain `*`.
fn segment_matches(pat: &str, s: &str) -> bool {
    let parts: Vec<&str> = pat.split('*').collect();
    if parts.len() == 1 {
        return pat == s;
    }
    let mut pos = 0usize;
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !s.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if i == last {
            return s[pos..].ends_with(part);
        } else if part.is_empty() {
            // consecutive `*` — skip
        } else if let Some(idx) = s[pos..].find(part) {
            pos += idx + part.len();
        } else {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        assert!(glob_matches("https://example.com/path", "https://example.com/path"));
    }

    #[test]
    fn exact_no_match() {
        assert!(!glob_matches("https://example.com/foo", "https://example.com/bar"));
    }

    #[test]
    fn star_matches_segment() {
        assert!(glob_matches("https://example.com/*", "https://example.com/foo"));
    }

    #[test]
    fn star_does_not_cross_slash() {
        assert!(!glob_matches("https://example.com/*", "https://example.com/foo/bar"));
    }

    #[test]
    fn double_star_crosses_slash() {
        assert!(glob_matches("https://example.com/**", "https://example.com/foo/bar"));
    }

    #[test]
    fn no_scheme_in_pattern_strips_scheme_from_url() {
        assert!(glob_matches("example.com/foo", "https://example.com/foo"));
    }

    #[test]
    fn star_in_middle_of_segment() {
        assert!(glob_matches(
            "https://example.com/api/*.json",
            "https://example.com/api/data.json"
        ));
        assert!(!glob_matches(
            "https://example.com/api/*.json",
            "https://example.com/api/data.xml"
        ));
    }

    #[test]
    fn double_star_zero_segments() {
        assert!(glob_matches("https://example.com/**", "https://example.com/"));
    }
}
