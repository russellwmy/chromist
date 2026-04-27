//! Cross-frame element location via `FrameLocator`.

use crate::frame::Frame;
use crate::locator::Locator;

/// A handle that scopes element queries to an `<iframe>`'s content document.
///
/// Created by [`Page::frame_locator`](crate::Page::frame_locator) or [`Locator::frame_locator`](crate::Locator::frame_locator).  Each
/// call to `locator` returns a [`Locator`](crate::Locator) that resolves elements inside the
/// iframe matched by `frame_selector`.
///
/// **Note:** the iframe must be *same-origin* with the page.  Cross-origin
/// iframes block `contentDocument` access and will produce a `NotFound` error.
///
/// ```no_run
/// # async fn example(page: chromist::Page) -> chromist::Result<()> {
/// let fl = page.frame_locator("iframe#preview");
/// fl.locator("h1").text_content().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct FrameLocator {
    frame: Frame,
    frame_selector: String,
}

impl FrameLocator {
    pub(crate) fn new(frame: Frame, frame_selector: impl Into<String>) -> Self {
        Self { frame, frame_selector: frame_selector.into() }
    }

    /// Create a [`Locator`](crate::Locator) that resolves `selector` inside the iframe's
    /// content document.
    pub fn locator(&self, selector: impl Into<String>) -> Locator {
        let selector = selector.into();
        let js = format!(
            r#"(function() {{
  var iframe = document.querySelector({frame:?});
  if (!iframe || !iframe.contentDocument) return [];
  return Array.from(iframe.contentDocument.querySelectorAll({sel:?}));
}})()"#,
            frame = self.frame_selector,
            sel = selector,
        );
        Locator::new_with_js(self.frame.clone(), js)
    }

    /// Nest another frame locator inside this one.
    ///
    /// Useful when an iframe contains another iframe.
    pub fn frame_locator(&self, nested_selector: impl Into<String>) -> FrameLocator {
        let outer = self.frame_selector.clone();
        let inner = nested_selector.into();
        let combined = format!("__nested__:{outer}::{inner}");
        FrameLocator { frame: self.frame.clone(), frame_selector: combined }
    }
}
