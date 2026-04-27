//! JavaScript (V8) and CSS coverage collection.
//!
//! Obtain via [`Page::js_coverage`](crate::Page::js_coverage) /
//! [`Page::css_coverage`](crate::Page::css_coverage).
//!
//! Typical flow:
//! ```ignore
//! let cov = page.js_coverage();
//! cov.start().await?;
//! // ... drive the page ...
//! let scripts = cov.stop().await?;
//! ```

use crate::cdp::browser_protocol::css as cdp_css;
use crate::cdp::js_protocol::debugger as cdp_debugger;
use crate::cdp::js_protocol::profiler as cdp_profiler;
use crate::handler::{HandlerHandle, SessionRef};

pub(crate) use cdp_css::RuleUsage;
pub(crate) use cdp_profiler::ScriptCoverage;

/// Collects block-level JavaScript coverage via the `Profiler` domain.
#[derive(Debug, Clone)]
pub struct JsCoverage {
    handle: HandlerHandle,
    session_id: SessionRef,
}

impl JsCoverage {
    /// Construct from the handle + session that the coverage data should
    /// be collected against.
    pub fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id }
    }

    /// Enable `Debugger` + `Profiler` and start precise coverage with
    /// `callCount=false, detailed=true, allowTriggeredUpdates=false`.
    pub async fn start(&self) -> crate::Result<()> {
        let sid = self.session_id.current();
        self.handle.execute(cdp_debugger::EnableParams::default(), Some(sid.clone())).await?;
        self.handle.execute(cdp_profiler::EnableParams::default(), Some(sid.clone())).await?;
        let start = cdp_profiler::StartPreciseCoverageParams {
            call_count: Some(false),
            detailed: Some(true),
            allow_triggered_updates: Some(false),
        };
        self.handle.execute(start, Some(sid)).await?;
        Ok(())
    }

    /// Take the final coverage delta, then stop precise coverage and disable
    /// `Profiler` + `Debugger`.
    pub async fn stop(&self) -> crate::Result<Vec<ScriptCoverage>> {
        let sid = self.session_id.current();
        let take = self
            .handle
            .execute(cdp_profiler::TakePreciseCoverageParams::default(), Some(sid.clone()))
            .await?;
        // Best-effort shutdown; preserve captured data even if disable fails,
        // but surface the failure at debug level for diagnostics.
        if let Err(e) = self
            .handle
            .execute(cdp_profiler::StopPreciseCoverageParams::default(), Some(sid.clone()))
            .await
        {
            tracing::debug!(error = %e, "JsCoverage: StopPreciseCoverage failed");
        }
        if let Err(e) =
            self.handle.execute(cdp_profiler::DisableParams::default(), Some(sid.clone())).await
        {
            tracing::debug!(error = %e, "JsCoverage: Profiler.disable failed");
        }
        if let Err(e) = self.handle.execute(cdp_debugger::DisableParams::default(), Some(sid)).await
        {
            tracing::debug!(error = %e, "JsCoverage: Debugger.disable failed");
        }
        Ok(take.result)
    }
}

/// Collects CSS rule-usage coverage via the `CSS` domain.
#[derive(Debug, Clone)]
pub struct CssCoverage {
    handle: HandlerHandle,
    session_id: SessionRef,
}

impl CssCoverage {
    /// Construct from the handle + session that the coverage data should
    /// be collected against.
    pub fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id }
    }

    /// Enable the `CSS` domain and start rule-usage tracking.
    pub async fn start(&self) -> crate::Result<()> {
        let sid = self.session_id.current();
        self.handle.execute(cdp_css::EnableParams::default(), Some(sid.clone())).await?;
        self.handle.execute(cdp_css::StartRuleUsageTrackingParams::default(), Some(sid)).await?;
        Ok(())
    }

    /// Take the rule-usage delta, then stop tracking and disable `CSS`.
    pub async fn stop(&self) -> crate::Result<Vec<RuleUsage>> {
        let sid = self.session_id.current();
        let take = self
            .handle
            .execute(cdp_css::TakeCoverageDeltaParams::default(), Some(sid.clone()))
            .await?;
        if let Err(e) = self
            .handle
            .execute(cdp_css::StopRuleUsageTrackingParams::default(), Some(sid.clone()))
            .await
        {
            tracing::debug!(error = %e, "CssCoverage: StopRuleUsageTracking failed");
        }
        if let Err(e) = self.handle.execute(cdp_css::DisableParams::default(), Some(sid)).await {
            tracing::debug!(error = %e, "CssCoverage: CSS.disable failed");
        }
        Ok(take.coverage)
    }
}
