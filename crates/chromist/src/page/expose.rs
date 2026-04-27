//! `exposeFunction` / `exposeBinding` — bridge page-JS into async Rust handlers.
//!
//! Uses `Runtime.addBinding` to register a single bridge entry-point
//! (`__chromistBridgeCall`). An injected JS shim wraps each exposed name in a
//! `Promise`-returning function. When the page calls the function the shim
//! serializes `{fn, id, args}` and passes it to the binding; the Rust handler
//! is invoked asynchronously; the result is returned to the page by evaluating
//! `__chromistBridgeReply({id, result})`.

use futures::StreamExt;

use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::page::Page;

/// One-time JS setup: pending-map + reply dispatcher.
const BRIDGE_SETUP: &str = r#"(function() {
  if (window.__chromistBridgeReply !== undefined) return;
  window.__chromistBridgePending = {};
  window.__chromistBridgeId = 0;
  window.__chromistBridgeReply = function(msg) {
    var cb = window.__chromistBridgePending[msg.id];
    if (!cb) return;
    delete window.__chromistBridgePending[msg.id];
    if ('error' in msg) cb.reject(new Error(msg.error));
    else cb.resolve(msg.result);
  };
})();"#;

fn per_fn_wrapper(name: &str) -> String {
    format!(
        r#"window[{name:?}] = function() {{
  var args = Array.prototype.slice.call(arguments);
  return new Promise(function(resolve, reject) {{
    var id = ++window.__chromistBridgeId;
    window.__chromistBridgePending[id] = {{resolve: resolve, reject: reject}};
    window.__chromistBridgeCall(JSON.stringify({{fn: {name:?}, id: id, args: args}}));
  }});
}};"#
    )
}

impl Page {
    /// Expose a Rust async function to the page's JavaScript context.
    ///
    /// After this call `window[name](...args)` is available in the page and
    /// returns a `Promise` whose value is the return value of `handler`.
    ///
    /// The binding persists for the current page load. For new navigations,
    /// re-inject using [`add_init_script`](crate::Page::add_init_script).
    ///
    /// The dispatcher task is owned by the page and lives until the last
    /// `Page` clone is dropped, so callers do not need to track a handle.
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// page.expose_function("greet", |args: Vec<serde_json::Value>| async move {
    ///     let name = args.first().and_then(|v| v.as_str()).unwrap_or("world");
    ///     serde_json::json!(format!("Hello, {name}!"))
    /// }).await?;
    /// page.evaluate(r#"greet("Rust").then(console.log)"#).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn expose_function<F, Fut>(
        &self,
        name: impl Into<String>,
        handler: F,
    ) -> crate::Result<()>
    where
        F: Fn(Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = serde_json::Value> + Send + 'static,
    {
        let name = name.into();
        self.setup_bridge().await;
        self.evaluate(&per_fn_wrapper(&name)).await.ok();

        let fn_name = name.clone();
        let page = self.clone();
        let mut stream: crate::listeners::EventStream<cdp_runtime::BindingCalledEvent> =
            self.event_listener();

        let task = crate::runtime::spawn(async move {
            while let Some(ev) = stream.next().await {
                if ev.name != "__chromistBridgeCall" {
                    continue;
                }
                let msg: serde_json::Value = match serde_json::from_str(&ev.payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if msg.get("fn").and_then(|v| v.as_str()).unwrap_or("") != fn_name {
                    continue;
                }
                let id = match msg.get("id").and_then(|v| v.as_i64()) {
                    Some(v) => v,
                    None => continue,
                };
                let args: Vec<serde_json::Value> =
                    msg.get("args").and_then(|v| v.as_array()).cloned().unwrap_or_default();

                let result = handler(args).await;
                let reply = serde_json::json!({"id": id, "result": result});
                let eval_str = format!(
                    "window.__chromistBridgeReply && window.__chromistBridgeReply({reply})"
                );
                let _ = page.evaluate(&eval_str).await;
            }
        });
        self.register_listener_task(crate::AbortOnDrop(task));
        Ok(())
    }

    /// Expose a Rust async function with a `source` context object.
    ///
    /// Like [`expose_function`](Page::expose_function) but the handler receives
    /// the call-site `source` as its first argument (currently `null` — the
    /// field is reserved for future use) followed by the JS arguments.
    pub async fn expose_binding<F, Fut>(
        &self,
        name: impl Into<String>,
        handler: F,
    ) -> crate::Result<()>
    where
        F: Fn(serde_json::Value, Vec<serde_json::Value>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = serde_json::Value> + Send + 'static,
    {
        let name = name.into();
        self.setup_bridge().await;
        self.evaluate(&per_fn_wrapper(&name)).await.ok();

        let fn_name = name.clone();
        let page = self.clone();
        let mut stream: crate::listeners::EventStream<cdp_runtime::BindingCalledEvent> =
            self.event_listener();

        let task = crate::runtime::spawn(async move {
            while let Some(ev) = stream.next().await {
                if ev.name != "__chromistBridgeCall" {
                    continue;
                }
                let msg: serde_json::Value = match serde_json::from_str(&ev.payload) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if msg.get("fn").and_then(|v| v.as_str()).unwrap_or("") != fn_name {
                    continue;
                }
                let id = match msg.get("id").and_then(|v| v.as_i64()) {
                    Some(v) => v,
                    None => continue,
                };
                let args: Vec<serde_json::Value> =
                    msg.get("args").and_then(|v| v.as_array()).cloned().unwrap_or_default();

                let source = serde_json::Value::Null;
                let result = handler(source, args).await;
                let reply = serde_json::json!({"id": id, "result": result});
                let eval_str = format!(
                    "window.__chromistBridgeReply && window.__chromistBridgeReply({reply})"
                );
                let _ = page.evaluate(&eval_str).await;
            }
        });
        self.register_listener_task(crate::AbortOnDrop(task));
        Ok(())
    }

    /// Register the bridge binding and inject the one-time setup shim.
    async fn setup_bridge(&self) {
        let _ = self
            .handle
            .execute(
                cdp_runtime::AddBindingParams::new("__chromistBridgeCall"),
                Some(self.session_id.current()),
            )
            .await;
        self.evaluate(BRIDGE_SETUP).await.ok();
    }
}
