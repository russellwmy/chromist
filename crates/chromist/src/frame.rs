//! First-class frame handle with DOM, evaluation, and navigation surface.
//!
//! [`Frame`] wraps the identity triple `(HandlerHandle, SessionRef, FrameId)`
//! and an `Arc<RwLock<FrameTree>>` so every method can resolve live state
//! (URL, loading flag, utility context) without a CDP round-trip.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::cdp::browser_protocol::dom as cdp_dom;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::element::Element;
use crate::error::CdpError;
use crate::frame_tree::FrameTree;
use crate::handler::{HandlerHandle, SessionRef};

/// A handle to a single browser frame.
///
/// Cheaply cloneable — holds only shared references and a value-type `FrameId`.
#[derive(Clone)]
pub struct Frame {
    pub(crate) handle: HandlerHandle,
    pub(crate) session_id: SessionRef,
    pub(crate) frame_id: cdp_page::FrameId,
    pub(crate) frame_tree: Arc<RwLock<FrameTree>>,
    /// Back-reference to the [`Page`] that owns this frame.
    /// `None` when the frame was created outside a `Page` context
    /// (e.g. via [`Locator::content_frame`](crate::Locator::content_frame)).
    page: Option<crate::page::Page>,
}

impl Frame {
    pub(crate) fn new(
        handle: HandlerHandle,
        session_id: SessionRef,
        frame_id: cdp_page::FrameId,
        frame_tree: Arc<RwLock<FrameTree>>,
    ) -> Self {
        Self { handle, session_id, frame_id, frame_tree, page: None }
    }

    /// Attach a [`Page`] back-reference. Called by `Page` methods that vend frames.
    pub(crate) fn with_page(mut self, page: crate::page::Page) -> Self {
        self.page = Some(page);
        self
    }

    /// Returns the [`Page`](crate::Page) that owns this frame, if available.
    ///
    /// `None` for frames obtained via [`Locator::content_frame`](crate::Locator::content_frame) or other
    /// paths that do not go through a `Page` method.
    pub fn page(&self) -> Option<&crate::page::Page> {
        self.page.as_ref()
    }

    // ── Read-only accessors ───────────────────────────────────────────────

    /// Returns this frame's CDP `FrameId`.
    pub fn id(&self) -> &cdp_page::FrameId {
        &self.frame_id
    }

    /// Returns this frame's current URL from the local cache (no CDP
    /// round-trip). Updated on every `Page.frameNavigated` event.
    pub fn url(&self) -> Option<String> {
        self.frame_tree.read().ok().and_then(|t| t.get(&self.frame_id).map(|f| f.url.clone()))
    }

    /// Returns the frame's `name` attribute, if set.
    pub fn name(&self) -> Option<String> {
        self.frame_tree.read().ok().and_then(|t| t.get(&self.frame_id).and_then(|f| f.name.clone()))
    }

    /// Returns `true` while the frame is between `frameStartedLoading`
    /// and `frameStoppedLoading` events.
    pub fn is_loading(&self) -> bool {
        self.frame_tree
            .read()
            .ok()
            .and_then(|t| t.get(&self.frame_id).map(|f| f.loading))
            .unwrap_or(false)
    }

    /// Returns the parent frame's id, or `None` for the main frame.
    pub fn parent_id(&self) -> Option<cdp_page::FrameId> {
        self.frame_tree
            .read()
            .ok()
            .and_then(|t| t.get(&self.frame_id).and_then(|f| f.parent_id.clone()))
    }

    // ── Private helpers ───────────────────────────────────────────────────

    /// Resolve the utility-world `ExecutionContextId` for this frame, waiting
    /// up to 2 s for it to be seeded by the background event task.
    pub(crate) async fn utility_ctx_id(&self) -> crate::Result<cdp_runtime::ExecutionContextId> {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(tree) = self.frame_tree.read() {
                if let Some(ctx) = tree.utility_ctx_for_frame(&self.frame_id) {
                    return Ok(ctx);
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Resolve the document `NodeId` for this frame.
    ///
    /// Main-frame path: `DOM.getDocument` (fast, no round-trip per node).
    /// Sub-frame path: evaluate `document` in the frame's main context to get
    /// a `RemoteObjectId`, then `DOM.requestNode` to convert it to a `NodeId`.
    /// Fallback: `DOM.getDocument` when the frame has no main context yet.
    async fn document_node(&self) -> crate::Result<cdp_dom::NodeId> {
        let is_main = self
            .frame_tree
            .read()
            .ok()
            .and_then(|t| t.main_frame().map(|f| f.id == self.frame_id))
            .unwrap_or(false);
        if is_main {
            let resp = self
                .handle
                .execute(cdp_dom::GetDocumentParams::default(), Some(self.session_id.current()))
                .await?;
            return Ok(resp.root.node_id);
        }
        let main_ctx = self
            .frame_tree
            .read()
            .ok()
            .and_then(|t| t.get(&self.frame_id).and_then(|f| f.main_ctx));
        match main_ctx {
            Some(ctx) => {
                let mut eval = cdp_runtime::EvaluateParams::new("document".to_string());
                eval.context_id = Some(ctx);
                eval.return_by_value = Some(false);
                let resp = self.handle.execute(eval, Some(self.session_id.current())).await?;
                let oid = resp.result.object_id.ok_or(CdpError::NotFound)?;
                let req = cdp_dom::RequestNodeParams::new(oid);
                let node_resp = self.handle.execute(req, Some(self.session_id.current())).await?;
                if node_resp.node_id.0 == 0 {
                    return Err(CdpError::NotFound);
                }
                Ok(node_resp.node_id)
            }
            None => {
                let resp = self
                    .handle
                    .execute(cdp_dom::GetDocumentParams::default(), Some(self.session_id.current()))
                    .await?;
                Ok(resp.root.node_id)
            }
        }
    }

    // ── DOM methods ───────────────────────────────────────────────────────

    /// Create a [`Locator`](crate::locator::Locator) bound to this frame.
    pub fn locator(&self, selector: impl Into<String>) -> crate::locator::Locator {
        crate::locator::Locator::new(self.clone(), selector)
    }

    /// Find the first element matching a CSS selector, or `CdpError::NotFound`.
    pub async fn find_element(&self, selector: &str) -> crate::Result<Element> {
        let root = self.document_node().await?;
        let params = cdp_dom::QuerySelectorParams::new(root, selector.to_string());
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        if resp.node_id.0 == 0 {
            return Err(CdpError::NotFound);
        }
        let utility_ctx = self.utility_ctx_id().await.ok();
        Element::from_node_id(
            self.handle.clone(),
            self.session_id.clone(),
            resp.node_id,
            utility_ctx,
            Some(self.frame_id.clone()),
        )
        .await
    }

    /// Find all elements matching a CSS selector.
    pub async fn find_elements(&self, selector: &str) -> crate::Result<Vec<Element>> {
        let root = self.document_node().await?;
        let params = cdp_dom::QuerySelectorAllParams::new(root, selector.to_string());
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let utility_ctx = self.utility_ctx_id().await.ok();
        let mut out = Vec::new();
        for nid in resp.node_ids {
            if nid.0 == 0 {
                continue;
            }
            if let Ok(el) = Element::from_node_id(
                self.handle.clone(),
                self.session_id.clone(),
                nid,
                utility_ctx,
                Some(self.frame_id.clone()),
            )
            .await
            {
                out.push(el);
            }
        }
        Ok(out)
    }

    /// Find elements by evaluating a JavaScript expression in the utility world.
    ///
    /// The expression must evaluate to an `Array` or `NodeList` of DOM nodes.
    /// This is used internally by the `get_by_*` family.
    pub(crate) async fn find_elements_by_js(&self, js: &str) -> crate::Result<Vec<Element>> {
        let utility_ctx = self.utility_ctx_id().await.ok();
        let mut eval = cdp_runtime::EvaluateParams::new(js.to_string());
        eval.return_by_value = Some(false);
        eval.context_id = utility_ctx;
        let resp = self.handle.execute(eval, Some(self.session_id.current())).await?;
        let Some(array_oid) = resp.result.object_id else {
            return Ok(Vec::new());
        };
        let mut props_params = cdp_runtime::GetPropertiesParams::new(array_oid);
        props_params.own_properties = Some(true);
        let props = self.handle.execute(props_params, Some(self.session_id.current())).await?;
        let mut out = Vec::new();
        for prop in props.result {
            if prop.name.parse::<u32>().is_err() {
                continue;
            }
            let Some(val) = prop.value else { continue };
            let Some(oid) = val.object_id else { continue };
            let req = cdp_dom::RequestNodeParams::new(oid);
            let node_resp = self.handle.execute(req, Some(self.session_id.current())).await?;
            if node_resp.node_id.0 != 0 {
                if let Ok(el) = Element::from_node_id(
                    self.handle.clone(),
                    self.session_id.clone(),
                    node_resp.node_id,
                    utility_ctx,
                    Some(self.frame_id.clone()),
                )
                .await
                {
                    out.push(el);
                }
            }
        }
        Ok(out)
    }

    // ── Semantic get_by_* helpers ─────────────────────────────────────────

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements containing the
    /// given exact text.
    pub fn get_by_text(&self, text: impl Into<String>) -> crate::locator::Locator {
        crate::locator::Locator::new_with_js(
            self.clone(),
            crate::locator::js_get_by_text(&text.into()),
        )
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// ARIA role, optionally filtered by accessible name.
    pub fn get_by_role(
        &self,
        role: impl Into<String>,
        name: Option<&str>,
    ) -> crate::locator::Locator {
        crate::locator::Locator::new_with_js(
            self.clone(),
            crate::locator::js_get_by_role(&role.into(), name),
        )
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds form controls associated
    /// with the given label text.
    pub fn get_by_label(&self, label: impl Into<String>) -> crate::locator::Locator {
        crate::locator::Locator::new_with_js(
            self.clone(),
            crate::locator::js_get_by_label(&label.into()),
        )
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `placeholder` attribute value.
    pub fn get_by_placeholder(&self, text: impl Into<String>) -> crate::locator::Locator {
        crate::locator::Locator::new_with_js(
            self.clone(),
            crate::locator::js_get_by_placeholder(&text.into()),
        )
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `data-testid` attribute value.
    pub fn get_by_test_id(&self, id: impl Into<String>) -> crate::locator::Locator {
        crate::locator::Locator::new_with_js(
            self.clone(),
            crate::locator::js_get_by_test_id(&id.into()),
        )
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `alt` attribute value (e.g. image buttons).
    pub fn get_by_alt_text(&self, text: impl Into<String>) -> crate::locator::Locator {
        crate::locator::Locator::new_with_js(
            self.clone(),
            crate::locator::js_get_by_alt_text(&text.into()),
        )
    }

    /// Returns a [`Locator`](crate::locator::Locator) that finds elements with the given
    /// `title` attribute value.
    pub fn get_by_title(&self, text: impl Into<String>) -> crate::locator::Locator {
        crate::locator::Locator::new_with_js(
            self.clone(),
            crate::locator::js_get_by_title(&text.into()),
        )
    }

    /// Find the first element matching an XPath expression.
    pub async fn find_xpath(&self, xpath: &str) -> crate::Result<Element> {
        let results = self.find_xpaths(xpath).await?;
        results.into_iter().next().ok_or(CdpError::NotFound)
    }

    /// Find all elements matching an XPath expression.
    pub async fn find_xpaths(&self, xpath: &str) -> crate::Result<Vec<Element>> {
        let utility_ctx = self.utility_ctx_id().await.ok();
        let expr = format!(
            "(()=>{{const r=document.evaluate({:?},document,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);const a=[];for(let i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}})()",
            xpath
        );
        let mut eval = cdp_runtime::EvaluateParams::new(expr);
        eval.return_by_value = Some(false);
        eval.context_id = utility_ctx;
        let resp = self.handle.execute(eval, Some(self.session_id.current())).await?;
        let Some(array_oid) = resp.result.object_id else {
            return Ok(Vec::new());
        };
        let mut props_params = cdp_runtime::GetPropertiesParams::new(array_oid);
        props_params.own_properties = Some(true);
        let props = self.handle.execute(props_params, Some(self.session_id.current())).await?;
        let mut out = Vec::new();
        for prop in props.result {
            if prop.name.parse::<u32>().is_err() {
                continue;
            }
            let Some(val) = prop.value else { continue };
            let Some(oid) = val.object_id else { continue };
            let req = cdp_dom::RequestNodeParams::new(oid);
            let node_resp = self.handle.execute(req, Some(self.session_id.current())).await?;
            if node_resp.node_id.0 != 0 {
                if let Ok(el) = Element::from_node_id(
                    self.handle.clone(),
                    self.session_id.clone(),
                    node_resp.node_id,
                    utility_ctx,
                    Some(self.frame_id.clone()),
                )
                .await
                {
                    out.push(el);
                }
            }
        }
        Ok(out)
    }

    /// Wait for an element matching `selector` to appear (30 s timeout).
    pub async fn wait_for_selector(&self, selector: impl Into<String>) -> crate::Result<Element> {
        self.wait_for_selector_with_timeout(selector, Duration::from_secs(30)).await
    }

    /// Wait for an element matching `selector` to appear, polling every 100 ms.
    pub async fn wait_for_selector_with_timeout(
        &self,
        selector: impl Into<String>,
        timeout: Duration,
    ) -> crate::Result<Element> {
        let selector = selector.into();
        let frame = self.clone();
        crate::runtime::timeout(timeout, async move {
            loop {
                if let Ok(el) = frame.find_element(&selector).await {
                    return Ok::<Element, CdpError>(el);
                }
                crate::runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    // ── Evaluation methods ────────────────────────────────────────────────

    /// Evaluate JavaScript in this frame and return the result.
    ///
    /// Function-shaped input is auto-wrapped in an IIFE — callers may pass
    /// either bare expressions or function bodies.
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

    // ── Navigation methods ────────────────────────────────────────────────

    /// Navigate to a URL and wait for the `load` lifecycle event.
    pub async fn goto(
        &self,
        params: impl Into<cdp_page::NavigateParams>,
    ) -> crate::Result<crate::Navigation> {
        self.goto_with(params, crate::lifecycle::GotoOptions::default()).await
    }

    /// Navigate to a URL with explicit options.
    pub async fn goto_with(
        &self,
        params: impl Into<cdp_page::NavigateParams>,
        options: crate::lifecycle::GotoOptions,
    ) -> crate::Result<crate::Navigation> {
        let waiter = self.navigation_waiter(options.wait_until).with_timeout(options.timeout);
        let mut params = params.into();
        if let Some(referer) = options.referer {
            params.referrer = Some(referer);
        }
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        if let Some(err) = &resp.error_text {
            if !err.is_empty() {
                return Err(CdpError::NavigationFailed { reason: err.clone() });
            }
        }
        waiter.wait().await?;
        Ok(resp.into())
    }

    /// Create a pre-armed `NavigationWaiter` for the given condition.
    pub fn navigation_waiter(
        &self,
        condition: crate::lifecycle::WaitUntil,
    ) -> crate::lifecycle::NavigationWaiter {
        crate::lifecycle::NavigationWaiter {
            sub: self.handle.subscribe(Some(self.session_id.current())),
            condition,
            timeout: Duration::from_secs(30),
            frame_id: Some(self.frame_id.clone()),
        }
    }

    /// Wait for the next load event (30 s timeout).
    pub async fn wait_for_navigation(&self) -> crate::Result<()> {
        self.navigation_waiter(crate::lifecycle::WaitUntil::Load).wait().await
    }

    // ── Frame-scoped evaluation ───────────────────────────────────────────

    /// Evaluate `expr` in this frame's main-world context.
    async fn eval_in_frame(&self, expr: &str) -> crate::Result<cdp_runtime::EvaluateResponse> {
        let main_ctx = self
            .frame_tree
            .read()
            .ok()
            .and_then(|t| t.get(&self.frame_id).and_then(|f| f.main_ctx));
        let mut params = cdp_runtime::EvaluateParams::new(expr.to_string());
        params.return_by_value = Some(true);
        if let Some(ctx) = main_ctx {
            params.context_id = Some(ctx);
        }
        self.handle.execute(params, Some(self.session_id.current())).await
    }

    /// Return the outer HTML of the frame's document.
    pub async fn content(&self) -> crate::Result<String> {
        let resp = self.eval_in_frame("document.documentElement.outerHTML").await?;
        match resp.result.value {
            Some(serde_json::Value::String(s)) => Ok(s),
            Some(v) => Ok(v.to_string()),
            None => Ok(String::new()),
        }
    }

    /// Replace the frame's HTML using `Page.setDocumentContent`.
    pub async fn set_content(&self, html: impl AsRef<str>) -> crate::Result<()> {
        let waiter = self.navigation_waiter(crate::lifecycle::WaitUntil::Load);
        let params = crate::cdp::browser_protocol::page::SetDocumentContentParams::new(
            self.frame_id.clone(),
            html.as_ref().to_string(),
        );
        self.handle.execute(params, Some(self.session_id.current())).await?;
        waiter.wait().await
    }

    /// Return the `document.title` of this frame, or `None` when empty.
    pub async fn title(&self) -> crate::Result<Option<String>> {
        let resp = self.eval_in_frame("document.title").await?;
        Ok(match resp.result.value {
            Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s),
            _ => None,
        })
    }

    /// Wait for the frame's document to reach `state`.
    ///
    /// Uses `document.readyState` as a fast-path before subscribing to
    /// lifecycle events.  Returns immediately for [`WaitUntil::NoWait`](crate::WaitUntil::NoWait).
    pub async fn wait_for_load_state(
        &self,
        state: crate::lifecycle::WaitUntil,
    ) -> crate::Result<()> {
        if state == crate::lifecycle::WaitUntil::NoWait {
            return Ok(());
        }
        if matches!(
            state,
            crate::lifecycle::WaitUntil::Load | crate::lifecycle::WaitUntil::DomContentLoaded
        ) {
            if let Ok(resp) = self.eval_in_frame("document.readyState").await {
                let rs =
                    resp.result.value.as_ref().and_then(|v| v.as_str()).unwrap_or("").to_string();
                let satisfied = match state {
                    crate::lifecycle::WaitUntil::Load => rs == "complete",
                    crate::lifecycle::WaitUntil::DomContentLoaded => {
                        rs == "interactive" || rs == "complete"
                    }
                    _ => false,
                };
                if satisfied {
                    return Ok(());
                }
            }
        }
        let waiter = self.navigation_waiter(state);
        waiter.wait().await
    }

    /// Poll `expr` every 100 ms until it evaluates to a truthy value (30 s
    /// timeout).
    pub async fn wait_for_function(
        &self,
        expr: impl Into<String>,
    ) -> crate::Result<crate::evaluate::EvaluationResult> {
        let expr = expr.into();
        let frame = self.clone();
        crate::runtime::timeout(Duration::from_secs(30), async move {
            loop {
                if let Ok(resp) = frame.eval_in_frame(&expr).await {
                    let res = crate::evaluate::EvaluationResult { inner: resp };
                    if res.is_truthy() {
                        return Ok(res);
                    }
                }
                crate::runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Wait until the frame's URL contains `pattern` (30 s timeout).
    pub async fn wait_for_url(&self, pattern: impl Into<String>) -> crate::Result<()> {
        let pattern = pattern.into();
        let frame = self.clone();
        crate::runtime::timeout(Duration::from_secs(30), async move {
            loop {
                if let Some(url) = frame.url() {
                    if url.contains(pattern.as_str()) {
                        return Ok(());
                    }
                }
                crate::runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Return all immediate child frames of this frame.
    pub fn child_frames(&self) -> crate::Result<Vec<Frame>> {
        let ids: Vec<cdp_page::FrameId> = self
            .frame_tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .children_of(&self.frame_id)
            .into_iter()
            .map(|f| f.id.clone())
            .collect();
        let page = self.page.clone();
        Ok(ids
            .into_iter()
            .map(|id| {
                let f = Frame::new(
                    self.handle.clone(),
                    self.session_id.clone(),
                    id,
                    std::sync::Arc::clone(&self.frame_tree),
                );
                if let Some(p) = page.clone() {
                    f.with_page(p)
                } else {
                    f
                }
            })
            .collect())
    }

    /// Returns `true` if the frame is no longer present in the frame tree.
    pub fn is_detached(&self) -> bool {
        self.frame_tree.read().ok().map(|t| t.get(&self.frame_id).is_none()).unwrap_or(true)
    }

    // ── Selector predicates ───────────────────────────────────────────────

    /// Return `true` if the first element matching `selector` is visible.
    pub async fn is_visible(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).is_visible().await
    }

    /// Return `true` if the first element matching `selector` is hidden or
    /// absent.
    pub async fn is_hidden(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).is_hidden().await
    }

    /// Return `true` if the first element matching `selector` is enabled.
    pub async fn is_enabled(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).is_enabled().await
    }

    /// Return `true` if the first element matching `selector` is disabled.
    pub async fn is_disabled(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).is_disabled().await
    }

    /// Return `true` if the first checkbox / radio matching `selector` is
    /// checked.
    pub async fn is_checked(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).is_checked().await
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("frame_id", &self.frame_id.inner())
            .field("url", &self.url())
            .finish()
    }
}
