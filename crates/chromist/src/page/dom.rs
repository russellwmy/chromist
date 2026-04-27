//! [`Dom`](crate::Dom) sub-handle: DOM queries (`find_element`, `find_xpath`, …) and
//! waiters (`wait_for_selector`, …).
//!
//! Obtained via [`Page::dom`]. Cheaply cloneable — holds a [`HandlerHandle`](crate::HandlerHandle),
//! the page's [`SessionRef`], and the page's [`FrameTree`] so it can resolve
//! the main frame's utility execution context locally without a CDP
//! round-trip.
//!
//! Frame navigation helpers (`main_frame`, `frame_by_id`, `frames`, …) and
//! locator-delegated state checks (`is_visible`, `is_enabled`, …) remain on
//! [`Page`]: they're thin convenience wrappers around `page.locator()` /
//! `Frame::with_page()` and don't gain anything from the split.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::Page;
use crate::cdp::browser_protocol::dom as cdp_dom;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::element::Element;
use crate::error::CdpError;
use crate::frame_tree::FrameTree;
use crate::handler::{HandlerHandle, SessionRef};

/// Sub-handle for DOM queries on a [`Page`]. Obtain via [`Page::dom`].
#[derive(Debug, Clone)]
pub struct Dom {
    handle: HandlerHandle,
    session_id: SessionRef,
    frame_tree: Arc<RwLock<FrameTree>>,
}

impl Dom {
    pub(in crate::page) fn new(
        handle: HandlerHandle,
        session_id: SessionRef,
        frame_tree: Arc<RwLock<FrameTree>>,
    ) -> Self {
        Self { handle, session_id, frame_tree }
    }

    fn session(&self) -> Option<Arc<str>> {
        Some(self.session_id.current())
    }

    /// Returns the frame ID of the main (top-level) frame from the local
    /// frame tree — no CDP round-trip.
    fn mainframe_id(&self) -> crate::Result<cdp_page::FrameId> {
        self.frame_tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .main_frame()
            .map(|f| f.id.clone())
            .ok_or(CdpError::NotFound)
    }

    /// Resolves the utility-world execution context for the main frame,
    /// waiting up to 2 s for it to be populated by the event subscription.
    async fn utility_ctx_id(&self) -> crate::Result<cdp_runtime::ExecutionContextId> {
        let frame_id = self.mainframe_id()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(tree) = self.frame_tree.read() {
                if let Some(ctx) = tree.utility_ctx_for_frame(&frame_id) {
                    return Ok(ctx);
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn document_node(&self) -> crate::Result<cdp_dom::NodeId> {
        let resp =
            self.handle.execute(cdp_dom::GetDocumentParams::default(), self.session()).await?;
        Ok(resp.root.node_id)
    }

    /// Find the first DOM element matching a CSS selector, or return
    /// [`CdpError::NotFound`].
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn find_element(&self, selector: &str) -> crate::Result<Element> {
        let root = self.document_node().await?;
        let params = cdp_dom::QuerySelectorParams::new(root, selector.to_string());
        let resp = self.handle.execute(params, self.session()).await?;
        if resp.node_id.0 == 0 {
            return Err(CdpError::NotFound);
        }
        let utility_ctx = self.utility_ctx_id().await.ok();
        let frame_id =
            self.frame_tree.read().ok().and_then(|t| t.main_frame().map(|f| f.id.clone()));
        Element::from_node_id(
            self.handle.clone(),
            self.session_id.clone(),
            resp.node_id,
            utility_ctx,
            frame_id,
        )
        .await
    }

    /// Find all DOM elements matching a CSS selector.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn find_elements(&self, selector: &str) -> crate::Result<Vec<Element>> {
        let root = self.document_node().await?;
        let params = cdp_dom::QuerySelectorAllParams::new(root, selector.to_string());
        let resp = self.handle.execute(params, self.session()).await?;
        let utility_ctx = self.utility_ctx_id().await.ok();
        let frame_id =
            self.frame_tree.read().ok().and_then(|t| t.main_frame().map(|f| f.id.clone()));
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
                frame_id.clone(),
            )
            .await
            {
                out.push(el);
            }
        }
        Ok(out)
    }

    /// Find the first element matching an XPath expression.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn find_xpath(&self, xpath: &str) -> crate::Result<Element> {
        let results = self.find_xpaths(xpath).await?;
        results.into_iter().next().ok_or(CdpError::NotFound)
    }

    /// Find all elements matching an XPath expression.
    ///
    /// Uses 2 + N CDP round trips (1 evaluate + 1 getProperties + N
    /// requestNode), down from 2 + 4N in the naive approach.
    pub async fn find_xpaths(&self, xpath: &str) -> crate::Result<Vec<Element>> {
        let utility_ctx = self.utility_ctx_id().await.ok();
        let frame_id =
            self.frame_tree.read().ok().and_then(|t| t.main_frame().map(|f| f.id.clone()));

        // Single evaluate that collects all matching nodes into a JS array
        // and returns it by reference (not by value) so we get an object_id
        // we can introspect with getProperties. Run in the utility world so
        // the XPath engine uses unmodified document.evaluate.
        let expr = format!(
            "(()=>{{const r=document.evaluate({:?},document,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);const a=[];for(let i=0;i<r.snapshotLength;i++)a.push(r.snapshotItem(i));return a;}})()",
            xpath
        );
        let mut eval = cdp_runtime::EvaluateParams::new(expr);
        eval.return_by_value = Some(false);
        eval.context_id = utility_ctx;
        let resp = self.handle.execute(eval, self.session()).await?;
        let Some(array_oid) = resp.result.object_id else {
            return Ok(Vec::new());
        };

        // Get all array elements in one call — numeric-keyed own properties
        // are the DOM node references; "length" and other non-numeric keys
        // are skipped.
        let mut props_params = cdp_runtime::GetPropertiesParams::new(array_oid);
        props_params.own_properties = Some(true);
        let props = self.handle.execute(props_params, self.session()).await?;

        let mut out = Vec::new();
        for prop in props.result {
            if prop.name.parse::<u32>().is_err() {
                continue;
            }
            let Some(val) = prop.value else { continue };
            let Some(oid) = val.object_id else { continue };
            let req = cdp_dom::RequestNodeParams::new(oid);
            let node_resp = self.handle.execute(req, self.session()).await?;
            if node_resp.node_id.0 != 0 {
                if let Ok(el) = Element::from_node_id(
                    self.handle.clone(),
                    self.session_id.clone(),
                    node_resp.node_id,
                    utility_ctx,
                    frame_id.clone(),
                )
                .await
                {
                    out.push(el);
                }
            }
        }
        Ok(out)
    }

    /// Wait for an element matching the CSS `selector` to appear. Uses the
    /// default 30 s timeout and 100 ms polling interval.
    #[tracing::instrument(skip(self, selector), level = "debug")]
    pub async fn wait_for_selector(&self, selector: impl Into<String>) -> crate::Result<Element> {
        self.wait_for_selector_with_timeout(selector, Duration::from_secs(30)).await
    }

    /// Wait for an element matching the CSS `selector` to appear, polling
    /// every 100 ms until it does or `timeout` elapses.
    pub async fn wait_for_selector_with_timeout(
        &self,
        selector: impl Into<String>,
        timeout: Duration,
    ) -> crate::Result<Element> {
        let selector = selector.into();
        let dom = self.clone();
        crate::runtime::timeout(timeout, async move {
            loop {
                if let Ok(el) = dom.find_element(&selector).await {
                    return Ok::<Element, CdpError>(el);
                }
                crate::runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Wait for an element matching the XPath expression to appear.
    #[tracing::instrument(skip(self, xpath), level = "debug")]
    pub async fn wait_for_xpath(&self, xpath: impl Into<String>) -> crate::Result<Element> {
        self.wait_for_xpath_with_timeout(xpath, Duration::from_secs(30)).await
    }

    /// Wait for an element matching the XPath expression to appear, polling
    /// every 100 ms until it does or `timeout` elapses.
    pub async fn wait_for_xpath_with_timeout(
        &self,
        xpath: impl Into<String>,
        timeout: Duration,
    ) -> crate::Result<Element> {
        let xpath = xpath.into();
        let dom = self.clone();
        crate::runtime::timeout(timeout, async move {
            loop {
                if let Ok(el) = dom.find_xpath(&xpath).await {
                    return Ok::<Element, CdpError>(el);
                }
                crate::runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .map_err(|_| CdpError::Timeout)?
    }

    /// Get the root DOM document node.
    pub async fn document(&self) -> crate::Result<crate::DomNode> {
        let params = cdp_dom::GetDocumentParams { depth: Some(-1), ..Default::default() };
        let resp = self.handle.execute(params, self.session()).await?;
        Ok(resp.root.into())
    }

    /// Describe a DOM node by node ID.
    pub async fn describe_node(&self, node_id: cdp_dom::NodeId) -> crate::Result<crate::DomNode> {
        let params = cdp_dom::DescribeNodeParams {
            node_id: Some(node_id),
            depth: Some(100),
            ..Default::default()
        };
        let resp = self.handle.execute(params, self.session()).await?;
        Ok(resp.node.into())
    }
}

impl Page {
    /// Returns the [`Dom`](crate::Dom) sub-handle for DOM queries (`find_element`,
    /// `find_xpath`, …) and waiters (`wait_for_selector`, …).
    pub fn dom(&self) -> Dom {
        Dom::new(self.handle.clone(), self.session_id.clone(), Arc::clone(&self.frame_tree))
    }

    /// Returns the frame ID of the main (top-level) frame.
    ///
    /// Reads from the local `FrameTree` — no CDP round-trip.
    pub fn mainframe(&self) -> crate::Result<cdp_page::FrameId> {
        self.frame_tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .main_frame()
            .map(|f| f.id.clone())
            .ok_or(CdpError::NotFound)
    }

    /// Returns a [`Frame`](crate::frame::Frame) handle for the main (top-level) frame.
    ///
    /// Reads from the local `FrameTree` — no CDP round-trip.
    pub fn main_frame(&self) -> crate::frame::Frame {
        let frame_id = self
            .frame_tree
            .read()
            .ok()
            .and_then(|t| t.main_frame().map(|f| f.id.clone()))
            .unwrap_or_else(|| cdp_page::FrameId::new(""));
        crate::frame::Frame::new(
            self.handle.clone(),
            self.session_id.clone(),
            frame_id,
            Arc::clone(&self.frame_tree),
        )
        .with_page(self.clone())
    }

    /// Returns a [`Frame`](crate::frame::Frame) handle for the frame with the given ID, if known.
    pub fn frame_by_id(&self, id: &cdp_page::FrameId) -> Option<crate::frame::Frame> {
        let exists = self.frame_tree.read().ok()?.get(id).is_some();
        if exists {
            Some(
                crate::frame::Frame::new(
                    self.handle.clone(),
                    self.session_id.clone(),
                    id.clone(),
                    Arc::clone(&self.frame_tree),
                )
                .with_page(self.clone()),
            )
        } else {
            None
        }
    }

    /// Returns a [`Frame`](crate::frame::Frame) handle for the frame with the
    /// given `name` attribute, if one exists.
    pub fn frame_by_name(&self, name: &str) -> Option<crate::frame::Frame> {
        let frame_id = self
            .frame_tree
            .read()
            .ok()?
            .all()
            .find(|f| f.name.as_deref() == Some(name))
            .map(|f| f.id.clone())?;
        Some(
            crate::frame::Frame::new(
                self.handle.clone(),
                self.session_id.clone(),
                frame_id,
                Arc::clone(&self.frame_tree),
            )
            .with_page(self.clone()),
        )
    }

    /// Returns [`Frame`](crate::frame::Frame) handles for all frames in the page's frame tree.
    ///
    /// Reads from the local `FrameTree` — no CDP round-trip.
    pub fn frames(&self) -> crate::Result<Vec<crate::frame::Frame>> {
        let ids: Vec<cdp_page::FrameId> = self
            .frame_tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .all()
            .map(|f| f.id.clone())
            .collect();
        let page = self.clone();
        Ok(ids
            .into_iter()
            .map(|id| {
                crate::frame::Frame::new(
                    self.handle.clone(),
                    self.session_id.clone(),
                    id,
                    Arc::clone(&self.frame_tree),
                )
                .with_page(page.clone())
            })
            .collect())
    }

    /// Returns the `name` attribute of the given frame, if present.
    pub fn frame_name(&self, frame_id: &cdp_page::FrameId) -> crate::Result<Option<String>> {
        Ok(self
            .frame_tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .get(frame_id)
            .and_then(|f| f.name.clone()))
    }

    /// Returns the current URL of the given frame.
    pub fn frame_url(&self, frame_id: &cdp_page::FrameId) -> crate::Result<Option<String>> {
        Ok(self
            .frame_tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .get(frame_id)
            .map(|f| f.url.clone()))
    }

    /// Returns the parent frame ID for the given frame, or `None` if it is the root.
    pub fn frame_parent(
        &self,
        frame_id: &cdp_page::FrameId,
    ) -> crate::Result<Option<cdp_page::FrameId>> {
        Ok(self
            .frame_tree
            .read()
            .map_err(|_| CdpError::LockPoisoned)?
            .get(frame_id)
            .and_then(|f| f.parent_id.clone()))
    }

    // ── Element state helpers (locator-delegated; non-strict) ─────────────

    /// Returns `true` if an element matching `selector` is currently visible.
    ///
    /// Equivalent to `page.locator(selector).non_strict().is_visible()` —
    /// does not retry and returns `false` when no element is found.
    pub async fn is_visible(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).non_strict().is_visible().await
    }

    /// Returns `true` if no element matching `selector` is visible.
    ///
    /// A missing element counts as hidden.
    pub async fn is_hidden(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).non_strict().is_hidden().await
    }

    /// Returns `true` if the first element matching `selector` is enabled.
    pub async fn is_enabled(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).non_strict().is_enabled().await
    }

    /// Returns `true` if the first element matching `selector` is disabled.
    pub async fn is_disabled(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).non_strict().is_disabled().await
    }

    /// Returns `true` if the first element matching `selector` can accept text input.
    pub async fn is_editable(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).non_strict().is_editable().await
    }

    /// Returns `true` if the first element matching `selector` is checked.
    pub async fn is_checked(&self, selector: impl Into<String>) -> crate::Result<bool> {
        self.locator(selector).non_strict().is_checked().await
    }
}
