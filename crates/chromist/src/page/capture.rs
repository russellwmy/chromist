use super::Page;
use crate::cdp::browser_protocol::emulation as cdp_emulation;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::cdp::browser_protocol::performance as cdp_perf;
use crate::error::CdpError;

impl Page {
    /// Take a PNG screenshot of the visible viewport and return the raw bytes.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn screenshot(&self) -> crate::Result<Vec<u8>> {
        let params = cdp_page::CaptureScreenshotParams::default();
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(resp.data.into_bytes())
    }

    /// Take a screenshot with detailed options.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn screenshot_with(
        &self,
        params: crate::screenshot::ScreenshotParams,
    ) -> crate::Result<Vec<u8>> {
        use crate::screenshot::ScreenshotFormat;
        let format = match params.format {
            ScreenshotFormat::Jpeg => Some(cdp_page::CaptureScreenshotParamsFormat::Jpeg),
            ScreenshotFormat::Webp => Some(cdp_page::CaptureScreenshotParamsFormat::Webp),
            ScreenshotFormat::Png => Some(cdp_page::CaptureScreenshotParamsFormat::Png),
        };
        // full_page implies capturing beyond the viewport, otherwise honour the flag.
        let cdp_params = cdp_page::CaptureScreenshotParams {
            format,
            quality: params.quality,
            clip: params.clip,
            capture_beyond_viewport: Some(params.full_page || params.capture_beyond_viewport),
            from_surface: Some(params.from_surface),
            ..Default::default()
        };
        if params.omit_background {
            self.handle
                .execute(
                    cdp_emulation::SetDefaultBackgroundColorOverrideParams { color: None },
                    Some(self.session_id.current()),
                )
                .await?;
        }
        let resp = self.handle.execute(cdp_params, Some(self.session_id.current())).await?;
        if params.omit_background {
            self.handle
                .execute(
                    cdp_emulation::SetDefaultBackgroundColorOverrideParams {
                        color: Some(crate::cdp::browser_protocol::dom::Rgba {
                            r: 255,
                            g: 255,
                            b: 255,
                            a: Some(1.0),
                        }),
                    },
                    Some(self.session_id.current()),
                )
                .await?;
        }
        Ok(resp.data.into_bytes())
    }

    /// Save a screenshot to a file and return the bytes.
    pub async fn save_screenshot(
        &self,
        params: crate::screenshot::ScreenshotParams,
        path: impl AsRef<std::path::Path>,
    ) -> crate::Result<Vec<u8>> {
        let data = self.screenshot_with(params).await?;
        tokio::fs::write(path, &data).await.map_err(CdpError::Io)?;
        Ok(data)
    }

    /// Render this page to PDF and return the raw bytes.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn pdf(&self) -> crate::Result<Vec<u8>> {
        self.pdf_with(cdp_page::PrintToPdfParams::default()).await
    }

    /// Generate a PDF with custom parameters.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn pdf_with(&self, params: cdp_page::PrintToPdfParams) -> crate::Result<Vec<u8>> {
        let resp = self.handle.execute(params, Some(self.session_id.current())).await?;
        Ok(resp.data.into_bytes())
    }

    /// Save a PDF to a file and return the bytes.
    pub async fn save_pdf(&self, path: impl AsRef<std::path::Path>) -> crate::Result<Vec<u8>> {
        let data = self.pdf().await?;
        tokio::fs::write(path, &data).await.map_err(CdpError::Io)?;
        Ok(data)
    }

    /// Return browser performance metrics for this page as a name → value map.
    pub async fn metrics(&self) -> crate::Result<crate::PerfMetrics> {
        let resp = self
            .handle
            .execute(cdp_perf::GetMetricsParams::default(), Some(self.session_id.current()))
            .await?;
        Ok(crate::PerfMetrics::from(resp.metrics))
    }

    /// Returns layout metrics for the current page.
    pub async fn layout_metrics(&self) -> crate::Result<crate::LayoutMetrics> {
        let resp = self
            .handle
            .execute(cdp_page::GetLayoutMetricsParams::default(), Some(self.session_id.current()))
            .await?;
        Ok(resp.into())
    }
}
