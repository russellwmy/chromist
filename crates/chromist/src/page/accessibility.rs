use super::Page;
use crate::accessibility::{build_tree, AXNode};
use crate::cdp::browser_protocol::accessibility as cdp_ax;

impl Page {
    /// AI-friendly ARIA snapshot of the entire page.
    ///
    /// Shortcut for `self.locator("body").aria_snapshot()`.
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// let snapshot = page.aria_snapshot().await?;
    /// println!("{snapshot}");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn aria_snapshot(&self) -> crate::Result<String> {
        self.locator("body").aria_snapshot().await
    }

    /// Capture a snapshot of the page's accessibility tree.
    ///
    /// Returns the root [`AXNode`], or `None` when the tree is empty.
    /// Nodes that are ignored by assistive technology are excluded from the
    /// returned tree (but not from intermediate parent/child resolution).
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// if let Some(root) = page.accessibility_snapshot().await? {
    ///     println!("root role: {:?}", root.role);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn accessibility_snapshot(&self) -> crate::Result<Option<AXNode>> {
        let _ = self
            .handle
            .execute(cdp_ax::EnableParams::default(), Some(self.session_id.current()))
            .await;

        let resp = self
            .handle
            .execute(cdp_ax::GetFullAxTreeParams::default(), Some(self.session_id.current()))
            .await?;

        if resp.nodes.is_empty() {
            return Ok(None);
        }

        Ok(build_tree(&resp.nodes))
    }
}
