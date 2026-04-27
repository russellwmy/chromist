use std::time::Duration;

use crate::element::Element;
use crate::error::CdpError;
use crate::frame::Frame;
use crate::progress::Progress;

/// Re-resolves a DOM element by selector on every method call.
///
/// Unlike [`Element`](crate::Element), a `Locator` never caches a `NodeId` — each action
/// re-queries the document, so the locator stays valid across navigations and
/// DOM mutations.
///
/// In **strict mode** (the default), [`CdpError::StrictViolation`] is returned
/// when the selector matches more than one element.  Call [`non_strict`] to
/// disable this check and act on the first match.
///
/// [`non_strict`]: Locator::non_strict
#[derive(Debug, Clone)]
pub struct Locator {
    frame: Frame,
    selector: String,
    /// When set, resolution uses `Frame::find_elements_by_js` instead of CSS
    /// `querySelectorAll`. This supports semantic queries such as `get_by_text`.
    js_selector: Option<String>,
    strict: bool,
    /// When `Some(n)`, `resolve_one` returns the *n*th match (0-indexed).
    /// Negative values resolve from the end (e.g. `-1` = last).
    /// `None` means strict/first-match semantics.
    index: Option<i64>,
}

impl Locator {
    pub(crate) fn new(frame: Frame, selector: impl Into<String>) -> Self {
        Self { frame, selector: selector.into(), js_selector: None, strict: true, index: None }
    }

    /// Construct a locator backed by a JavaScript expression that returns a
    /// `NodeList` or `Array` of elements. Used by the `get_by_*` family.
    pub(crate) fn new_with_js(frame: Frame, js: String) -> Self {
        Self { frame, selector: String::new(), js_selector: Some(js), strict: true, index: None }
    }

    /// Disable strict mode — act on the first match when the selector is
    /// ambiguous rather than returning [`CdpError::StrictViolation`].
    pub fn non_strict(mut self) -> Self {
        self.strict = false;
        self
    }

    /// Returns the JavaScript expression that resolves this locator's elements
    /// as an `Array`.
    fn to_js_expression(&self) -> String {
        if let Some(js) = &self.js_selector {
            js.clone()
        } else {
            format!("Array.from(document.querySelectorAll({:?}))", self.selector)
        }
    }

    async fn resolve_one(&self) -> crate::Result<Element> {
        let elements = if let Some(js) = &self.js_selector {
            self.frame.find_elements_by_js(js).await?
        } else {
            self.frame.find_elements(&self.selector).await?
        };
        match self.index {
            Some(i) if i >= 0 => elements.into_iter().nth(i as usize).ok_or(CdpError::NotFound),
            Some(_) => elements.into_iter().last().ok_or(CdpError::NotFound),
            None => {
                if self.strict && elements.len() > 1 {
                    return Err(CdpError::StrictViolation);
                }
                elements.into_iter().next().ok_or(CdpError::NotFound)
            }
        }
    }

    /// Resolve all elements matched by this locator.
    async fn resolve_all(&self) -> crate::Result<Vec<Element>> {
        if let Some(js) = &self.js_selector {
            self.frame.find_elements_by_js(js).await
        } else {
            self.frame.find_elements(&self.selector).await
        }
    }

    // ── Semantic get_by_* helpers ─────────────────────────────────────────

    /// Returns a locator that finds elements containing the given exact text.
    pub fn get_by_text(&self, text: impl Into<String>) -> Locator {
        Locator::new_with_js(self.frame.clone(), js_get_by_text(&text.into()))
    }

    /// Returns a locator that finds elements with the given ARIA role,
    /// optionally filtered by accessible name.
    pub fn get_by_role(&self, role: impl Into<String>, name: Option<&str>) -> Locator {
        Locator::new_with_js(self.frame.clone(), js_get_by_role(&role.into(), name))
    }

    /// Returns a locator that finds form controls associated with the given label text.
    pub fn get_by_label(&self, label: impl Into<String>) -> Locator {
        Locator::new_with_js(self.frame.clone(), js_get_by_label(&label.into()))
    }

    /// Returns a locator that finds elements with the given `placeholder` attribute value.
    pub fn get_by_placeholder(&self, text: impl Into<String>) -> Locator {
        Locator::new_with_js(self.frame.clone(), js_get_by_placeholder(&text.into()))
    }

    /// Returns a locator that finds elements with the given `data-testid` attribute value.
    pub fn get_by_test_id(&self, id: impl Into<String>) -> Locator {
        Locator::new_with_js(self.frame.clone(), js_get_by_test_id(&id.into()))
    }

    async fn resolve_with_retry(&self, progress: &Progress) -> crate::Result<Element> {
        loop {
            progress.check()?;
            match self.resolve_one().await {
                Ok(el) => return Ok(el),
                Err(CdpError::StrictViolation) => return Err(CdpError::StrictViolation),
                Err(e) if e.is_retryable() => {
                    crate::runtime::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    // ── Index / slice helpers ─────────────────────────────────────────────

    /// Returns a locator that resolves the *n*th match (0-indexed).
    ///
    /// Disables strict mode for this locator since an index is an explicit
    /// disambiguation.
    pub fn nth(&self, n: usize) -> Locator {
        Locator { index: Some(n as i64), strict: false, ..self.clone() }
    }

    /// Returns a locator pinned to the first match.  Equivalent to `nth(0)`.
    pub fn first(&self) -> Locator {
        Locator { index: Some(0), strict: false, ..self.clone() }
    }

    /// Returns a locator pinned to the last match.
    pub fn last(&self) -> Locator {
        Locator { index: Some(-1), strict: false, ..self.clone() }
    }

    // ── Counting / enumeration ────────────────────────────────────────────

    /// Returns the number of elements currently matched by this locator.
    ///
    /// Does not apply strict-mode rules — returns `0` when nothing is found.
    pub async fn count(&self) -> crate::Result<usize> {
        Ok(self.resolve_all().await?.len())
    }

    /// Resolves all matches and returns one `Locator` per element, each pinned
    /// to its index via [`nth`].
    ///
    /// [`nth`]: Locator::nth
    pub async fn all(&self) -> crate::Result<Vec<Locator>> {
        let count = self.resolve_all().await?.len();
        Ok((0..count)
            .map(|i| Locator { index: Some(i as i64), strict: false, ..self.clone() })
            .collect())
    }

    // ── Filtering ─────────────────────────────────────────────────────────

    /// Returns a narrowed locator whose resolution is filtered to elements
    /// whose `textContent` contains `text`.
    pub fn filter_has_text(&self, text: impl Into<String>) -> Locator {
        let text = text.into();
        let base = self.to_js_expression();
        let js = format!("({base}).filter(el => el.textContent.includes({:?}))", text);
        Locator::new_with_js(self.frame.clone(), js)
    }

    /// Returns a narrowed locator whose resolution is filtered to elements
    /// that have at least one descendant matching `selector`.
    pub fn filter_has(&self, selector: impl Into<String>) -> Locator {
        let sel = selector.into();
        let base = self.to_js_expression();
        let js = format!("({base}).filter(el => !!el.querySelector({:?}))", sel);
        Locator::new_with_js(self.frame.clone(), js)
    }

    /// Generic filter: supply `has_text` and/or `has_selector` to narrow the
    /// matched set.  Both filters are applied when both are provided.
    pub fn filter(&self, has_text: Option<&str>, has_selector: Option<&str>) -> Locator {
        let mut loc = self.clone();
        if let Some(text) = has_text {
            loc = loc.filter_has_text(text);
        }
        if let Some(sel) = has_selector {
            loc = loc.filter_has(sel);
        }
        loc
    }

    /// Return a new locator scoped to descendants of the matched element(s).
    pub fn locator(&self, selector: impl Into<String>) -> Locator {
        let sel = selector.into();
        let base = self.to_js_expression();
        let js = format!("({base}).flatMap(el => Array.from(el.querySelectorAll({sel:?})))");
        Locator::new_with_js(self.frame.clone(), js)
    }

    /// Return the intersection of this locator and `other` — only elements
    /// matched by *both* are returned.
    pub fn and(&self, other: &Locator) -> Locator {
        let self_js = self.to_js_expression();
        let other_js = other.to_js_expression();
        let js = format!(
            "(function() {{ const b = new Set({other_js}); return ({self_js}).filter(el => b.has(el)); }})()"
        );
        Locator::new_with_js(self.frame.clone(), js)
    }

    /// Return a [`FrameLocator`](crate::frame_locator::FrameLocator) scoped to
    /// the iframe matched by `selector` within this locator's frame.
    pub fn frame_locator(&self, selector: impl Into<String>) -> crate::frame_locator::FrameLocator {
        crate::frame_locator::FrameLocator::new(self.frame.clone(), selector)
    }

    /// Resolve the iframe element matched by this locator and return a
    /// [`Frame`](crate::frame::Frame) for its content document.
    ///
    /// Returns `Ok(None)` when the element is not found or has no associated
    /// frame (i.e. is not an `<iframe>` / `<frame>`).
    pub async fn content_frame(&self) -> crate::Result<Option<crate::frame::Frame>> {
        let el = match self.resolve_one().await {
            Ok(el) => el,
            Err(CdpError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let params = crate::cdp::browser_protocol::dom::DescribeNodeParams {
            node_id: Some(el.node_id),
            ..Default::default()
        };
        let resp = self.frame.handle.execute(params, Some(self.frame.session_id.current())).await?;
        if let Some(frame_id) = resp.node.frame_id {
            let frame = crate::frame::Frame::new(
                self.frame.handle.clone(),
                self.frame.session_id.clone(),
                frame_id,
                std::sync::Arc::clone(&self.frame.frame_tree),
            );
            Ok(Some(frame))
        } else {
            Ok(None)
        }
    }

    // ── Actions ───────────────────────────────────────────────────────────

    /// Wait for the element to appear in the DOM and return it.
    pub async fn wait_for(&self) -> crate::Result<Element> {
        self.resolve_with_retry(&Progress::default()).await
    }

    /// Re-resolve and click the element. Retries on stale-node errors
    /// until the locator's timeout elapses.
    pub async fn click(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.click().await
    }

    /// Re-resolve and move the mouse cursor over the element.
    pub async fn hover(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.hover().await
    }

    /// Re-resolve, focus, and type `text` into the element.
    pub async fn type_text(&self, text: &str) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.type_text(text).await
    }

    /// Click the matched checkbox / radio so its `checked` becomes `true`.
    /// No-op if it's already checked.
    pub async fn check(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.check().await
    }

    /// Click the matched checkbox so its `checked` becomes `false`.
    /// No-op if it's already unchecked.
    pub async fn uncheck(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.uncheck().await
    }

    /// Set the matched `<select>` element to the option with the given
    /// `value` attribute and fire `input`/`change` events.
    pub async fn select_option(&self, value: &str) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.select_option(value).await
    }

    /// Clear the element and type `value`.  Equivalent to clearing the field
    /// and calling [`type_text`].
    ///
    /// [`type_text`]: Locator::type_text
    pub async fn fill(&self, value: &str) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.fill(value).await
    }

    /// Clear the element's current value.
    pub async fn clear(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.clear().await
    }

    /// Focus the element and press a single key.
    pub async fn press(&self, key: &str) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.press(key).await
    }

    /// Touch-tap the element.
    pub async fn tap(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.tap().await
    }

    /// Dispatch a synthetic DOM event on the element.
    ///
    /// `event_type` is the event name (e.g. `"click"`, `"input"`). `init` is
    /// an optional JSON `EventInit` dictionary.
    pub async fn dispatch_event(
        &self,
        event_type: &str,
        init: Option<&serde_json::Value>,
    ) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.dispatch_event(event_type, init).await
    }

    /// Evaluate a JS function with the resolved element bound as `this`.
    ///
    /// Returns the JSON result.
    pub async fn evaluate(&self, func: impl Into<String>) -> crate::Result<serde_json::Value> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.evaluate(func).await
    }

    /// Returns the matched element's `innerText`.
    pub async fn inner_text(&self) -> crate::Result<String> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.inner_text().await
    }

    /// Returns the matched element's `textContent` (faster, includes
    /// hidden nodes — see `inner_text` for visible-text-only).
    pub async fn text_content(&self) -> crate::Result<String> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.text_content().await
    }

    /// Returns the matched element's `innerHTML`.
    pub async fn inner_html(&self) -> crate::Result<String> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.inner_html().await
    }

    /// Returns the value of the named HTML attribute, or `None` if absent.
    pub async fn attribute(&self, name: &str) -> crate::Result<Option<String>> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.attribute(name).await
    }

    /// Returns `true` if the element currently exists in the DOM and is
    /// visible.  Does not retry — returns `false` for `NotFound`.
    pub async fn is_visible(&self) -> crate::Result<bool> {
        match self.resolve_one().await {
            Ok(el) => el.is_visible().await,
            Err(CdpError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Move keyboard focus to the matched element.
    pub async fn focus(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.focus().await
    }

    /// Remove focus from the element.
    pub async fn blur(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.blur().await
    }

    /// Double-click the element.
    pub async fn dblclick(&self) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.dblclick().await
    }

    /// Type `text` character-by-character, firing key events for each character.
    ///
    /// Unlike [`fill`](Locator::fill), this dispatches `keydown`/`char`/`keyup`
    /// so that widgets driven by key events (autocomplete, etc.) react correctly.
    pub async fn press_sequentially(&self, text: &str) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.press_sequentially(text).await
    }

    /// Drag this element to `target`.
    ///
    /// Resolves both locators (with retry), then simulates mousedown on this
    /// element, incremental mouse moves, and mouseup on `target`.
    pub async fn drag_to(&self, target: &Locator) -> crate::Result<()> {
        let src_el = self.resolve_with_retry(&Progress::default()).await?;
        let dst_el = target.resolve_with_retry(&Progress::default()).await?;
        src_el.drag_to(&dst_el).await
    }

    /// Returns `true` if the element currently exists in the DOM and is
    /// enabled.  Does not retry — returns `false` for `NotFound`.
    pub async fn is_enabled(&self) -> crate::Result<bool> {
        match self.resolve_one().await {
            Ok(el) => el.is_enabled().await,
            Err(CdpError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Returns `true` if the element is hidden or absent from the DOM.
    /// Does not retry — a missing element counts as hidden.
    pub async fn is_hidden(&self) -> crate::Result<bool> {
        match self.resolve_one().await {
            Ok(el) => el.is_hidden().await,
            Err(CdpError::NotFound) => Ok(true),
            Err(e) => Err(e),
        }
    }

    /// Returns `true` if the element's `checked` property is truthy.
    /// Does not retry — returns `false` for `NotFound`.
    pub async fn is_checked(&self) -> crate::Result<bool> {
        match self.resolve_one().await {
            Ok(el) => el.is_checked().await,
            Err(CdpError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Returns `true` if the element is disabled.
    /// Does not retry — returns `false` for `NotFound`.
    pub async fn is_disabled(&self) -> crate::Result<bool> {
        match self.resolve_one().await {
            Ok(el) => el.is_disabled().await,
            Err(CdpError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Returns `true` if the element can accept text input.
    /// Does not retry — returns `false` for `NotFound`.
    pub async fn is_editable(&self) -> crate::Result<bool> {
        match self.resolve_one().await {
            Ok(el) => el.is_editable().await,
            Err(CdpError::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Returns the current value of an `<input>`, `<textarea>`, or `<select>` element.
    pub async fn input_value(&self) -> crate::Result<String> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.input_value().await
    }

    /// Returns the bounding box of the matched element, or `None` when the
    /// element has no layout box (e.g. `display:none`).
    pub async fn bounding_box(&self) -> crate::Result<Option<crate::layout::BoundingBox>> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.bounding_box().await
    }

    /// Returns the `innerText` of every element matched by this locator.
    pub async fn all_inner_texts(&self) -> crate::Result<Vec<String>> {
        let elements = self.resolve_all().await?;
        let mut out = Vec::with_capacity(elements.len());
        for el in elements {
            out.push(el.inner_text().await?);
        }
        Ok(out)
    }

    /// Returns the `textContent` of every element matched by this locator.
    pub async fn all_text_contents(&self) -> crate::Result<Vec<String>> {
        let elements = self.resolve_all().await?;
        let mut out = Vec::with_capacity(elements.len());
        for el in elements {
            out.push(el.text_content().await?);
        }
        Ok(out)
    }

    /// Returns a locator that finds elements with the given `alt` attribute value.
    pub fn get_by_alt_text(&self, text: impl Into<String>) -> Locator {
        Locator::new_with_js(self.frame.clone(), js_get_by_alt_text(&text.into()))
    }

    /// Returns a locator that finds elements with the given `title` attribute value.
    pub fn get_by_title(&self, text: impl Into<String>) -> Locator {
        Locator::new_with_js(self.frame.clone(), js_get_by_title(&text.into()))
    }

    // ── Advanced methods ──────────────────────────────────────────────────

    /// Screenshot the matched element.
    ///
    /// Scrolls the element into view and captures a PNG of its bounding box.
    pub async fn screenshot(&self) -> crate::Result<Vec<u8>> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        el.screenshot().await
    }

    /// Evaluate a JavaScript function with all matched elements passed as an array argument.
    ///
    /// Unlike [`evaluate`](Self::evaluate), which acts on a single element, this
    /// collects all matches first and passes the full array to `func`.  Useful
    /// for bulk data extraction.
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// let texts: Vec<String> = page
    ///     .locator("li")
    ///     .evaluate_all("els => els.map(e => e.textContent)")
    ///     .await?
    ///     .as_array()
    ///     .cloned()
    ///     .unwrap_or_default()
    ///     .into_iter()
    ///     .filter_map(|v| v.as_str().map(str::to_string))
    ///     .collect();
    /// # Ok(())
    /// # }
    /// ```
    pub async fn evaluate_all(&self, func: impl Into<String>) -> crate::Result<serde_json::Value> {
        let func = func.into();
        let elements_js = self.to_js_expression();
        let expr = format!("(function(){{const els={elements_js};return({func})(els);}})()");
        let resp = self.frame.evaluate(expr).await?;
        Ok(resp.into_inner().result.value.unwrap_or(serde_json::Value::Null))
    }

    /// Set files on the matched `<input type="file">` element.
    ///
    /// `files` is an iterator of absolute file-system paths.  The element
    /// must be visible and accept file selection.
    pub async fn set_input_files(
        &self,
        files: impl IntoIterator<Item = impl Into<String>>,
    ) -> crate::Result<()> {
        let el = self.resolve_with_retry(&Progress::default()).await?;
        let files: Vec<String> = files.into_iter().map(|f| f.into()).collect();
        let mut params = crate::cdp::browser_protocol::dom::SetFileInputFilesParams::new(files);
        params.node_id = Some(el.node_id);
        self.frame.handle.execute(params, Some(self.frame.session_id.current())).await?;
        Ok(())
    }

    /// Return a YAML-like accessibility snapshot of the matched element's subtree.
    ///
    /// Enables the Accessibility CDP domain, calls `Accessibility.getPartialAXTree`
    /// for the matched element, and formats the result as an indented list.
    /// Returns an empty string when the element is not represented in the AX tree.
    pub async fn aria_snapshot(&self) -> crate::Result<String> {
        use crate::cdp::browser_protocol::accessibility as cdp_ax;
        let el = self.resolve_with_retry(&Progress::default()).await?;
        let _ = self
            .frame
            .handle
            .execute(cdp_ax::EnableParams::default(), Some(self.frame.session_id.current()))
            .await;
        let params = cdp_ax::GetPartialAxTreeParams {
            object_id: Some(el.remote_object_id.clone()),
            ..Default::default()
        };
        let resp = self.frame.handle.execute(params, Some(self.frame.session_id.current())).await?;
        match crate::accessibility::build_tree(&resp.nodes) {
            Some(root) => Ok(format_aria_node(&root, 0)),
            None => Ok(String::new()),
        }
    }

    /// Create an [`Expect`] builder for web-first assertions on this locator.
    pub fn expect(self) -> Expect {
        Expect::new(self)
    }
}

// ── ARIA snapshot formatter ───────────────────────────────────────────────────

fn format_aria_node(node: &crate::accessibility::AXNode, depth: usize) -> String {
    if node.ignored {
        return node.children.iter().map(|c| format_aria_node(c, depth)).collect();
    }
    let indent = "  ".repeat(depth);
    let role = node.role.as_deref().unwrap_or("generic");
    let mut line = format!("{indent}- {role}");
    if let Some(name) = &node.name {
        if !name.is_empty() {
            line.push_str(&format!(" {name:?}"));
        }
    }
    if let Some(val) = &node.value {
        if !val.is_empty() {
            line.push_str(&format!(": {val}"));
        }
    }
    line.push('\n');
    for child in &node.children {
        line.push_str(&format_aria_node(child, depth + 1));
    }
    line
}

// ── JS expression builders (pub(crate) so Page and Frame can reuse them) ─────

pub(crate) fn js_get_by_text(text: &str) -> String {
    format!(
        r#"Array.from(document.querySelectorAll('*')).filter(el => el.childNodes.length > 0 && Array.from(el.childNodes).some(n => n.nodeType === 3 && n.textContent.trim() === {text:?}))"#
    )
}

pub(crate) fn js_get_by_role(role: &str, name: Option<&str>) -> String {
    let name_filter = match name {
        Some(n) => format!(
            r#" && (!{n:?} || el.getAttribute('aria-label') === {n:?} || el.textContent.trim() === {n:?})"#
        ),
        None => String::new(),
    };
    format!(
        r#"Array.from(document.querySelectorAll('[role="{role}"]')).filter(el => true{name_filter})"#
    )
}

pub(crate) fn js_get_by_label(label: &str) -> String {
    format!(
        r#"(function(l){{const ids=[...document.querySelectorAll('label')].filter(lb=>lb.textContent.trim()===l).map(lb=>lb.htmlFor).filter(Boolean);return [...document.querySelectorAll('[id]')].filter(el=>ids.includes(el.id));}})({})"#,
        serde_json::Value::String(label.to_string())
    )
}

pub(crate) fn js_get_by_placeholder(text: &str) -> String {
    format!(
        r#"Array.from(document.querySelectorAll('[placeholder]')).filter(el => el.placeholder === {text:?})"#
    )
}

pub(crate) fn js_get_by_test_id(id: &str) -> String {
    format!(r#"Array.from(document.querySelectorAll('[data-testid={id:?}]'))"#)
}

pub(crate) fn js_get_by_alt_text(text: &str) -> String {
    format!(r#"Array.from(document.querySelectorAll('[alt]')).filter(el => el.alt === {text:?})"#)
}

pub(crate) fn js_get_by_title(text: &str) -> String {
    format!(
        r#"Array.from(document.querySelectorAll('[title]')).filter(el => el.title === {text:?})"#
    )
}

/// Web-first assertion builder for a [`Locator`](crate::Locator).
///
/// Each assertion polls until the condition is satisfied or `timeout` elapses
/// (default 5 s).  On timeout, returns [`CdpError::Timeout`](crate::CdpError::Timeout).
///
/// Prefix with `!` (or call `.not()` in method-call chains) to invert any assertion:
///
/// ```no_run
/// # async fn example(page: chromist::Page) -> chromist::Result<()> {
/// page.locator("#submit").expect().to_be_visible().await?;
/// page.locator("input[name=email]").expect().to_have_value("user@example.com").await?;
/// // Negate via `!` (std::ops::Not) — element must NOT be visible:
/// (!page.locator("#spinner").expect()).to_be_visible().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Expect {
    locator: Locator,
    timeout: Duration,
    /// When `true` every assertion's success condition is inverted.
    negated: bool,
    /// When `true` text comparisons are case-insensitive.
    ignore_case: bool,
}

/// Negate an [`Expect`] assertion — `!locator.expect()` / `.not()`.
impl std::ops::Not for Expect {
    type Output = Self;
    fn not(mut self) -> Self {
        self.negated = !self.negated;
        self
    }
}

impl Expect {
    fn new(locator: Locator) -> Self {
        Self { locator, timeout: Duration::from_secs(5), negated: false, ignore_case: false }
    }

    /// Override the assertion timeout (default 5 s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Make text comparisons case-insensitive for `to_have_text` and
    /// `to_contain_text`.
    pub fn ignore_case(mut self) -> Self {
        self.ignore_case = true;
        self
    }

    // ── private deadline helper ───────────────────────────────────────────

    fn deadline(&self) -> std::time::Instant {
        std::time::Instant::now() + self.timeout
    }

    // ── assertions ────────────────────────────────────────────────────────

    /// Assert that the element is visible within the timeout.
    pub async fn to_be_visible(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = self.locator.is_visible().await.unwrap_or(false);
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element is hidden or absent within the timeout.
    ///
    /// Shorthand for `.not().to_be_visible()`.
    pub async fn to_be_hidden(self) -> crate::Result<()> {
        (!self).to_be_visible().await
    }

    /// Assert that the element is enabled within the timeout.
    pub async fn to_be_enabled(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = self.locator.is_enabled().await.unwrap_or(false);
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element's `checked` property is `true`.
    pub async fn to_be_checked(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let checked = match self.locator.resolve_one().await {
                Ok(el) => el
                    .property("checked")
                    .await
                    .ok()
                    .flatten()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                Err(_) => false,
            };
            if checked != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Shorthand for `.not().to_be_checked()`.
    pub async fn not_to_be_checked(self) -> crate::Result<()> {
        (!self).to_be_checked().await
    }

    /// Assert that the element has keyboard focus (`document.activeElement`).
    pub async fn to_be_focused(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let focused = match self.locator.resolve_one().await {
                Ok(el) => el
                    .evaluate("function(){return document.activeElement === this;}")
                    .await
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                Err(_) => false,
            };
            if focused != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the number of matched elements equals `n`.
    pub async fn to_have_count(self, n: usize) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let count = self.locator.count().await.unwrap_or(0);
            let ok = count == n;
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that `innerText` (trimmed) equals `expected`.
    ///
    /// Use [`ignore_case`] for a case-insensitive comparison.
    ///
    /// [`ignore_case`]: Expect::ignore_case
    pub async fn to_have_text(self, expected: &str) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = if let Ok(text) = self.locator.inner_text().await {
                if self.ignore_case {
                    text.trim().to_lowercase() == expected.trim().to_lowercase()
                } else {
                    text.trim() == expected.trim()
                }
            } else {
                false
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that `innerText` contains `substring`.
    ///
    /// Use [`ignore_case`] for a case-insensitive search.
    ///
    /// [`ignore_case`]: Expect::ignore_case
    pub async fn to_contain_text(self, substring: &str) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = if let Ok(text) = self.locator.inner_text().await {
                if self.ignore_case {
                    text.to_lowercase().contains(&substring.to_lowercase())
                } else {
                    text.contains(substring)
                }
            } else {
                false
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element's `value` property equals `expected`.
    pub async fn to_have_value(self, expected: &str) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => el
                    .string_property("value")
                    .await
                    .ok()
                    .flatten()
                    .map(|v| v == expected)
                    .unwrap_or(false),
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element has attribute `name` equal to `value`.
    pub async fn to_have_attribute(self, name: &str, value: &str) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => {
                    el.attribute(name).await.ok().flatten().map(|v| v == value).unwrap_or(false)
                }
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Invert the next assertion (method-chain alternative to `!locator.expect()`).
    pub fn negated(self) -> Self {
        !self
    }

    /// Assert that the element is disabled.
    ///
    /// Shorthand for `.not().to_be_enabled()`.
    pub async fn to_be_disabled(self) -> crate::Result<()> {
        (!self).to_be_enabled().await
    }

    /// Assert that the element is editable (not read-only and not disabled).
    pub async fn to_be_editable(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = self.locator.is_editable().await.unwrap_or(false);
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element is attached to the DOM (can be resolved).
    pub async fn to_be_attached(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = self.locator.resolve_one().await.is_ok();
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element is empty — no inner text for general elements,
    /// empty `value` for inputs/textareas.
    pub async fn to_be_empty(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => {
                    let tag = el
                        .evaluate("function(){return this.tagName.toLowerCase();}")
                        .await
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_default();
                    if matches!(tag.as_str(), "input" | "textarea" | "select") {
                        el.input_value().await.map(|v| v.is_empty()).unwrap_or(false)
                    } else {
                        el.inner_text().await.map(|t| t.trim().is_empty()).unwrap_or(false)
                    }
                }
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element's bounding box intersects the current viewport.
    pub async fn to_be_in_viewport(self) -> crate::Result<()> {
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => el
                    .evaluate(
                        "function(){\
                            var r=this.getBoundingClientRect();\
                            return r.bottom>0&&r.right>0\
                                &&r.top<window.innerHeight\
                                &&r.left<window.innerWidth;\
                        }",
                    )
                    .await
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element has CSS class `class_name` in its `classList`.
    pub async fn to_have_class(self, class_name: &str) -> crate::Result<()> {
        let class_name = class_name.to_owned();
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => el
                    .evaluate(&format!(
                        "function(){{return this.classList.contains({:?});}}",
                        class_name
                    ))
                    .await
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element's `id` attribute equals `expected`.
    pub async fn to_have_id(self, expected: &str) -> crate::Result<()> {
        self.to_have_attribute("id", expected).await
    }

    /// Assert that the computed CSS property `property` equals `value`.
    pub async fn to_have_css(self, property: &str, value: &str) -> crate::Result<()> {
        let property = property.to_owned();
        let value = value.to_owned();
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => el
                    .evaluate(&format!(
                        "function(){{return window.getComputedStyle(this).getPropertyValue({:?});}}",
                        property
                    ))
                    .await
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.trim() == value.trim()))
                    .unwrap_or(false),
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that the element's JavaScript property `name` deep-equals `expected`.
    pub async fn to_have_js_property(
        self,
        name: &str,
        expected: serde_json::Value,
    ) -> crate::Result<()> {
        let name = name.to_owned();
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => {
                    el.property(&name).await.ok().flatten().map(|v| v == expected).unwrap_or(false)
                }
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Assert that a `<select multiple>` element has exactly the given selected values.
    pub async fn to_have_values(self, expected: &[&str]) -> crate::Result<()> {
        let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        let deadline = self.deadline();
        loop {
            let ok = match self.locator.resolve_one().await {
                Ok(el) => {
                    let result = el
                        .evaluate(
                            "function(){\
                                return Array.from(this.selectedOptions)\
                                    .map(function(o){return o.value;});\
                            }",
                        )
                        .await;
                    match result {
                        Ok(v) => {
                            let mut actual: Vec<String> = v
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|x| x.as_str().map(str::to_owned))
                                        .collect()
                                })
                                .unwrap_or_default();
                            actual.sort();
                            let mut exp = expected.clone();
                            exp.sort();
                            actual == exp
                        }
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            };
            if ok != self.negated {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(CdpError::Timeout);
            }
            crate::runtime::sleep(Duration::from_millis(50)).await;
        }
    }
}
