//! [`Emulation`](crate::Emulation) sub-handle: viewport, user-agent, media, timezone, locale,
//! geolocation, network conditions, CSP/JS toggles.
//!
//! Obtained via [`Page::emulation`]. Cheaply cloneable — holds only a
//! [`HandlerHandle`](crate::HandlerHandle) and the page's [`SessionRef`].
//!
//! Methods that cross page-level state (e.g. stealth-mode init-script
//! injection, download-behaviour with browser-context awareness) remain on
//! [`Page`] itself.

use std::sync::Arc;

use super::Page;
use crate::cdp::browser_protocol::browser as cdp_browser;
use crate::cdp::browser_protocol::emulation as cdp_emulation;
use crate::cdp::browser_protocol::network as cdp_network;
use crate::cdp::browser_protocol::page as cdp_page;
use crate::error::CdpError;
use crate::handler::{HandlerHandle, SessionRef};
use crate::layout::Viewport;

/// Sub-handle for emulation primitives on a [`Page`]. Obtain via
/// [`Page::emulation`].
#[derive(Debug, Clone)]
pub struct Emulation {
    handle: HandlerHandle,
    session_id: SessionRef,
}

impl Emulation {
    pub(in crate::page) fn new(handle: HandlerHandle, session_id: SessionRef) -> Self {
        Self { handle, session_id }
    }

    fn session(&self) -> Option<Arc<str>> {
        Some(self.session_id.current())
    }

    /// Override the `User-Agent` header sent with requests from this page.
    #[tracing::instrument(skip_all, level = "debug")]
    pub async fn set_user_agent(
        &self,
        params: impl Into<cdp_emulation::SetUserAgentOverrideParams>,
    ) -> crate::Result<()> {
        self.handle.execute(params.into(), self.session()).await?;
        Ok(())
    }

    /// Set the viewport size and device-scale factor for this page.
    #[tracing::instrument(skip(self), level = "debug")]
    pub async fn set_viewport(&self, vp: Viewport) -> crate::Result<()> {
        let params = cdp_emulation::SetDeviceMetricsOverrideParams {
            width: vp.width as i64,
            height: vp.height as i64,
            device_scale_factor: vp.device_scale_factor,
            mobile: vp.emulating_mobile,
            scale: None,
            screen_width: None,
            screen_height: None,
            position_x: None,
            position_y: None,
            dont_set_visible_size: None,
            screen_orientation: None,
            viewport: None,
            display_feature: None,
            device_posture: None,
            scrollbar_type: None,
            screen_orientation_lock_emulation: None,
        };
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Set the emulated CSS media type (e.g. `"screen"` or `"print"`).
    pub async fn emulate_media_type(&self, media: impl Into<String>) -> crate::Result<()> {
        let params =
            cdp_emulation::SetEmulatedMediaParams { media: Some(media.into()), features: None };
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Emulate CSS media features (e.g. `prefers-color-scheme: dark`).
    pub async fn emulate_media_features(
        &self,
        features: Vec<cdp_emulation::MediaFeature>,
    ) -> crate::Result<()> {
        let params =
            cdp_emulation::SetEmulatedMediaParams { media: None, features: Some(features) };
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Override the browser's timezone for this page (e.g. `"America/New_York"`).
    pub async fn emulate_timezone(&self, tz: impl Into<String>) -> crate::Result<()> {
        self.handle
            .execute(cdp_emulation::SetTimezoneOverrideParams::new(tz.into()), self.session())
            .await?;
        Ok(())
    }

    /// Override the browser's locale for this page (e.g. `"en-US"`).
    pub async fn emulate_locale(&self, locale: impl Into<String>) -> crate::Result<()> {
        let params = cdp_emulation::SetLocaleOverrideParams { locale: Some(locale.into()) };
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Override the browser's geolocation for this page.
    pub async fn set_geolocation(&self, lat: f64, lon: f64, accuracy: f64) -> crate::Result<()> {
        let params = cdp_emulation::SetGeolocationOverrideParams {
            latitude: Some(lat),
            longitude: Some(lon),
            accuracy: Some(accuracy),
            altitude: None,
            altitude_accuracy: None,
            heading: None,
            speed: None,
        };
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Returns the user agent string being used by this page.
    pub async fn user_agent(&self) -> crate::Result<String> {
        let resp = self.handle.execute(cdp_browser::GetVersionParams::default(), None).await?;
        Ok(resp.user_agent)
    }

    /// Toggle network-offline emulation.
    #[allow(deprecated)]
    pub async fn set_offline(&self, offline: bool) -> crate::Result<()> {
        let params = cdp_network::EmulateNetworkConditionsParams::new(offline, 0.0, -1.0, -1.0);
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Enable or disable cache for this page.
    pub async fn set_cache_enabled(&self, enabled: bool) -> crate::Result<()> {
        self.handle
            .execute(cdp_network::SetCacheDisabledParams::new(!enabled), self.session())
            .await?;
        Ok(())
    }

    /// Set extra HTTP headers for all requests from this page.
    pub async fn set_extra_headers(
        &self,
        headers: std::collections::HashMap<String, String>,
    ) -> crate::Result<()> {
        let value = serde_json::to_value(&headers)?;
        let params = cdp_network::SetExtraHttpHeadersParams::new(cdp_network::Headers::new(value));
        self.handle.execute(params, self.session()).await?;
        Ok(())
    }

    /// Ignore HTTPS errors for this page.
    pub async fn set_ignore_certificate_errors(&self, ignore: bool) -> crate::Result<()> {
        use crate::cdp::browser_protocol::security as cdp_security;
        self.handle
            .execute(cdp_security::SetIgnoreCertificateErrorsParams::new(ignore), self.session())
            .await?;
        Ok(())
    }

    /// Bypass Content-Security-Policy restrictions on this page.
    ///
    /// When `true`, inline scripts, eval, and other CSP-blocked sources are
    /// allowed.  Useful for injecting scripts into pages with strict CSP.
    pub async fn set_bypass_csp(&self, enabled: bool) -> crate::Result<()> {
        self.handle.execute(cdp_page::SetBypassCspParams::new(enabled), self.session()).await?;
        Ok(())
    }

    /// Enable or disable JavaScript execution on this page.
    ///
    /// `false` disables all JS (equivalent to
    /// `Emulation.setScriptExecutionDisabled(value: true)`).
    pub async fn set_javascript_enabled(&self, enabled: bool) -> crate::Result<()> {
        self.handle
            .execute(cdp_emulation::SetScriptExecutionDisabledParams::new(!enabled), self.session())
            .await?;
        Ok(())
    }
}

impl Page {
    /// Returns the [`Emulation`](crate::Emulation) sub-handle for viewport, user-agent, media,
    /// timezone, locale, geolocation, network conditions, CSP/JS toggles.
    ///
    /// The returned handle is cheap to clone and shares the page's session.
    pub fn emulation(&self) -> Emulation {
        Emulation::new(self.handle.clone(), self.session_id.clone())
    }

    /// Inject stealth init scripts to hide common browser-automation signals.
    ///
    /// The default ([`StealthOptions::default`](crate::detection::StealthOptions::default))
    /// installs a minimal four-script set; opt into the full set or attach a
    /// custom user agent via the builder methods on
    /// [`StealthOptions`](crate::detection::StealthOptions).
    pub async fn enable_stealth_mode(
        &self,
        options: crate::detection::StealthOptions,
    ) -> crate::Result<()> {
        if let Some(ua) = options.user_agent {
            self.emulation().set_user_agent(ua).await?;
        }
        if options.full {
            self.add_init_script(crate::js::STEALTH_HIDE_WEBDRIVER).await?;
            self.add_init_script(crate::js::STEALTH_CHROME_RUNTIME).await?;
            self.add_init_script(crate::js::STEALTH_PERMISSIONS).await?;
            self.add_init_script(crate::js::STEALTH_WEBGL_VENDOR).await?;
            self.add_init_script(crate::js::STEALTH_PLUGINS).await?;
        } else {
            self.add_init_script(
                r#"
Object.defineProperty(Object.getPrototypeOf(navigator), 'webdriver', { get: () => false });
window.chrome = { runtime: {} };
Object.defineProperty(navigator, 'languages', { get: () => ['en-US', 'en'] });
Object.defineProperty(navigator, 'plugins', { get: () => [1, 2, 3, 4, 5] });
"#,
            )
            .await?;
        }
        Ok(())
    }

    /// Configure where downloads from this page are saved.
    ///
    /// Sets `Browser.setDownloadBehavior` with `behavior=allow` and the given
    /// `download_path` for the browser context that owns this page.
    pub async fn set_download_behavior(
        &self,
        download_path: &std::path::Path,
    ) -> crate::Result<()> {
        let path_str = download_path
            .to_str()
            .ok_or_else(|| CdpError::InvalidPath(download_path.to_path_buf()))?
            .to_string();
        let context_id = self.browser_context.as_ref().and_then(|ctx| ctx.id.clone());
        let mut params = cdp_browser::SetDownloadBehaviorParams::new(
            cdp_browser::SetDownloadBehaviorParamsBehavior::Allow,
        );
        params.browser_context_id = context_id;
        params.download_path = Some(path_str);
        params.events_enabled = Some(true);
        self.handle.execute(params, None).await?;
        Ok(())
    }
}
