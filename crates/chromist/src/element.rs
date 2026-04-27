//! DOM element handle with click/type/scroll helpers.
//!
//! [`Element`](crate::Element) wraps the `NodeId` / `BackendNodeId` / `RemoteObjectId` triple
//! that CDP uses to identify a DOM node across its three domains (DOM, Runtime,
//! Input). Commands go through [`Page::execute`](crate::page::Page::execute)
//! scoped to the owning session. Bounding-box lookups go through
//! `DOM.getContentQuads` / `DOM.getBoxModel` and are surfaced via
//! [`crate::layout::ElementQuad`] and [`crate::layout::BoxModel`].

use serde_json::Value;

use crate::cdp::browser_protocol::dom as cdp_dom;
use crate::cdp::browser_protocol::input as cdp_input;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::js_protocol::runtime as cdp_runtime;
use crate::error::CdpError;
use crate::handler::{HandlerHandle, SessionRef};
use crate::layout::{BoundingBox, Point};
use crate::page::input::{ClickOptions, MouseButton};

/// Lazy stream of element attribute `(name, Result<Option<value>>)` pairs.
///
/// Backed by a single `DOM.getAttributes` call; values are yielded lazily
/// as the stream is polled. Implements [`futures::Stream`], not [`Iterator`].
#[derive(Debug)]
pub struct AttributeStream {
    inner: std::vec::IntoIter<(String, String)>,
}

impl AttributeStream {
    fn new(attrs: Vec<(String, String)>) -> Self {
        Self { inner: attrs.into_iter() }
    }
}

impl futures::Stream for AttributeStream {
    type Item = (String, crate::Result<Option<String>>);

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.inner.next() {
            Some((name, value)) => std::task::Poll::Ready(Some((name, Ok(Some(value))))),
            None => std::task::Poll::Ready(None),
        }
    }
}

/// A handle to a single DOM node.
///
/// Wraps the three CDP identifiers needed to address a node across `DOM`,
/// `Runtime`, and `Input` domains. Cheaply cloneable. Element references
/// become stale on navigation or when the underlying node is detached —
/// re-resolve via [`Locator`](crate::Locator) for retry-safe access patterns.
#[derive(Debug, Clone)]
pub struct Element {
    handle: HandlerHandle,
    session_id: SessionRef,
    /// CDP `DOM.NodeId` — handle for `DOM.*` commands.
    pub node_id: cdp_dom::NodeId,
    /// Stable backend node id, persisted across session swaps.
    pub backend_node_id: Option<cdp_dom::BackendNodeId>,
    /// CDP `Runtime.RemoteObjectId` — handle for `Runtime.callFunctionOn`.
    pub remote_object_id: cdp_runtime::RemoteObjectId,
    /// Object ID resolved in the utility world (if available). `call_fn` uses
    /// this so function bodies execute with unmodified built-ins, immune to
    /// page-script prototype patching.
    object_id: cdp_runtime::RemoteObjectId,
    /// The utility world's execution context ID, carried so child-element
    /// lookups can also resolve into the same isolated world.
    utility_ctx: Option<cdp_runtime::ExecutionContextId>,
    /// The frame that owns this element, if known.
    pub(crate) frame_id: Option<cdp_page::FrameId>,
}

impl Element {
    /// Resolve a node ID into an Element. If `utility_ctx` is supplied the
    /// node is resolved in the isolated utility world so all `call_fn` calls
    /// use unmodified built-ins, immune to page-script prototype patching.
    pub(crate) async fn from_node_id(
        handle: HandlerHandle,
        session_id: SessionRef,
        node_id: cdp_dom::NodeId,
        utility_ctx: Option<cdp_runtime::ExecutionContextId>,
        frame_id: Option<cdp_page::FrameId>,
    ) -> crate::Result<Self> {
        let mut resolve =
            cdp_dom::ResolveNodeParams { node_id: Some(node_id), ..Default::default() };
        resolve.execution_context_id = utility_ctx;
        let resp = handle.execute(resolve, Some(session_id.current())).await?;
        let object_id = resp.object.object_id.ok_or(CdpError::MissingObjectId)?;
        let describe = cdp_dom::DescribeNodeParams { node_id: Some(node_id), ..Default::default() };
        let backend_node_id = handle
            .execute(describe, Some(session_id.current()))
            .await
            .ok()
            .map(|r| r.node.backend_node_id);
        Ok(Element {
            handle,
            session_id,
            node_id,
            backend_node_id,
            remote_object_id: object_id.clone(),
            object_id,
            utility_ctx,
            frame_id,
        })
    }

    /// Returns the frame ID that owns this element, if known.
    pub fn frame_id(&self) -> Option<&cdp_page::FrameId> {
        self.frame_id.as_ref()
    }

    /// Returns this element's `DOM.NodeId`.
    pub fn node_id(&self) -> &cdp_dom::NodeId {
        &self.node_id
    }

    /// Returns this element's `Runtime.RemoteObjectId` — the handle
    /// passed to `Runtime.callFunctionOn` for JS calls bound to the node.
    pub fn object_id(&self) -> &cdp_runtime::RemoteObjectId {
        &self.object_id
    }

    async fn call_fn(&self, func: &str) -> crate::Result<Value> {
        let mut params = cdp_runtime::CallFunctionOnParams::new(func.to_string());
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(true);
        params.await_promise = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(resp.result.value.unwrap_or(Value::Null))
    }

    async fn call_fn_bool(&self, func: &str) -> crate::Result<bool> {
        Ok(self.call_fn(func).await? == Value::Bool(true))
    }

    /// Verify that the element is visible, stable, and not disabled.
    /// Returns `Err` with a retryable error code when any check fails.
    async fn ensure_actionable(&self) -> crate::Result<()> {
        if !self.call_fn_bool(crate::js::IS_VISIBLE).await? {
            return Err(CdpError::NotFound);
        }
        if !self.call_fn_bool(crate::js::IS_STABLE).await? {
            return Err(CdpError::ScrollingFailed(String::new()));
        }
        if !self.call_fn_bool(crate::js::IS_ENABLED).await? {
            return Err(CdpError::NotFound);
        }
        Ok(())
    }

    /// Verify that the element is actionable (visible, stable, enabled) and
    /// that nothing is obscuring its center point. Retries are expected from
    /// callers — all returned errors are retryable.
    async fn ensure_actionable_and_hittable(&self) -> crate::Result<()> {
        self.ensure_actionable().await?;
        if !self.call_fn_bool(crate::js::HIT_TEST).await? {
            return Err(CdpError::ScrollingFailed(String::new()));
        }
        Ok(())
    }

    /// Returns `true` if the element is visible (non-zero size, not hidden/opacity:0).
    pub async fn is_visible(&self) -> crate::Result<bool> {
        self.call_fn_bool(crate::js::IS_VISIBLE).await
    }

    /// Returns `true` if the element is not disabled.
    pub async fn is_enabled(&self) -> crate::Result<bool> {
        self.call_fn_bool(crate::js::IS_ENABLED).await
    }

    /// Remove focus from the element.
    pub async fn blur(&self) -> crate::Result<()> {
        self.call_fn("function() { this.blur(); return undefined; }").await?;
        Ok(())
    }

    /// Double-click the element (waits for actionability).
    pub async fn dblclick(&self) -> crate::Result<()> {
        self.click_with(ClickOptions { click_count: 2, button: MouseButton::Left }).await
    }

    /// Type `text` character-by-character, dispatching a `keydown`/`char`/`keyup`
    /// sequence for each character so key-event-driven inputs (e.g. autocomplete)
    /// respond correctly.
    pub async fn press_sequentially(&self, text: &str) -> crate::Result<()> {
        self.focus().await?;
        for ch in text.chars() {
            let s = ch.to_string();
            let mut ev =
                cdp_input::DispatchKeyEventParams::new(cdp_input::DispatchKeyEventParamsType::Char);
            ev.text = Some(s.clone());
            ev.unmodified_text = Some(s);
            self.handle.execute(ev, Some(self.session_id.current())).await?;
        }
        Ok(())
    }

    /// Returns `true` if the element is hidden or detached from the document.
    pub async fn is_hidden(&self) -> crate::Result<bool> {
        self.call_fn_bool(crate::js::IS_HIDDEN).await
    }

    /// Returns `true` if the element's `checked` property is truthy.
    pub async fn is_checked(&self) -> crate::Result<bool> {
        self.call_fn_bool(crate::js::IS_CHECKED).await
    }

    /// Returns `true` if the element is disabled.
    pub async fn is_disabled(&self) -> crate::Result<bool> {
        self.call_fn_bool(crate::js::IS_DISABLED).await
    }

    /// Returns `true` if the element can accept text input (not disabled, not read-only).
    pub async fn is_editable(&self) -> crate::Result<bool> {
        self.call_fn_bool(crate::js::IS_EDITABLE).await
    }

    /// Returns the current value of an `<input>`, `<textarea>`, or `<select>` element.
    pub async fn input_value(&self) -> crate::Result<String> {
        Ok(self.string_property("value").await?.unwrap_or_default())
    }

    /// Returns the element's `innerText` (visible text, respecting
    /// `display:none` and CSS `text-transform`).
    pub async fn inner_text(&self) -> crate::Result<String> {
        let v = self.call_fn(crate::js::ELEMENT_INNER_TEXT).await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// Returns the element's `textContent` (raw text including hidden
    /// nodes; faster than `innerText` and not reflowed).
    pub async fn text_content(&self) -> crate::Result<String> {
        let v = self.call_fn(crate::js::ELEMENT_TEXT_CONTENT).await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// Returns the value of the named HTML attribute, or `None` if absent.
    pub async fn attribute(&self, name: &str) -> crate::Result<Option<String>> {
        let func = format!("function() {{ return this.getAttribute({:?}); }}", name);
        let v = self.call_fn(&func).await?;
        Ok(match v {
            Value::String(s) => Some(s),
            _ => None,
        })
    }

    /// Returns every attribute on this element as `(name, value)` pairs.
    pub async fn attributes(&self) -> crate::Result<Vec<(String, String)>> {
        let params = cdp_dom::GetAttributesParams::new(self.node_id);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let mut out = Vec::new();
        let mut it = resp.attributes.into_iter();
        while let (Some(k), Some(v)) = (it.next(), it.next()) {
            out.push((k, v));
        }
        Ok(out)
    }

    /// Returns the element's axis-aligned bounding rectangle in CSS
    /// pixels, or `None` if it has no layout (display:none, detached).
    pub async fn bounding_box(&self) -> crate::Result<Option<BoundingBox>> {
        let params =
            cdp_dom::GetBoxModelParams { node_id: Some(self.node_id), ..Default::default() };
        match self.handle.execute(params, Some(self.session_id.current())).await {
            Ok(resp) => Ok(BoundingBox::from_quad(&resp.model.border.0)),
            Err(CdpError::Cdp { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Returns a viewport-coordinate point inside the element suitable
    /// for synthesised click events. Scrolls the element into view first.
    pub async fn clickable_point(&self) -> crate::Result<Point> {
        let params = cdp_dom::GetContentQuadsParams {
            object_id: Some(self.object_id.clone()),
            ..Default::default()
        };
        match self.handle.execute(params, Some(self.session_id.current())).await {
            Ok(resp) if !resp.quads.is_empty() => {
                let quad = &resp.quads[0].0;
                if quad.len() >= 8 {
                    let x = (quad[0] + quad[2] + quad[4] + quad[6]) / 4.0;
                    let y = (quad[1] + quad[3] + quad[5] + quad[7]) / 4.0;
                    return Ok(Point::new(x, y));
                }
                Ok(Point::new(0.0, 0.0))
            }
            _ => {
                let v = self.call_fn(crate::js::CLICKABLE_POINT).await?;
                let x = v.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let y = v.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
                Ok(Point::new(x, y))
            }
        }
    }

    /// Scroll the element into view if it is not already visible. Uses
    /// `block:'center'` and `inline:'center'` with instant behaviour.
    pub async fn scroll_into_view(&self) -> crate::Result<()> {
        let func = r#"async function() {
        if (!this.isConnected || this.nodeType !== Node.ELEMENT_NODE) return;
        const visible = await new Promise(resolve => {
            const io = new IntersectionObserver(entries => {
                resolve(entries[0].isIntersecting);
                io.disconnect();
            });
            io.observe(this);
        });
        if (!visible) this.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' });
    }"#;
        let mut params =
            crate::cdp::js_protocol::runtime::CallFunctionOnParams::new(func.to_string());
        params.object_id = Some(self.object_id.clone());
        params.await_promise = Some(true);
        let _ = self.handle.execute(params, Some(self.session_id.current())).await;
        Ok(())
    }

    /// Move keyboard focus to the element via `HTMLElement.focus()`.
    pub async fn focus(&self) -> crate::Result<()> {
        self.call_fn("function() { this.focus(); return undefined; }").await?;
        Ok(())
    }

    /// Single-click the element after waiting for actionability
    /// (visible, stable, enabled, hit-testable).
    pub async fn click(&self) -> crate::Result<()> {
        self.click_with(ClickOptions::default()).await
    }

    /// Send `text` as a sequence of keyboard events to the focused
    /// element. For form inputs that need `keydown`/`char`/`keyup` per
    /// character, use [`Self::press_sequentially`].
    pub async fn type_text(&self, text: &str) -> crate::Result<()> {
        let progress = crate::progress::Progress::default();
        loop {
            progress.check()?;
            self.scroll_into_view().await.ok();
            match self.ensure_actionable().await {
                Ok(()) => break,
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        self.focus().await?;
        let params = cdp_input::InsertTextParams::new(text.to_string());
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Set the value of a `<select>` element and fire `input` + `change` events.
    pub async fn select_option(&self, value: &str) -> crate::Result<()> {
        let progress = crate::progress::Progress::default();
        loop {
            progress.check()?;
            self.scroll_into_view().await.ok();
            match self.ensure_actionable().await {
                Ok(()) => break,
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let func = format!(
            "function(){{this.value={:?};this.dispatchEvent(new Event('input',{{bubbles:true}}));this.dispatchEvent(new Event('change',{{bubbles:true}}));}}",
            value
        );
        self.call_fn(&func).await?;
        Ok(())
    }

    /// Check a checkbox or radio button (no-op if already checked).
    pub async fn check(&self) -> crate::Result<()> {
        if self.call_fn_bool("function(){return !!this.checked;}").await? {
            return Ok(());
        }
        self.click_with(ClickOptions::default()).await
    }

    /// Uncheck a checkbox (no-op if already unchecked).
    pub async fn uncheck(&self) -> crate::Result<()> {
        if !self.call_fn_bool("function(){return !!this.checked;}").await? {
            return Ok(());
        }
        self.click_with(ClickOptions::default()).await
    }

    /// Clear the element's value, firing `input` and `change` events.
    ///
    /// Works for `<input>`, `<textarea>`, and `contenteditable` elements.
    pub async fn clear(&self) -> crate::Result<()> {
        self.focus().await?;
        self.call_fn(
            "function(){if('value' in this){this.value='';this.dispatchEvent(new Event('input',{bubbles:true}));this.dispatchEvent(new Event('change',{bubbles:true}));}else{this.textContent='';}}",
        ).await?;
        Ok(())
    }

    /// Clear the element and type `value`, firing `input` events.
    ///
    /// Waits for actionability (visible + stable + enabled) before dispatching.
    pub async fn fill(&self, value: &str) -> crate::Result<()> {
        let progress = crate::progress::Progress::default();
        loop {
            progress.check()?;
            self.scroll_into_view().await.ok();
            match self.ensure_actionable().await {
                Ok(()) => break,
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        self.focus().await?;
        // Clear by setting value = '' and fire input event so frameworks react.
        self.call_fn(
            "function(){if('value' in this){this.value='';this.dispatchEvent(new Event('input',{bubbles:true}));}else{this.textContent='';}}",
        ).await.ok();
        if !value.is_empty() {
            let params =
                crate::cdp::browser_protocol::input::InsertTextParams::new(value.to_string());
            self.handle.execute(params, Some(self.session_id.current())).await?;
        }
        Ok(())
    }

    /// Set files on a `<input type="file">` element via `DOM.setFileInputFiles`.
    ///
    /// `files` is an iterator of absolute filesystem paths (as strings or
    /// [`Path`](std::path::Path)-like values). The element must be a file input;
    /// passing non-file inputs returns a CDP protocol error.
    pub async fn set_input_files(
        &self,
        files: impl IntoIterator<Item = impl Into<String>>,
    ) -> crate::Result<()> {
        let files: Vec<String> = files.into_iter().map(|f| f.into()).collect();
        let mut params = cdp_dom::SetFileInputFilesParams::new(files);
        params.node_id = Some(self.node_id);
        self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Perform a touch-tap at the element's center.
    ///
    /// Waits for actionability and hittability, scrolls into view, then dispatches
    /// `touchstart` + `touchend` via `Input.dispatchTouchEvent`.
    pub async fn tap(&self) -> crate::Result<()> {
        let progress = crate::progress::Progress::default();
        loop {
            progress.check()?;
            self.scroll_into_view().await.ok();
            match self.ensure_actionable_and_hittable().await {
                Ok(()) => break,
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let p = self.clickable_point().await?;
        use crate::cdp::browser_protocol::input as cdp_input;
        let touch_point = cdp_input::TouchPoint::new(p.x, p.y);
        let start = cdp_input::DispatchTouchEventParams::new(
            cdp_input::DispatchTouchEventParamsType::TouchStart,
            vec![touch_point],
        );
        self.handle.execute(start, Some(self.session_id.current())).await?;
        let end = cdp_input::DispatchTouchEventParams::new(
            cdp_input::DispatchTouchEventParamsType::TouchEnd,
            vec![],
        );
        self.handle.execute(end, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Dispatch a synthetic DOM event on this element.
    ///
    /// `event_type` is the event name (e.g. `"click"`, `"input"`, `"custom"`).
    /// `init` is an optional JSON object that is `Object.assign`-ed onto the
    /// event's `EventInit` dictionary (`bubbles` and `cancelable` default to
    /// `true`).
    pub async fn dispatch_event(
        &self,
        event_type: &str,
        init: Option<&serde_json::Value>,
    ) -> crate::Result<()> {
        let init_str = init.map(|v| v.to_string()).unwrap_or_else(|| "{}".to_string());
        let func = format!(
            "function(){{this.dispatchEvent(new Event({:?},Object.assign({{bubbles:true,cancelable:true}},{init_str})));}}",
            event_type
        );
        self.call_fn(&func).await?;
        Ok(())
    }

    /// Evaluate a JS function with this element bound as `this`, returning the JSON result.
    pub async fn evaluate(&self, func: impl Into<String>) -> crate::Result<serde_json::Value> {
        self.call_fn(&func.into()).await
    }

    /// Press a single key on this focused element. `key` is a name
    /// from [`USKEYBOARD_LAYOUT`](crate::USKEYBOARD_LAYOUT) (e.g.
    /// `"Enter"`, `"Tab"`, `"ArrowDown"`) or a literal character.
    pub async fn press(&self, key: &str) -> crate::Result<()> {
        self.focus().await?;
        let session = Some(self.session_id.current());

        let (key_str, key_code, code, text) = if let Some(def) = crate::keys::key_definition(key) {
            (def.key, def.key_code, def.code, def.text)
        } else {
            (key, 0, "", None)
        };

        let mut down =
            cdp_input::DispatchKeyEventParams::new(cdp_input::DispatchKeyEventParamsType::KeyDown);
        down.key = Some(key_str.to_string());
        down.windows_virtual_key_code = Some(key_code);
        down.code = Some(code.to_string());
        if let Some(t) = text {
            down.text = Some(t.to_string());
        }
        self.handle.execute(down, session.clone()).await?;

        if let Some(t) = text {
            let mut char_event =
                cdp_input::DispatchKeyEventParams::new(cdp_input::DispatchKeyEventParamsType::Char);
            char_event.key = Some(key_str.to_string());
            char_event.text = Some(t.to_string());
            char_event.windows_virtual_key_code = Some(key_code);
            char_event.code = Some(code.to_string());
            self.handle.execute(char_event, session.clone()).await?;
        }

        let mut up =
            cdp_input::DispatchKeyEventParams::new(cdp_input::DispatchKeyEventParamsType::KeyUp);
        up.key = Some(key_str.to_string());
        up.windows_virtual_key_code = Some(key_code);
        up.code = Some(code.to_string());
        self.handle.execute(up, session).await?;
        Ok(())
    }

    /// Returns `innerHTML` of this element.
    pub async fn inner_html(&self) -> crate::Result<String> {
        let v = self.call_fn(crate::js::ELEMENT_INNER_HTML).await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// Returns `outerHTML` of this element.
    pub async fn outer_html(&self) -> crate::Result<String> {
        let v = self.call_fn(crate::js::ELEMENT_OUTER_HTML).await?;
        Ok(v.as_str().unwrap_or("").to_string())
    }

    /// Move mouse to center of element (hover).
    pub async fn hover(&self) -> crate::Result<()> {
        let progress = crate::progress::Progress::default();
        loop {
            progress.check()?;
            self.scroll_into_view().await.ok();
            match self.ensure_actionable_and_hittable().await {
                Ok(()) => break,
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let p = self.clickable_point().await?;
        let moved = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseMoved,
            p.x,
            p.y,
        );
        self.handle.execute(moved, Some(self.session_id.current())).await?;
        Ok(())
    }

    /// Drag this element to `target` by simulating mousedown → move → mouseup.
    ///
    /// Scrolls both elements into view, waits for actionability on the source,
    /// then moves the mouse in 10 steps so drag-enter/drag-over events fire on
    /// intermediate elements.
    pub async fn drag_to(&self, target: &Element) -> crate::Result<()> {
        let progress = crate::progress::Progress::default();
        loop {
            progress.check()?;
            self.scroll_into_view().await.ok();
            match self.ensure_actionable().await {
                Ok(()) => break,
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
        target.scroll_into_view().await.ok();

        let src = self.clickable_point().await?;
        let dst = target.clickable_point().await?;
        let session = Some(self.session_id.current());

        let moved = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseMoved,
            src.x,
            src.y,
        );
        self.handle.execute(moved, session.clone()).await?;

        let mut pressed = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MousePressed,
            src.x,
            src.y,
        );
        pressed.button = Some(cdp_input::MouseButton::Left);
        pressed.click_count = Some(1);
        self.handle.execute(pressed, session.clone()).await?;

        let steps = 10i32;
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let x = src.x + (dst.x - src.x) * t;
            let y = src.y + (dst.y - src.y) * t;
            let mut mv = cdp_input::DispatchMouseEventParams::new(
                cdp_input::DispatchMouseEventParamsType::MouseMoved,
                x,
                y,
            );
            mv.button = Some(cdp_input::MouseButton::Left);
            self.handle.execute(mv, session.clone()).await?;
        }

        let mut released = cdp_input::DispatchMouseEventParams::new(
            cdp_input::DispatchMouseEventParamsType::MouseReleased,
            dst.x,
            dst.y,
        );
        released.button = Some(cdp_input::MouseButton::Left);
        released.click_count = Some(1);
        self.handle.execute(released, session.clone()).await?;
        Ok(())
    }

    /// Click with custom options (e.g. double-click, right-click).
    pub async fn click_with(&self, options: ClickOptions) -> crate::Result<()> {
        let progress = crate::progress::Progress::default();
        loop {
            progress.check()?;
            self.scroll_into_view().await.ok();
            match self.ensure_actionable_and_hittable().await {
                Ok(()) => break,
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        let p = self.clickable_point().await?;
        let session = Some(self.session_id.current());
        let count = options.click_count.max(1);
        for _ in 0..count {
            let mut down = cdp_input::DispatchMouseEventParams::new(
                cdp_input::DispatchMouseEventParamsType::MousePressed,
                p.x,
                p.y,
            );
            down.button = Some(mouse_button_to_cdp(&options.button));
            down.click_count = Some(count as i64);
            self.handle.execute(down, session.clone()).await?;

            let mut up = cdp_input::DispatchMouseEventParams::new(
                cdp_input::DispatchMouseEventParamsType::MouseReleased,
                p.x,
                p.y,
            );
            up.button = Some(mouse_button_to_cdp(&options.button));
            up.click_count = Some(count as i64);
            self.handle.execute(up, session.clone()).await?;
        }
        Ok(())
    }

    /// Call a JS function with `this` bound to this element; returns the full CDP response.
    pub async fn call_js_fn(
        &self,
        func: impl Into<String>,
        await_promise: bool,
    ) -> crate::Result<cdp_runtime::CallFunctionOnResponse> {
        let mut params = cdp_runtime::CallFunctionOnParams::new(func.into());
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(true);
        params.await_promise = Some(await_promise);
        self.handle.execute(params, Some(self.session_id.current())).await
    }

    /// Returns the JSON value of this remote object.
    pub async fn json_value(&self) -> crate::Result<serde_json::Value> {
        let mut params = cdp_runtime::CallFunctionOnParams::new("function(){return this;}");
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(resp.result.value.unwrap_or(serde_json::Value::Null))
    }

    /// Returns all properties of this remote object as a map.
    pub async fn properties(
        &self,
    ) -> crate::Result<std::collections::HashMap<String, cdp_runtime::PropertyDescriptor>> {
        let mut params = cdp_runtime::GetPropertiesParams::new(self.object_id.clone());
        params.own_properties = Some(true);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let mut map = std::collections::HashMap::new();
        for prop in resp.result {
            map.insert(prop.name.clone(), prop);
        }
        Ok(map)
    }

    /// Find a single descendant element by CSS selector.
    pub async fn find_element(&self, selector: &str) -> crate::Result<Element> {
        let func = format!("function(){{return this.querySelector({:?});}}", selector);
        let mut params = cdp_runtime::CallFunctionOnParams::new(func);
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(false);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let Some(oid) = resp.result.object_id else {
            return Err(CdpError::NotFound);
        };
        let req = cdp_dom::RequestNodeParams::new(oid);
        let node_resp = self.handle.execute(req, Some(self.session_id.current())).await?;
        if node_resp.node_id.0 == 0 {
            return Err(CdpError::NotFound);
        }
        Element::from_node_id(
            self.handle.clone(),
            self.session_id.clone(),
            node_resp.node_id,
            self.utility_ctx,
            self.frame_id.clone(),
        )
        .await
    }

    /// Find all descendant elements matching a CSS selector.
    pub async fn find_elements(&self, selector: &str) -> crate::Result<Vec<Element>> {
        let func =
            format!("function(){{return Array.from(this.querySelectorAll({:?}));}}", selector);
        let mut params = cdp_runtime::CallFunctionOnParams::new(func);
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(false);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let Some(arr_oid) = resp.result.object_id else {
            return Ok(Vec::new());
        };
        let mut len_params =
            cdp_runtime::CallFunctionOnParams::new("function(){return this.length;}");
        len_params.object_id = Some(arr_oid.clone());
        len_params.return_by_value = Some(true);
        let len_resp = self.handle.execute(len_params, Some(self.session_id.current())).await?;
        let len = len_resp.result.value.and_then(|v| v.as_u64()).unwrap_or(0);
        let mut out = Vec::new();
        for i in 0..len {
            let func = format!("function(){{return this[{}];}}", i);
            let mut p = cdp_runtime::CallFunctionOnParams::new(func);
            p.object_id = Some(arr_oid.clone());
            p.return_by_value = Some(false);
            let r = self.handle.execute(p, Some(self.session_id.current())).await?;
            if let Some(oid) = r.result.object_id {
                let req = cdp_dom::RequestNodeParams::new(oid);
                let node_resp = self.handle.execute(req, Some(self.session_id.current())).await?;
                if node_resp.node_id.0 != 0 {
                    if let Ok(el) = Element::from_node_id(
                        self.handle.clone(),
                        self.session_id.clone(),
                        node_resp.node_id,
                        self.utility_ctx,
                        self.frame_id.clone(),
                    )
                    .await
                    {
                        out.push(el);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Takes a screenshot of only this element (scrolls into view first).
    pub async fn screenshot(&self) -> crate::Result<Vec<u8>> {
        self.scroll_into_view().await?;
        let bb = self.bounding_box().await?;
        let mut p = crate::cdp::browser_protocol::page::CaptureScreenshotParams::default();
        if let Some(bb) = bb {
            let metrics = self
                .handle
                .execute(
                    crate::cdp::browser_protocol::page::GetLayoutMetricsParams::default(),
                    Some(self.session_id.current()),
                )
                .await
                .ok();
            let scroll_x = metrics.as_ref().map(|m| m.css_visual_viewport.page_x).unwrap_or(0.0);
            let scroll_y = metrics.as_ref().map(|m| m.css_visual_viewport.page_y).unwrap_or(0.0);
            p.clip = Some(crate::cdp::browser_protocol::page::Viewport {
                x: bb.x + scroll_x,
                y: bb.y + scroll_y,
                width: bb.width,
                height: bb.height,
                scale: 1.0,
            });
        }
        let resp = self.handle.execute(p, Some(self.session_id.current())).await?;
        Ok(resp.data.into_bytes())
    }

    /// Returns the full CSS box model (content / padding / border /
    /// margin quads + width / height) for this element.
    pub async fn box_model(&self) -> crate::Result<crate::layout::BoxModel> {
        use crate::layout::{BoxModel, ElementQuad};
        let params = crate::cdp::browser_protocol::dom::GetBoxModelParams {
            object_id: Some(self.object_id.clone()),
            ..Default::default()
        };
        let resp = self
            .handle
            .execute(params, Some(self.session_id.current()))
            .await
            .map_err(|_| CdpError::NotFound)?;
        let m = resp.model;
        Ok(BoxModel {
            content: ElementQuad::from_raw(&m.content.0).ok_or(CdpError::NotFound)?,
            padding: ElementQuad::from_raw(&m.padding.0).ok_or(CdpError::NotFound)?,
            border: ElementQuad::from_raw(&m.border.0).ok_or(CdpError::NotFound)?,
            margin: ElementQuad::from_raw(&m.margin.0).ok_or(CdpError::NotFound)?,
            width: m.width.max(0) as u32,
            height: m.height.max(0) as u32,
        })
    }

    /// Returns the full DOM-node description of this element (100-depth describe).
    pub async fn description(&self) -> crate::Result<crate::DomNode> {
        let params = cdp_dom::DescribeNodeParams {
            node_id: Some(self.node_id),
            depth: Some(100),
            ..Default::default()
        };
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(resp.node.into())
    }

    /// Returns the value of a named JS property on this element as a `String`.
    pub async fn string_property(&self, name: &str) -> crate::Result<Option<String>> {
        let func = format!(
            "function() {{ var v = this[{:?}]; return v == null ? null : String(v); }}",
            name
        );
        let resp = self.call_js_fn(func, false).await?;
        Ok(match resp.result.value {
            Some(serde_json::Value::String(s)) => Some(s),
            Some(serde_json::Value::Null) | None => None,
            Some(v) => Some(v.to_string()),
        })
    }

    /// Returns the value of a named JS property on this element as JSON.
    pub async fn property(&self, name: &str) -> crate::Result<Option<serde_json::Value>> {
        let func = format!(
            "function() {{ var v = this[{:?}]; return v === undefined ? null : v; }}",
            name
        );
        let resp = self.call_js_fn(func, false).await?;
        Ok(match resp.result.value {
            Some(serde_json::Value::Null) | None => None,
            Some(v) => Some(v),
        })
    }

    /// Find the first descendant matching an XPath expression.
    pub async fn find_xpath(&self, xpath: &str) -> crate::Result<Element> {
        let results = self.find_xpaths(xpath).await?;
        results.into_iter().next().ok_or(CdpError::NotFound)
    }

    /// Find every descendant matching an XPath expression.
    pub async fn find_xpaths(&self, xpath: &str) -> crate::Result<Vec<Element>> {
        let func = format!(
            "function(){{const r=document.evaluate({:?},this,null,XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,null);const out=[];for(let i=0;i<r.snapshotLength;i++)out.push(r.snapshotItem(i));return out;}}",
            xpath
        );
        let mut params = cdp_runtime::CallFunctionOnParams::new(func);
        params.object_id = Some(self.object_id.clone());
        params.return_by_value = Some(false);
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        let Some(arr_oid) = resp.result.object_id else {
            return Ok(Vec::new());
        };
        let mut len_params =
            cdp_runtime::CallFunctionOnParams::new("function(){return this.length;}");
        len_params.object_id = Some(arr_oid.clone());
        len_params.return_by_value = Some(true);
        let len_resp = self.handle.execute(len_params, Some(self.session_id.current())).await?;
        let len = len_resp.result.value.and_then(|v| v.as_u64()).unwrap_or(0);
        let mut out = Vec::new();
        for i in 0..len {
            let func = format!("function(){{return this[{}];}}", i);
            let mut p = cdp_runtime::CallFunctionOnParams::new(func);
            p.object_id = Some(arr_oid.clone());
            p.return_by_value = Some(false);
            let r = self.handle.execute(p, Some(self.session_id.current())).await?;
            if let Some(oid) = r.result.object_id {
                let req = cdp_dom::RequestNodeParams::new(oid);
                let node_resp = self.handle.execute(req, Some(self.session_id.current())).await?;
                if node_resp.node_id.0 != 0 {
                    if let Ok(el) = Element::from_node_id(
                        self.handle.clone(),
                        self.session_id.clone(),
                        node_resp.node_id,
                        self.utility_ctx,
                        self.frame_id.clone(),
                    )
                    .await
                    {
                        out.push(el);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Capture a screenshot clipped to this element with custom
    /// [`ScreenshotParams`](crate::ScreenshotParams).
    pub async fn screenshot_with(
        &self,
        format: crate::screenshot::ScreenshotFormat,
    ) -> crate::Result<Vec<u8>> {
        self.scroll_into_view().await?;
        let bb = self.bounding_box().await?;
        let mut p = crate::cdp::browser_protocol::page::CaptureScreenshotParams {
            format: Some(match format {
                crate::screenshot::ScreenshotFormat::Jpeg => {
                    crate::cdp::browser_protocol::page::CaptureScreenshotParamsFormat::Jpeg
                }
                crate::screenshot::ScreenshotFormat::Webp => {
                    crate::cdp::browser_protocol::page::CaptureScreenshotParamsFormat::Webp
                }
                crate::screenshot::ScreenshotFormat::Png => {
                    crate::cdp::browser_protocol::page::CaptureScreenshotParamsFormat::Png
                }
            }),
            ..Default::default()
        };
        if let Some(bb) = bb {
            p.clip = Some(crate::cdp::browser_protocol::page::Viewport {
                x: bb.x,
                y: bb.y,
                width: bb.width,
                height: bb.height,
                scale: 1.0,
            });
        }
        let resp = self.handle.execute(p, Some(self.session_id.current())).await?;
        Ok(resp.data.into_bytes())
    }

    /// Save a screenshot of this element to a file.
    pub async fn save_screenshot(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> crate::Result<Vec<u8>> {
        let data = self.screenshot().await?;
        tokio::fs::write(path, &data).await.map_err(CdpError::Io)?;
        Ok(data)
    }

    /// Returns a `Stream<Item = (name, Result<Option<value>>)>` over this element's attributes.
    ///
    /// Fetches all attributes via a single `DOM.getAttributes` CDP call and streams
    /// them lazily. Use `attributes` instead if you need a plain `Vec`.
    pub async fn iter_attributes(&self) -> crate::Result<AttributeStream> {
        Ok(AttributeStream::new(self.attributes().await?))
    }

    /// Alias for `iter_attributes`.
    pub async fn attributes_stream(&self) -> crate::Result<AttributeStream> {
        self.iter_attributes().await
    }
}

fn mouse_button_to_cdp(b: &MouseButton) -> cdp_input::MouseButton {
    match b {
        MouseButton::Left => cdp_input::MouseButton::Left,
        MouseButton::Right => cdp_input::MouseButton::Right,
        MouseButton::Middle => cdp_input::MouseButton::Middle,
        MouseButton::None => cdp_input::MouseButton::Left,
    }
}
