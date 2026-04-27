use super::Page;
use crate::cdp::browser_protocol::emulation as cdp_emulation;
use crate::device::DeviceDescriptor;

impl Page {
    /// Apply device emulation — sets viewport, user agent, mobile flag, and touch support.
    ///
    /// ```no_run
    /// # async fn example(page: chromist::Page) -> chromist::Result<()> {
    /// use chromist::DeviceDescriptor;
    /// page.emulate_device(&DeviceDescriptor::iphone_13()).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn emulate_device(&self, device: &DeviceDescriptor) -> crate::Result<()> {
        self.emulation().set_viewport(device.viewport).await?;

        let ua = cdp_emulation::SetUserAgentOverrideParams::new(device.user_agent.clone());
        self.handle.execute(ua, Some(self.session_id.current())).await?;

        if device.has_touch {
            let tp = cdp_emulation::SetTouchEmulationEnabledParams {
                enabled: true,
                max_touch_points: Some(5),
            };
            if let Err(e) = self.handle.execute(tp, Some(self.session_id.current())).await {
                tracing::debug!(error = %e, "emulate_device: SetTouchEmulationEnabled failed");
            }
        }

        Ok(())
    }
}
