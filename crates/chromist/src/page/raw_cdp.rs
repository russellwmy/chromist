//! Low-level escape hatch for raw CDP domain enable/disable toggles.
//!
//! Most users never need this. The high-level [`Page`](super::Page) API
//! enables the domains it needs (Page, Runtime, lifecycle events, etc.) at
//! attach time. Use [`RawCdp`] only when you're issuing raw CDP commands via
//! [`Page::execute`](super::Page::execute) and need a domain that isn't
//! enabled by default.

use super::Page;
use crate::cdp::browser_protocol::css as cdp_css;
use crate::cdp::browser_protocol::dom as cdp_dom;
use crate::cdp::browser_protocol::log as cdp_log;
use crate::cdp::browser_protocol::network as cdp_network;
use crate::cdp::js_protocol::debugger as cdp_debugger;
use crate::cdp::js_protocol::runtime as cdp_runtime;

/// Borrowed handle for raw CDP domain toggles. Obtain via
/// [`Page::raw_cdp`](super::Page::raw_cdp).
#[derive(Debug)]
pub struct RawCdp<'a> {
    pub(super) page: &'a Page,
}

impl<'a> RawCdp<'a> {
    /// Enable the `Log` CDP domain to receive console log entries.
    pub async fn enable_log(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_log::EnableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Disable the `Log` CDP domain.
    pub async fn disable_log(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_log::DisableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Enable the `Runtime` CDP domain.
    pub async fn enable_runtime(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_runtime::EnableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Disable the `Runtime` CDP domain.
    pub async fn disable_runtime(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_runtime::DisableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Enable the `DOM` CDP domain.
    pub async fn enable_dom(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_dom::EnableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Disable the `DOM` CDP domain.
    pub async fn disable_dom(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_dom::DisableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Enable the `CSS` CDP domain.
    pub async fn enable_css(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_css::EnableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Disable the `CSS` CDP domain.
    pub async fn disable_css(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_css::DisableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Enable the `Debugger` CDP domain.
    pub async fn enable_debugger(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_debugger::EnableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Disable the `Debugger` CDP domain.
    pub async fn disable_debugger(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_debugger::DisableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }

    /// Enable the `Network` CDP domain (required for network events).
    pub async fn enable_network(&self) -> crate::Result<()> {
        self.page
            .handle
            .execute(cdp_network::EnableParams::default(), Some(self.page.session_id.current()))
            .await?;
        Ok(())
    }
}
