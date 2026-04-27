use crate::cdp::browser_protocol::page as cdp_page;

/// Image encoding for a page screenshot.
///
/// # Examples
///
/// ```
/// use chromist::ScreenshotFormat;
/// assert_eq!(ScreenshotFormat::default(), ScreenshotFormat::Png);
/// assert_eq!(ScreenshotFormat::Jpeg.to_string(), "jpeg");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ScreenshotFormat {
    #[default]
    /// Lossless PNG (default).
    Png,
    /// Lossy JPEG — use [`ScreenshotParamsBuilder::quality`] to tune.
    Jpeg,
    /// WebP — supports both lossy and lossless encoding via `quality`.
    Webp,
}

impl std::fmt::Display for ScreenshotFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScreenshotFormat::Png => f.write_str("png"),
            ScreenshotFormat::Jpeg => f.write_str("jpeg"),
            ScreenshotFormat::Webp => f.write_str("webp"),
        }
    }
}

/// Parameters for [`Page::screenshot_with`](crate::Page::screenshot_with).
///
/// Construct via [`ScreenshotParams::builder`] (recommended) or
/// [`ScreenshotParams::default`].
#[derive(Debug, Clone)]
pub struct ScreenshotParams {
    /// Image encoding (PNG, JPEG, or WebP). Default PNG.
    pub format: ScreenshotFormat,
    /// JPEG / WebP quality 0–100. Ignored for PNG.
    pub quality: Option<i64>,
    /// When `true`, capture the full scrollable height of the page; chromist
    /// also enables [`Self::capture_beyond_viewport`].
    pub full_page: bool,
    /// Render with a transparent background by suppressing the body's
    /// default white. Useful for compositing overlays.
    pub omit_background: bool,
    /// Crop the screenshot to a specific viewport rectangle. `None` captures
    /// the full visible area (or full page when [`Self::full_page`]).
    pub clip: Option<cdp_page::Viewport>,
    /// Capture from the surface rather than the view. Defaults to `true`.
    pub from_surface: bool,
    /// Capture content outside the viewport. Defaults to `false`.
    ///
    /// Note: when `full_page` is set, chromist internally enables this too.
    pub capture_beyond_viewport: bool,
}

impl Default for ScreenshotParams {
    fn default() -> Self {
        Self {
            format: ScreenshotFormat::default(),
            quality: None,
            full_page: false,
            omit_background: false,
            clip: None,
            from_surface: true,
            capture_beyond_viewport: false,
        }
    }
}

impl ScreenshotParams {
    /// Returns a fresh builder seeded with [`ScreenshotParams::default`].
    #[must_use]
    pub fn builder() -> ScreenshotParamsBuilder {
        ScreenshotParamsBuilder::default()
    }
}

/// Fluent builder for [`ScreenshotParams`].
#[derive(Debug, Default)]
pub struct ScreenshotParamsBuilder {
    inner: ScreenshotParams,
}

impl ScreenshotParamsBuilder {
    /// Choose the image encoding (PNG, JPEG, or WebP).
    pub fn format(mut self, f: ScreenshotFormat) -> Self {
        self.inner.format = f;
        self
    }

    /// JPEG/WebP quality 0–100. Higher = larger file, less compression.
    /// Ignored when [`Self::format`] is PNG.
    pub fn quality(mut self, q: i64) -> Self {
        self.inner.quality = Some(q);
        self
    }

    /// Capture the full scrollable height of the page. Implies
    /// `capture_beyond_viewport(true)`.
    pub fn full_page(mut self) -> Self {
        self.inner.full_page = true;
        self
    }

    /// Render with a transparent background by suppressing the document's
    /// default white. Useful when compositing the screenshot over other
    /// imagery.
    pub fn omit_background(mut self) -> Self {
        self.inner.omit_background = true;
        self
    }

    /// Crop to a specific clip rectangle within the page coordinate space.
    pub fn clip(mut self, clip: cdp_page::Viewport) -> Self {
        self.inner.clip = Some(clip);
        self
    }

    /// Capture from the GPU surface (`true`, default) or from the rendered
    /// view (`false`). Surface capture is faster and matches what the user
    /// sees.
    pub fn from_surface(mut self, v: bool) -> Self {
        self.inner.from_surface = v;
        self
    }

    /// Allow content outside the visible viewport to be captured. Required
    /// for `full_page` (chromist sets it automatically in that case).
    pub fn capture_beyond_viewport(mut self, v: bool) -> Self {
        self.inner.capture_beyond_viewport = v;
        self
    }

    #[must_use]
    /// Consume the builder and return the populated [`ScreenshotParams`].
    pub fn build(self) -> ScreenshotParams {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_display() {
        assert_eq!(ScreenshotFormat::Png.to_string(), "png");
        assert_eq!(ScreenshotFormat::Jpeg.to_string(), "jpeg");
        assert_eq!(ScreenshotFormat::Webp.to_string(), "webp");
    }

    #[test]
    fn default_params() {
        let p = ScreenshotParams::default();
        assert_eq!(p.format, ScreenshotFormat::Png);
        assert!(p.quality.is_none());
        assert!(!p.full_page);
        assert!(!p.omit_background);
        assert!(p.clip.is_none());
        assert!(p.from_surface);
        assert!(!p.capture_beyond_viewport);
    }

    #[test]
    fn builder_sets_fields() {
        let clip = cdp_page::Viewport { x: 0.0, y: 0.0, width: 100.0, height: 50.0, scale: 1.0 };
        let p = ScreenshotParams::builder()
            .format(ScreenshotFormat::Jpeg)
            .quality(80)
            .full_page()
            .omit_background()
            .clip(clip.clone())
            .from_surface(false)
            .capture_beyond_viewport(true)
            .build();
        assert_eq!(p.format, ScreenshotFormat::Jpeg);
        assert_eq!(p.quality, Some(80));
        assert!(p.full_page);
        assert!(p.omit_background);
        assert!(p.clip.is_some());
        assert!(!p.from_surface);
        assert!(p.capture_beyond_viewport);
    }
}
