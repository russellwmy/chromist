use std::time::Duration;

use futures::StreamExt;
use serde::Deserialize as _;

use super::{Page, ScriptSource};
use crate::cdp::browser_protocol::network as cdp_network;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::js_protocol::debugger as cdp_debugger;
use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::error::CdpError;

impl Page {
    /// Evaluate JavaScript in the page's main frame and return the result.
    ///
    /// Function-shaped input (`function () { … }`, `async () => …`, etc.) is
    /// auto-wrapped in an IIFE so callers can pass either expressions or
    /// function bodies. For full `Runtime.evaluate` parameter control, build
    /// an [`EvaluateParams`](cdp_runtime::EvaluateParams) and submit it via
    /// `page.handle().execute(params, page.session_id())`.
    #[tracing::instrument(skip(self, js), level = "debug")]
    pub async fn evaluate(
        &self,
        js: impl Into<String>,
    ) -> crate::Result<crate::evaluate::EvaluationResult> {
        let js = js.into();
        let expr =
            if crate::evaluate::is_likely_js_function(&js) { format!("({js})()") } else { js };
        let mut params = cdp_runtime::EvaluateParams::new(expr);
        params.return_by_value = Some(true);
        let inner = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(crate::evaluate::EvaluationResult { inner })
    }

    /// Inject a script that runs on every new document before any page JS.
    ///
    /// Returns an [`InitScriptId`](crate::InitScriptId) that can be passed to
    /// [`Page::remove_init_script`] to remove the script later.
    pub async fn add_init_script(
        &self,
        params: impl Into<cdp_page::AddScriptToEvaluateOnNewDocumentParams>,
    ) -> crate::Result<crate::InitScriptId> {
        let resp = self.handle.execute(params.into(), Some(self.session_id.current())).await?;
        Ok(crate::InitScriptId(resp.identifier))
    }

    /// Remove an init script previously registered via [`Page::add_init_script`].
    pub async fn remove_init_script(&self, id: crate::InitScriptId) -> crate::Result<()> {
        let params = cdp_page::RemoveScriptToEvaluateOnNewDocumentParams::new(id.0);
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Returns the execution context ID for the main frame.
    pub async fn execution_context(&self) -> crate::Result<cdp_runtime::ExecutionContextId> {
        let frame_id = self.mainframe()?;
        self.frame_execution_context(&frame_id).await
    }

    /// Returns the execution context ID for a specific frame.
    pub async fn frame_execution_context(
        &self,
        _frame_id: &cdp_page::FrameId,
    ) -> crate::Result<cdp_runtime::ExecutionContextId> {
        let js = "globalThis.__chromist_ctx_id = (globalThis.__chromist_ctx_id || (Math.random() * 2147483647 | 0)); globalThis.__chromist_ctx_id";
        let mut params = cdp_runtime::EvaluateParams::new(js);
        params.return_by_value = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let id = resp.result.value.and_then(|v| v.as_i64()).unwrap_or(1);
        Ok(cdp_runtime::ExecutionContextId(id))
    }

    /// Returns an isolated execution context ID for the main frame.
    pub async fn secondary_execution_context(
        &self,
    ) -> crate::Result<cdp_runtime::ExecutionContextId> {
        let frame_id = self.mainframe()?;
        self.frame_secondary_execution_context(&frame_id).await
    }

    /// Returns the isolated utility-world execution context ID for the given frame.
    ///
    /// Returns the cached ID maintained by the background subscription started
    /// in [`Page::attach`]. Falls back to `Page.createIsolatedWorld` if the
    /// cache is not yet populated (e.g. called synchronously after attach).
    pub async fn frame_secondary_execution_context(
        &self,
        frame_id: &cdp_page::FrameId,
    ) -> crate::Result<cdp_runtime::ExecutionContextId> {
        if let Some(ctx) =
            self.frame_tree.read().ok().and_then(|t| t.utility_ctx_for_frame(frame_id))
        {
            return Ok(ctx);
        }
        let mut params = cdp_page::CreateIsolatedWorldParams::new(frame_id.clone());
        params.world_name = Some(crate::frame_tree::UTILITY_WORLD_NAME.to_string());
        params.grant_univeral_access = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(resp.execution_context_id)
    }

    /// Returns a future that executes a CDP command on this page's session.
    pub fn command_future<C>(
        &self,
        cmd: C,
    ) -> impl std::future::Future<Output = crate::Result<C::Response>> + Send + 'static
    where
        C: chromist_types::Command + serde::Serialize + Send + 'static,
        C::Response: serde::de::DeserializeOwned + Send + 'static,
    {
        let handle = self.handle.clone();
        let session_id = self.session_id.clone();
        async move { handle.execute(cmd, Some(session_id.current())).await }
    }

    /// Execute a command while capturing the next HTTP response; returns both together.
    pub fn http_future<C>(
        &self,
        cmd: C,
    ) -> impl std::future::Future<
        Output = crate::Result<(C::Response, crate::network::ArcHttpRequest)>,
    > + Send
           + 'static
    where
        C: chromist_types::Command + serde::Serialize + Send + 'static,
        C::Response: serde::de::DeserializeOwned + Send + 'static,
    {
        let handle = self.handle.clone();
        let session_id = self.session_id.clone();
        async move {
            let _ = handle
                .execute(
                    crate::cdp::browser_protocol::network::EnableParams::default(),
                    Some(session_id.current()),
                )
                .await;
            let mut sub = handle.subscribe(Some(session_id.current()));
            let cmd_result = handle.execute(cmd, Some(session_id.current())).await?;
            let mut http_req: crate::network::ArcHttpRequest = None;
            let _ = crate::runtime::timeout(Duration::from_secs(30), async {
                while let Some(frame) = sub.next().await {
                    if frame.method == "Network.responseReceived" {
                        match cdp_network::ResponseReceivedEvent::deserialize(&frame.params) {
                            Err(e) => tracing::trace!(
                                method = "Network.responseReceived",
                                error = %e,
                                "dropping event: deserialization failed"
                            ),
                            Ok(ev) => {
                                http_req = Some(std::sync::Arc::new(crate::network::HttpRequest {
                                    request_id: ev.request_id,
                                    url: ev.response.url.clone(),
                                    method: String::new(),
                                    headers: std::collections::HashMap::new(),
                                    post_data: None,
                                    frame_id: ev.frame_id,
                                    response: Some(ev.response),
                                    failure_text: None,
                                    from_memory_cache: false,
                                    interception_id: None,
                                    is_navigation_request: false,
                                    allow_interception: false,
                                    resource_type: None,
                                    redirect_chain: Vec::new(),
                                    timestamp: None,
                                    encoded_response_length: None,
                                    handle: None,
                                    session_id: None,
                                }));
                            }
                        }
                    } else if frame.method == "Page.loadEventFired" {
                        break;
                    }
                }
            })
            .await;
            Ok((cmd_result, http_req))
        }
    }

    /// Poll `expr` every 100 ms until it evaluates to a truthy value, or 30 s elapses.
    ///
    /// Returns the first truthy [`EvaluationResult`](crate::EvaluationResult). Returns [`CdpError::Timeout`](crate::CdpError::Timeout)
    /// if the deadline is reached without a truthy result.
    #[tracing::instrument(skip(self, expr), level = "debug")]
    pub async fn wait_for_function(
        &self,
        expr: impl Into<String>,
    ) -> crate::Result<crate::evaluate::EvaluationResult> {
        let expr = expr.into();
        let page = self.clone();
        crate::runtime::timeout(Duration::from_secs(30), async move {
            loop {
                match page.evaluate(&expr).await {
                    Ok(result) if result.is_truthy() => return Ok(result),
                    Ok(_) => {}
                    Err(_) => {}
                }
                crate::runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Evaluate a JavaScript expression and return a [`JsHandle`](crate::JsHandle) to the result.
    ///
    /// Unlike [`evaluate`](Self::evaluate), the object is not serialized to JSON — it stays
    /// live in the browser until the handle is disposed or the page navigates.
    ///
    /// Returns [`CdpError::MissingObjectId`] if the expression evaluates to a primitive.
    #[tracing::instrument(skip(self, js), level = "debug")]
    pub async fn evaluate_handle(
        &self,
        js: impl Into<String>,
    ) -> crate::Result<crate::js_handle::JsHandle> {
        let js = js.into();
        let expr =
            if crate::evaluate::is_likely_js_function(&js) { format!("({js})()") } else { js };
        let mut params = cdp_runtime::EvaluateParams::new(expr);
        params.return_by_value = Some(false);
        params.await_promise = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        if let Some(ex) = resp.exception_details {
            return Err(CdpError::JavascriptException(Box::new(ex)));
        }
        let object_id = resp.result.object_id.ok_or(CdpError::MissingObjectId)?;
        Ok(crate::js_handle::JsHandle::new(self.handle.clone(), self.session_id.clone(), object_id))
    }

    /// Fetch the source text of a script by its runtime `ScriptId`.
    pub async fn script_source(&self, script_id: cdp_runtime::ScriptId) -> crate::Result<String> {
        let params = cdp_debugger::GetScriptSourceParams::new(script_id);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(resp.script_source)
    }

    /// Inject a `<script>` tag into the current page's `<head>`.
    ///
    /// Pass [`ScriptSource::Url`] to load an external script via `src` (the
    /// future resolves only after the script's `load` event fires) or
    /// [`ScriptSource::Inline`] to insert literal source as `textContent`.
    pub async fn add_script_tag(&self, source: ScriptSource) -> crate::Result<()> {
        let expr = match source {
            ScriptSource::Url(u) => {
                let u = u.replace('`', "\\`");
                format!(
                    "new Promise((res,rej)=>{{const s=document.createElement('script');\
                     s.src=`{u}`;s.onload=()=>res();s.onerror=(e)=>rej(e);\
                     document.head.appendChild(s);}})"
                )
            }
            ScriptSource::Inline(c) => {
                let c = c.replace('`', "\\`");
                format!(
                    "(()=>{{const s=document.createElement('script');\
                     s.textContent=`{c}`;document.head.appendChild(s);}})();"
                )
            }
        };
        let mut params = cdp_runtime::EvaluateParams::new(expr);
        params.await_promise = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        if let Some(ex) = resp.exception_details {
            return Err(CdpError::JavascriptException(Box::new(ex)));
        }
        Ok(())
    }
}
