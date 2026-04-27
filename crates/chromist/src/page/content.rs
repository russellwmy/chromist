use super::Page;
use crate::cdp::browser_protocol::page as cdp_page;

impl Page {
    /// Returns the current main-frame URL from the cached frame tree.
    ///
    /// Sync and zero-allocation aside from the `String` clone — matches
    /// [`Frame::url`](crate::frame::Frame::url). The cache is refreshed on
    /// every `Page.frameNavigated` event. For URL changes via `pushState` /
    /// `replaceState` (which don't fire `frameNavigated`), evaluate
    /// `"location.href"` for the live value:
    ///
    /// ```ignore
    /// let live = page.evaluate("location.href").await?.into_value::<String>()?;
    /// ```
    pub fn url(&self) -> Option<String> {
        self.frame_tree.read().ok().and_then(|t| t.main_frame().map(|f| f.url.clone()))
    }

    /// Returns the current page title via `document.title`.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn title(&self) -> crate::Result<Option<String>> {
        let resp = self.evaluate("document.title").await?;
        match resp.into_inner().result.value {
            Some(serde_json::Value::String(s)) if !s.is_empty() => Ok(Some(s)),
            _ => Ok(None),
        }
    }

    /// Returns the full HTML of the current document (including DOCTYPE).
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn content(&self) -> crate::Result<String> {
        let resp = self.evaluate(crate::js::GET_DOCUMENT_CONTENT).await?;
        match resp.into_inner().result.value {
            Some(serde_json::Value::String(s)) => Ok(s),
            Some(v) => Ok(v.to_string()),
            None => Ok(String::new()),
        }
    }

    /// Replace the page's HTML content and wait for the load event.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn set_content(&self, html: impl AsRef<str>) -> crate::Result<()> {
        // Use `Page.setDocumentContent` — the canonical CDP command for setting
        // a frame's HTML — and wait for the `load` lifecycle event so callers
        // don't race subsequent DOM queries against an in-flight parser.
        let waiter = self.navigation_waiter(crate::lifecycle::WaitUntil::Load);
        let frame_id = self.mainframe()?;
        let params = cdp_page::SetDocumentContentParams::new(frame_id, html.as_ref().to_string());
        self.handle.execute(params, Some(self.session_id.current())).await?;
        waiter.wait().await
    }
}
