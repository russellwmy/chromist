//! HTTP request/response tracking derived from CDP `Network.*` events.
//!
//! [`NetworkManager`] listens to `Network.requestWillBeSent`,
//! `responseReceived`, `loadingFailed`, and `loadingFinished` to build a
//! coherent [`HttpRequest`] record per CDP `RequestId`. Records are kept
//! behind a `Mutex<HashMap>` so callers can snapshot in-flight or completed
//! requests at any time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};

use crate::cdp::browser_protocol::network::{
    self as cdp_network, LoadingFailedEvent, LoadingFinishedEvent, RequestId,
    RequestServedFromCacheEvent, RequestWillBeSentEvent, ResponseReceivedEvent,
};
use serde::Deserialize as _;

use crate::error::CdpError;
use crate::handler::HandlerHandle;

/// An optional reference-counted HTTP request.
pub type ArcHttpRequest = Option<Arc<HttpRequest>>;

/// Summary of request / response byte sizes derived from CDP events.
#[derive(Debug, Clone, Default)]
pub struct RequestSizes {
    /// Approximate request body size (length of `post_data` in bytes).
    pub request_body_size: Option<usize>,
    /// Total encoded (wire) bytes for the response, from `Network.loadingFinished`.
    pub encoded_response_length: Option<f64>,
}

/// A captured HTTP request with metadata.
#[derive(Clone)]
pub struct HttpRequest {
    /// CDP `Network.RequestId` — handle for follow-up commands.
    pub request_id: crate::cdp::browser_protocol::network::RequestId,
    /// Full request URL including scheme and query string.
    pub url: String,
    /// HTTP method (`GET`, `POST`, …).
    pub method: String,
    /// Request headers as case-insensitive name → value map.
    pub headers: std::collections::HashMap<String, String>,
    /// Raw POST body, if any. UTF-8 strings only — see CDP `postDataEntries`
    /// for binary bodies.
    pub post_data: Option<String>,
    /// Frame that initiated the request.
    pub frame_id: Option<crate::cdp::browser_protocol::page::FrameId>,
    /// Server response, populated once `Network.responseReceived` fires.
    pub response: Option<crate::cdp::browser_protocol::network::Response>,
    /// Set on `Network.loadingFailed` with the renderer's failure reason.
    pub failure_text: Option<String>,
    /// `true` when the response was served from the browser's memory cache.
    pub from_memory_cache: bool,
    /// Fetch domain interception ID, present when the request was intercepted via `Fetch.enable`.
    pub interception_id: Option<crate::cdp::browser_protocol::fetch::RequestId>,
    /// `true` when the request is a top-level navigation (vs sub-resource).
    pub is_navigation_request: bool,
    /// `true` when this request is currently paused for interception.
    pub allow_interception: bool,
    /// CDP resource classification (Document, XHR, Fetch, Stylesheet, …).
    pub resource_type: Option<crate::cdp::browser_protocol::network::ResourceType>,
    /// Chain of redirects that led to this request.
    pub redirect_chain: Vec<std::sync::Arc<HttpRequest>>,
    /// Monotonic timestamp of the originating `requestWillBeSent` event, if known.
    pub timestamp: Option<f64>,
    /// Total encoded response bytes captured from `Network.loadingFinished`.
    pub encoded_response_length: Option<f64>,
    /// Handle for self-fetching the response body via `Network.getResponseBody`.
    pub(crate) handle: Option<HandlerHandle>,
    /// CDP session ID this request was captured on.
    pub(crate) session_id: Option<Arc<str>>,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpRequest")
            .field("request_id", &self.request_id)
            .field("url", &self.url)
            .field("method", &self.method)
            .field("resource_type", &self.resource_type)
            .field("is_navigation_request", &self.is_navigation_request)
            .finish_non_exhaustive()
    }
}

/// Parsed response from `/json/version`.
fn headers_to_map(h: &crate::cdp::browser_protocol::network::Headers) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(obj) = h.0.as_object() {
        for (k, v) in obj {
            let val = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out.insert(k.clone(), val);
        }
    }
    out
}

fn build_http_request(
    ev: RequestWillBeSentEvent,
    handle: HandlerHandle,
    session_id: Arc<str>,
) -> HttpRequest {
    HttpRequest {
        request_id: ev.request_id,
        url: ev.request.url.clone(),
        method: ev.request.method.clone(),
        headers: headers_to_map(&ev.request.headers),
        post_data: ev.request.post_data.clone(),
        frame_id: ev.frame_id,
        response: None,
        failure_text: None,
        from_memory_cache: false,
        interception_id: None,
        is_navigation_request: false,
        allow_interception: false,
        resource_type: ev.r#type,
        redirect_chain: Vec::new(),
        timestamp: Some(*ev.timestamp.inner()),
        encoded_response_length: None,
        handle: Some(handle),
        session_id: Some(session_id),
    }
}

impl HttpRequest {
    /// All captured request headers (from `Network.requestWillBeSent`).
    ///
    /// Headers added by the browser after the event fires (e.g. `Cookie`,
    /// `Content-Length`) may not be present; use Fetch interception to capture
    /// those reliably.
    pub fn all_headers(&self) -> &HashMap<String, String> {
        &self.headers
    }

    /// All response headers, or `None` if no response has arrived yet.
    pub fn response_all_headers(&self) -> Option<HashMap<String, String>> {
        self.response.as_ref().map(|r| headers_to_map(&r.headers))
    }

    /// Request headers as an ordered array of `(name, value)` pairs.
    pub fn headers_array(&self) -> Vec<(String, String)> {
        self.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Response headers as an ordered array, or `None` if no response yet.
    pub fn response_headers_array(&self) -> Option<Vec<(String, String)>> {
        self.response.as_ref().map(|r| headers_to_map(&r.headers).into_iter().collect())
    }

    /// The resource type for the request (e.g. `Document`, `XHR`, `Fetch`).
    pub fn resource_type(&self) -> Option<&crate::cdp::browser_protocol::network::ResourceType> {
        self.resource_type.as_ref()
    }

    /// Whether this is a navigation request.
    pub fn is_navigation_request(&self) -> bool {
        self.is_navigation_request
    }

    /// The frame ID that initiated the request.
    pub fn frame_id(&self) -> Option<&crate::cdp::browser_protocol::page::FrameId> {
        self.frame_id.as_ref()
    }

    /// Request/response byte sizes derived from CDP events.
    pub fn sizes(&self) -> RequestSizes {
        RequestSizes {
            request_body_size: self.post_data.as_ref().map(|s| s.len()),
            encoded_response_length: self.encoded_response_length,
        }
    }

    /// TLS security details from the response, if available.
    pub fn security_details(
        &self,
    ) -> Option<&crate::cdp::browser_protocol::network::SecurityDetails> {
        self.response.as_ref().and_then(|r| r.security_details.as_ref())
    }

    /// Response timing breakdown, if available.
    pub fn timing(&self) -> Option<&crate::cdp::browser_protocol::network::ResourceTiming> {
        self.response.as_ref().and_then(|r| r.timing.as_ref())
    }

    /// Fetch the raw response body bytes via `Network.getResponseBody`.
    ///
    /// Requires the request to have finished loading and the `NetworkManager`
    /// to have been active when the request was captured.
    pub async fn body(&self) -> crate::Result<Vec<u8>> {
        let handle = self.handle.as_ref().ok_or(CdpError::NotFound)?;
        let session_id = self.session_id.clone();
        let params = crate::cdp::browser_protocol::network::GetResponseBodyParams::new(
            self.request_id.clone(),
        );
        let resp = handle.execute(params, session_id).await?;
        if resp.base64_encoded {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(resp.body.as_bytes())
                .map_err(|_| CdpError::RequestFailed("base64 decode error".into()))
        } else {
            Ok(resp.body.into_bytes())
        }
    }

    /// Fetch the response body as a UTF-8 string.
    pub async fn text(&self) -> crate::Result<String> {
        let bytes = self.body().await?;
        String::from_utf8(bytes)
            .map_err(|_| CdpError::RequestFailed("response body is not valid UTF-8".into()))
    }

    /// Fetch and parse the response body as JSON.
    pub async fn json(&self) -> crate::Result<serde_json::Value> {
        let text = self.text().await?;
        serde_json::from_str(&text)
            .map_err(|e| CdpError::RequestFailed(format!("JSON parse error: {e}")))
    }

    /// Returns `Ok(())` once the response has finished loading (i.e. `encoded_response_length`
    /// is populated), or `Err` if the request failed.
    pub fn finished(&self) -> crate::Result<()> {
        if self.failure_text.is_some() {
            return Err(CdpError::RequestFailed(self.failure_text.clone().unwrap_or_default()));
        }
        if self.encoded_response_length.is_some() {
            Ok(())
        } else {
            Err(CdpError::ResponseNotReady)
        }
    }
}

/// Reactive accumulator for in-flight HTTP requests on one CDP session.
///
/// Subscribes to the `Network` domain's lifecycle events and exposes streams
/// for freshly observed requests, completed responses, and failed requests.
/// Under the hood a single background task multiplexes the raw event stream
/// into three bounded output channels and an indexed map keyed by `RequestId`
/// so callers can look up a request by ID.
///
/// # Redirect handling
/// When a `requestWillBeSent` carries a `redirectResponse`, the old entry for
/// that request id is treated as completed (its response is attached and it
/// is pushed onto the new request's `redirect_chain`), then the new request
/// replaces it in the map.
#[derive(Clone)]
pub struct NetworkManager {
    inner: Arc<NetworkManagerInner>,
}

impl std::fmt::Debug for NetworkManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkManager")
            .field("session_id", &self.inner.session_id)
            .finish_non_exhaustive()
    }
}

struct NetworkManagerInner {
    handle: HandlerHandle,
    session_id: Arc<str>,
    requests: Mutex<HashMap<RequestId, Arc<HttpRequest>>>,
    // Fan-out channels. Each new call to `*_stream` replaces the sender
    // half so only the latest subscriber receives — matches how other
    // chromist helpers behave and keeps the implementation simple.
    on_request: Mutex<Option<mpsc::Sender<Arc<HttpRequest>>>>,
    on_response: Mutex<Option<mpsc::Sender<Arc<HttpRequest>>>>,
    on_failed: Mutex<Option<mpsc::Sender<Arc<HttpRequest>>>>,
    /// Aborts the background dispatcher when the last `NetworkManager` clone
    /// (and therefore the last `Arc<NetworkManagerInner>`) is dropped.
    dispatcher: Mutex<Option<crate::runtime::AbortOnDrop>>,
}

impl NetworkManager {
    /// Construct a manager and start its background dispatcher task.
    #[tracing::instrument(skip(handle), fields(session_id = %session_id))]
    pub fn new(handle: HandlerHandle, session_id: Arc<str>) -> Self {
        let inner = Arc::new(NetworkManagerInner {
            handle: handle.clone(),
            session_id: session_id.clone(),
            requests: Mutex::new(HashMap::new()),
            on_request: Mutex::new(None),
            on_response: Mutex::new(None),
            on_failed: Mutex::new(None),
            dispatcher: Mutex::new(None),
        });
        let mgr = NetworkManager { inner };
        mgr.spawn_dispatcher();
        mgr
    }

    /// Enable the `Network` domain for this session.
    pub async fn enable(&self) -> crate::Result<()> {
        self.inner
            .handle
            .execute(cdp_network::EnableParams::default(), Some(self.inner.session_id.clone()))
            .await?;
        Ok(())
    }

    /// Look up a request by its CDP `RequestId`.
    pub fn request(&self, request_id: &RequestId) -> Option<Arc<HttpRequest>> {
        match self.inner.requests.lock() {
            Ok(m) => m.get(request_id).cloned(),
            Err(_) => {
                tracing::warn!("NetworkManager requests lock poisoned in request lookup");
                None
            }
        }
    }

    /// Stream of newly observed requests (emitted at `requestWillBeSent`).
    ///
    /// Single-subscriber: calling this twice replaces the previous receiver,
    /// dropping any events the prior subscriber had not yet read. A warning
    /// is logged when this happens.
    pub fn request_stream(&self) -> mpsc::Receiver<Arc<HttpRequest>> {
        let (tx, rx) = mpsc::channel(64);
        if let Ok(mut slot) = self.inner.on_request.lock() {
            if slot.is_some() {
                tracing::warn!(
                    "NetworkManager::request_stream replaced an existing subscriber; \
                     prior receiver will stop seeing events"
                );
            }
            *slot = Some(tx);
        }
        rx
    }

    /// Stream of requests that received a response (emitted at
    /// `responseReceived` / `loadingFinished`).
    ///
    /// Single-subscriber: see [`Self::request_stream`].
    pub fn response_stream(&self) -> mpsc::Receiver<Arc<HttpRequest>> {
        let (tx, rx) = mpsc::channel(64);
        if let Ok(mut slot) = self.inner.on_response.lock() {
            if slot.is_some() {
                tracing::warn!("NetworkManager::response_stream replaced an existing subscriber");
            }
            *slot = Some(tx);
        }
        rx
    }

    /// Snapshot of all currently in-flight requests (not yet completed or failed).
    pub fn requests_snapshot(&self) -> Vec<Arc<HttpRequest>> {
        match self.inner.requests.lock() {
            Ok(m) => m.values().cloned().collect(),
            Err(_) => {
                tracing::warn!(
                    "NetworkManager requests lock poisoned in requests_snapshot; \
                     returning empty snapshot"
                );
                Vec::new()
            }
        }
    }

    /// Fetch the raw response body bytes for `req`.
    ///
    /// Calls `Network.getResponseBody`; the request must have finished loading.
    pub async fn response_body(&self, req: &HttpRequest) -> crate::Result<Vec<u8>> {
        let params = cdp_network::GetResponseBodyParams::new(req.request_id.clone());
        let resp = self.inner.handle.execute(params, Some(self.inner.session_id.clone())).await?;
        if resp.base64_encoded {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(resp.body.as_bytes())
                .map_err(|_| CdpError::RequestFailed("base64 decode error".into()))
        } else {
            Ok(resp.body.into_bytes())
        }
    }

    /// Fetch the response body as a UTF-8 string.
    pub async fn response_text(&self, req: &HttpRequest) -> crate::Result<String> {
        let bytes = self.response_body(req).await?;
        String::from_utf8(bytes)
            .map_err(|_| CdpError::RequestFailed("response body is not valid UTF-8".into()))
    }

    /// Fetch and parse the response body as JSON.
    pub async fn response_json(&self, req: &HttpRequest) -> crate::Result<serde_json::Value> {
        let text = self.response_text(req).await?;
        serde_json::from_str(&text)
            .map_err(|e| CdpError::RequestFailed(format!("JSON parse error: {e}")))
    }

    /// Stream of failed requests (emitted at `loadingFailed`).
    ///
    /// Single-subscriber: see [`Self::request_stream`].
    pub fn failed_stream(&self) -> mpsc::Receiver<Arc<HttpRequest>> {
        let (tx, rx) = mpsc::channel(64);
        if let Ok(mut slot) = self.inner.on_failed.lock() {
            if slot.is_some() {
                tracing::warn!("NetworkManager::failed_stream replaced an existing subscriber");
            }
            *slot = Some(tx);
        }
        rx
    }

    fn spawn_dispatcher(&self) {
        let inner = Arc::clone(&self.inner);
        let mut events = inner.handle.subscribe(Some(inner.session_id.clone()));
        let task = crate::runtime::spawn(async move {
            while let Some(frame) = events.next().await {
                match frame.method.as_str() {
                    "Network.requestWillBeSent" => {
                        match RequestWillBeSentEvent::deserialize(&frame.params) {
                            Ok(ev) => handle_will_be_sent(&inner, ev).await,
                            Err(e) => {
                                tracing::warn!(method = "Network.requestWillBeSent", error = %e, "event deserialization failed")
                            }
                        }
                    }
                    "Network.responseReceived" => {
                        match ResponseReceivedEvent::deserialize(&frame.params) {
                            Ok(ev) => handle_response(&inner, ev).await,
                            Err(e) => {
                                tracing::warn!(method = "Network.responseReceived", error = %e, "event deserialization failed")
                            }
                        }
                    }
                    "Network.loadingFinished" => {
                        match LoadingFinishedEvent::deserialize(&frame.params) {
                            Ok(ev) => handle_finished(&inner, ev).await,
                            Err(e) => {
                                tracing::warn!(method = "Network.loadingFinished", error = %e, "event deserialization failed")
                            }
                        }
                    }
                    "Network.loadingFailed" => {
                        match LoadingFailedEvent::deserialize(&frame.params) {
                            Ok(ev) => handle_failed(&inner, ev).await,
                            Err(e) => {
                                tracing::warn!(method = "Network.loadingFailed", error = %e, "event deserialization failed")
                            }
                        }
                    }
                    "Network.requestServedFromCache" => {
                        match RequestServedFromCacheEvent::deserialize(&frame.params) {
                            Ok(ev) => handle_cached(&inner, ev),
                            Err(e) => {
                                tracing::warn!(method = "Network.requestServedFromCache", error = %e, "event deserialization failed")
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
        if let Ok(mut slot) = self.inner.dispatcher.lock() {
            *slot = Some(crate::runtime::AbortOnDrop(task));
        }
    }
}

async fn emit(slot: &Mutex<Option<mpsc::Sender<Arc<HttpRequest>>>>, req: &Arc<HttpRequest>) {
    let sender = match slot.lock() {
        Ok(mut g) => g.as_mut().cloned(),
        Err(_) => {
            tracing::warn!("network emit: HTTP-request channel lock poisoned; dropping event");
            return;
        }
    };
    if let Some(mut s) = sender {
        let _ = s.send(Arc::clone(req)).await;
    }
}

async fn handle_will_be_sent(inner: &Arc<NetworkManagerInner>, ev: RequestWillBeSentEvent) {
    let redirect_response = ev.redirect_response.clone();
    let request_id = ev.request_id.clone();
    let mut new_req = build_http_request(ev, inner.handle.clone(), Arc::clone(&inner.session_id));

    // Collect previous entry for this id if this is a redirect.
    if let Some(resp) = redirect_response {
        let prior = match inner.requests.lock() {
            Ok(mut m) => m.remove(&request_id),
            Err(_) => {
                tracing::warn!("NetworkManager requests lock poisoned in handle_will_be_sent");
                None
            }
        };
        if let Some(prior_arc) = prior {
            // Produce a completed version of the prior request with the redirect response.
            let mut prior_inner: HttpRequest = (*prior_arc).clone();
            prior_inner.response = Some(resp);
            let completed = Arc::new(prior_inner);
            // Extend chain: prior's chain plus prior itself.
            new_req.redirect_chain = completed.redirect_chain.clone();
            new_req.redirect_chain.push(Arc::clone(&completed));
            emit(&inner.on_response, &completed).await;
        }
    }

    let arc = Arc::new(new_req);
    if let Ok(mut map) = inner.requests.lock() {
        map.insert(request_id, Arc::clone(&arc));
    }
    emit(&inner.on_request, &arc).await;
}

async fn handle_response(inner: &Arc<NetworkManagerInner>, ev: ResponseReceivedEvent) {
    let updated = match inner.requests.lock() {
        Ok(mut map) => {
            if let Some(arc) = map.get_mut(&ev.request_id) {
                let m = Arc::make_mut(arc);
                m.response = Some(ev.response);
                if m.resource_type.is_none() {
                    m.resource_type = Some(ev.r#type);
                }
                Some(Arc::clone(arc))
            } else {
                None
            }
        }
        Err(_) => {
            tracing::warn!("NetworkManager requests lock poisoned in handle_response");
            None
        }
    };
    if let Some(arc) = updated {
        emit(&inner.on_response, &arc).await;
    }
}

async fn handle_finished(inner: &Arc<NetworkManagerInner>, ev: LoadingFinishedEvent) {
    let arc = match inner.requests.lock() {
        Ok(mut m) => {
            if let Some(arc) = m.get_mut(&ev.request_id) {
                Arc::make_mut(arc).encoded_response_length = Some(ev.encoded_data_length);
                Some(Arc::clone(arc))
            } else {
                None
            }
        }
        Err(_) => {
            tracing::warn!("NetworkManager requests lock poisoned in handle_finished");
            None
        }
    };
    if let Some(arc) = arc {
        emit(&inner.on_response, &arc).await;
    }
}

async fn handle_failed(inner: &Arc<NetworkManagerInner>, ev: LoadingFailedEvent) {
    let arc = match inner.requests.lock() {
        Ok(mut map) => {
            if let Some(arc) = map.get_mut(&ev.request_id) {
                Arc::make_mut(arc).failure_text = Some(ev.error_text);
                Some(Arc::clone(arc))
            } else {
                None
            }
        }
        Err(_) => {
            tracing::warn!("NetworkManager requests lock poisoned in handle_failed");
            None
        }
    };
    if let Some(arc) = arc {
        emit(&inner.on_failed, &arc).await;
    }
}

fn handle_cached(inner: &Arc<NetworkManagerInner>, ev: RequestServedFromCacheEvent) {
    if let Ok(mut map) = inner.requests.lock() {
        if let Some(arc) = map.get_mut(&ev.request_id) {
            Arc::make_mut(arc).from_memory_cache = true;
        }
    }
}

/// Parsed `/json/version` response from a browser's DevTools HTTP endpoint.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BrowserConnection {
    /// Browser product name, e.g. `"HeadlessChrome/120.0.6099.71"`.
    #[serde(rename = "Browser")]
    pub browser: String,
    /// CDP protocol revision the browser implements.
    #[serde(rename = "Protocol-Version")]
    pub protocol_version: String,
    /// User-Agent string the browser will send.
    #[serde(rename = "User-Agent")]
    pub user_agent: String,
    /// V8 engine version, when reported.
    #[serde(rename = "V8-Version", default)]
    pub v8_version: String,
    /// WebKit revision string.
    #[serde(rename = "WebKit-Version", default)]
    pub webkit_version: String,
    /// WebSocket URL chromist should connect to for a CDP session.
    #[serde(rename = "webSocketDebuggerUrl")]
    pub web_socket_debugger_url: String,
}
