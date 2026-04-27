//! Full bidirectional WebSocket interception via JS shim + CDP bindings.
//!
//! [`Page::route_websocket`] injects a JS shim that overrides `window.WebSocket`
//! for matching URLs, bridges page events to Rust via `Runtime.addBinding`, and
//! lets the Rust handler drive the fake WebSocket via `Runtime.evaluate`.

use std::sync::Arc;

use futures::StreamExt;

use super::Page;
use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::handler::HandlerHandle;

/// Boxed async handler type for `route_websocket`.
pub type WsRouteHandler = Arc<
    dyn Fn(WebSocketRoute) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

// ---------------------------------------------------------------------------
// JS shim
// ---------------------------------------------------------------------------

/// Injected once per page. Overrides `window.WebSocket` so any URL that
/// matches an entry in `window.__chromistWsPatterns` is intercepted rather
/// than connecting to the real server.
///
/// Design notes
/// ─────────────
/// * Idempotent: a guard at the top prevents double-installation.
/// * Patterns are read at `new WebSocket(url)` time, not at injection time, so
///   patterns pushed after injection still take effect.
/// * Three one-way bindings bridge page→Rust:
///   - `__chromistWsCreate(json)` — `{id, url}`  fired on construction
///   - `__chromistWsSend(json)`   — `{id, data}` fired on `fake.send()`
///   - `__chromistWsClose(json)`  — `{id, code, reason}` fired on `fake.close()`
/// * `window.__chromistWsDispatch(json)` bridges Rust→page:
///   - `{type:'open', id}`
///   - `{type:'message', id, data}`
///   - `{type:'close', id, code, reason}`
pub(crate) const WS_SHIM: &str = r#"(function() {
  if (window.__chromistWsInstalled) return;
  window.__chromistWsInstalled = true;

  var _NativeWS = window.WebSocket;
  var _fakes = {};
  var _nextId = 1;

  function globMatch(pattern, url) {
    var escaped = pattern.replace(/[.+^${}()|[\]\\]/g, '\\$&');
    var reStr = escaped.replace(/\*\*/g, '￿').replace(/\*/g, '[^/]*').replace(/￿/g, '.*');
    return new RegExp('^' + reStr + '$').test(url);
  }

  function matchesAny(url) {
    var patterns = window.__chromistWsPatterns || [];
    for (var i = 0; i < patterns.length; i++) {
      if (globMatch(patterns[i], url)) return true;
    }
    return false;
  }

  window.__chromistWsDispatch = function(msgJson) {
    var msg;
    try { msg = JSON.parse(msgJson); } catch(e) { return; }
    var fake = _fakes[msg.id];
    if (!fake) return;
    if (msg.type === 'open') {
      fake.readyState = 1;
      var ev = new Event('open');
      if (typeof fake.onopen === 'function') fake.onopen(ev);
      fake._listeners.open.forEach(function(fn) { fn(ev); });
    } else if (msg.type === 'message') {
      var mev = new MessageEvent('message', {data: msg.data});
      if (typeof fake.onmessage === 'function') fake.onmessage(mev);
      fake._listeners.message.forEach(function(fn) { fn(mev); });
    } else if (msg.type === 'close') {
      fake.readyState = 3;
      var cev = new CloseEvent('close', {code: msg.code || 1000, reason: msg.reason || '', wasClean: true});
      if (typeof fake.onclose === 'function') fake.onclose(cev);
      fake._listeners.close.forEach(function(fn) { fn(cev); });
      delete _fakes[fake._id];
    } else if (msg.type === 'error') {
      var eev = new Event('error');
      if (typeof fake.onerror === 'function') fake.onerror(eev);
      fake._listeners.error.forEach(function(fn) { fn(eev); });
    }
  };

  function FakeWebSocket(url, protocols) {
    var self = this;
    self._id = _nextId++;
    self.url = url;
    self.readyState = 0; // CONNECTING
    self.bufferedAmount = 0;
    self.extensions = '';
    self.protocol = '';
    self.binaryType = 'blob';
    self.onopen = null;
    self.onmessage = null;
    self.onclose = null;
    self.onerror = null;
    self._listeners = {open:[], message:[], close:[], error:[]};
    _fakes[self._id] = self;
    window.__chromistWsCreate && window.__chromistWsCreate(JSON.stringify({id: self._id, url: url}));
  }

  FakeWebSocket.prototype.send = function(data) {
    window.__chromistWsSend && window.__chromistWsSend(JSON.stringify({id: this._id, data: data}));
  };

  FakeWebSocket.prototype.close = function(code, reason) {
    this.readyState = 2; // CLOSING
    window.__chromistWsClose && window.__chromistWsClose(JSON.stringify({id: this._id, code: code || 1000, reason: reason || ''}));
  };

  FakeWebSocket.prototype.addEventListener = function(type, fn) {
    if (this._listeners[type]) this._listeners[type].push(fn);
  };

  FakeWebSocket.prototype.removeEventListener = function(type, fn) {
    if (this._listeners[type]) {
      this._listeners[type] = this._listeners[type].filter(function(f) { return f !== fn; });
    }
  };

  FakeWebSocket.CONNECTING = 0;
  FakeWebSocket.OPEN = 1;
  FakeWebSocket.CLOSING = 2;
  FakeWebSocket.CLOSED = 3;

  window.WebSocket = function(url, protocols) {
    if (matchesAny(url)) {
      return new FakeWebSocket(url, protocols);
    }
    return new _NativeWS(url, protocols);
  };
  window.WebSocket.prototype = _NativeWS.prototype;
  window.WebSocket.CONNECTING = 0;
  window.WebSocket.OPEN = 1;
  window.WebSocket.CLOSING = 2;
  window.WebSocket.CLOSED = 3;
})();"#;

// ---------------------------------------------------------------------------
// WebSocketRoute
// ---------------------------------------------------------------------------

/// Handle to a fake intercepted WebSocket connection.
///
/// Obtained from the handler passed to [`Page::route_websocket`].  Each method
/// dispatches to the matching fake WS object inside the page via
/// `window.__chromistWsDispatch`.
#[derive(Clone)]
pub struct WebSocketRoute {
    /// Stable numeric ID matching `FakeWebSocket._id` in the page.
    pub id: u64,
    /// The URL the page passed to `new WebSocket(url)`.
    pub url: String,
    pub(crate) session_id: Arc<str>,
    pub(crate) handle: HandlerHandle,
}

impl std::fmt::Debug for WebSocketRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketRoute").field("id", &self.id).field("url", &self.url).finish()
    }
}

impl WebSocketRoute {
    /// The URL the page passed to `new WebSocket(url)`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Dispatch an `open` event to the fake WS — call this to "accept" the
    /// connection.  Sets `readyState = 1` and fires `onopen`.
    pub async fn accept(&self) -> crate::Result<()> {
        let msg = serde_json::json!({"type": "open", "id": self.id});
        self.dispatch(msg).await
    }

    /// Send a `message` event to the page (data flows server→page).
    pub async fn send(&self, data: impl Into<String>) -> crate::Result<()> {
        let msg = serde_json::json!({"type": "message", "id": self.id, "data": data.into()});
        self.dispatch(msg).await
    }

    /// Send a `close` event to the page.  Sets `readyState = 3`.
    pub async fn close(&self, code: u16, reason: impl Into<String>) -> crate::Result<()> {
        let msg = serde_json::json!({"type": "close", "id": self.id, "code": code, "reason": reason.into()});
        self.dispatch(msg).await
    }

    /// Send an `error` event to the page.
    pub async fn error(&self) -> crate::Result<()> {
        let msg = serde_json::json!({"type": "error", "id": self.id});
        self.dispatch(msg).await
    }

    /// Low-level: evaluate `window.__chromistWsDispatch(msgJson)` in the page.
    async fn dispatch(&self, msg: serde_json::Value) -> crate::Result<()> {
        // serde_json::to_string produces a valid JSON string literal including
        // surrounding quotes and all special-char escaping.
        // The `?` operator uses the `From<serde_json::Error>` impl on `CdpError`.
        let json_str = serde_json::to_string(&msg)?;
        let expr =
            format!("window.__chromistWsDispatch && window.__chromistWsDispatch({json_str})");
        let mut params = cdp_runtime::EvaluateParams::new(expr);
        params.return_by_value = Some(false);
        self.handle.execute(params, Some(self.session_id.clone())).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Page::route_websocket
// ---------------------------------------------------------------------------

impl Page {
    /// Intercept WebSocket connections whose URL matches `url_pattern`.
    ///
    /// `url_pattern` is a glob where `*` matches within a path segment and
    /// `**` matches across segments.
    ///
    /// The `handler` is called once per matching `new WebSocket(url)` with a
    /// [`WebSocketRoute`] that lets you [`accept`](WebSocketRoute::accept),
    /// [`send`](WebSocketRoute::send), and [`close`](WebSocketRoute::close)
    /// the fake connection.
    ///
    /// The listener is owned by the page and aborted when the last `Page`
    /// clone is dropped, mirroring [`Page::route`](crate::Page::route).
    pub async fn route_websocket<F, Fut>(
        &self,
        url_pattern: impl Into<String>,
        handler: F,
    ) -> crate::Result<()>
    where
        F: Fn(WebSocketRoute) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let handler: WsRouteHandler = Arc::new(move |r| Box::pin(handler(r)));
        self.setup_ws_shim(url_pattern.into()).await?;
        self.register_listener_task(self.spawn_ws_listener(handler));
        Ok(())
    }

    /// Install the WS shim + bindings for one pattern.
    ///
    /// Safe to call multiple times on the same page: the JS shim is
    /// idempotent (guarded by `window.__chromistWsInstalled`), and
    /// `Runtime.addBinding` is idempotent on the CDP side.
    pub(crate) async fn setup_ws_shim(&self, pattern: String) -> crate::Result<()> {
        let session = self.session_id.current();

        // Register the three one-way bindings (page→Rust). If any of these
        // fail the shim is non-functional; surface a warning so the user is
        // not left wondering why interception silently does nothing.
        for name in ["__chromistWsCreate", "__chromistWsSend", "__chromistWsClose"] {
            if let Err(e) = self
                .handle
                .execute(cdp_runtime::AddBindingParams::new(name), Some(session.clone()))
                .await
            {
                tracing::warn!(binding = name, error = %e, "WebSocket shim: AddBinding failed");
            }
        }

        // Inject the base shim for new documents.
        let mut shim_params =
            crate::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams::new(
                WS_SHIM.to_string(),
            );
        shim_params.world_name = None;
        if let Err(e) = self.execute(shim_params).await {
            tracing::warn!(error = %e, "WebSocket shim: base script injection failed");
        }

        // Push this pattern for new documents.
        let push_script = format!(
            "(window.__chromistWsPatterns=window.__chromistWsPatterns||[]).push({:?})",
            pattern
        );
        let mut push_params =
            crate::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams::new(
                push_script.clone(),
            );
        push_params.world_name = None;
        if let Err(e) = self.execute(push_params).await {
            tracing::warn!(error = %e, "WebSocket shim: pattern script injection failed");
        }

        // Immediately activate in the current page context.
        self.evaluate(WS_SHIM).await.ok();
        self.evaluate(&push_script).await.ok();

        Ok(())
    }

    /// Spawn the background task that watches `Runtime.bindingCalled` for
    /// `__chromistWsCreate` events and invokes `handler` for each new fake WS.
    ///
    /// Returns an [`AbortOnDrop`](crate::AbortOnDrop) so that the spawned task
    /// is cancelled when the wrapper is dropped, preventing the listener from
    /// outliving its parent.
    pub(crate) fn spawn_ws_listener(&self, handler: WsRouteHandler) -> crate::AbortOnDrop {
        let handle = self.handle.clone();
        let session_id = self.session_id.clone();

        let mut stream: crate::listeners::EventStream<cdp_runtime::BindingCalledEvent> =
            self.event_listener();

        crate::AbortOnDrop(crate::runtime::spawn(async move {
            while let Some(ev) = stream.next().await {
                // __chromistWsSend / __chromistWsClose are informational;
                // the handler drives the conversation, not the shim events.
                // Those bindings exist so the page can notify Rust, but the
                // current model is push-from-Rust. Extend here if needed.
                if ev.name.as_str() != "__chromistWsCreate" {
                    continue;
                }
                let msg: serde_json::Value = match serde_json::from_str(&ev.payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = match msg.get("id").and_then(|v| v.as_u64()) {
                    Some(v) => v,
                    None => continue,
                };
                let url = match msg.get("url").and_then(|v| v.as_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let route = WebSocketRoute {
                    id,
                    url,
                    session_id: session_id.current(),
                    handle: handle.clone(),
                };
                handler(route).await;
            }
        }))
    }
}
