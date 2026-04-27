//! Device descriptors for mobile/tablet emulation.

use crate::layout::Viewport;

/// Describes a device's screen and user-agent for emulation via
/// [`Page::emulate_device`](crate::Page::emulate_device).
#[derive(Debug, Clone)]
pub struct DeviceDescriptor {
    /// Screen dimensions and pixel ratio.
    pub viewport: Viewport,
    /// `User-Agent` string to send with requests.
    pub user_agent: String,
    /// CSS device pixel ratio (1.0 = desktop standard).
    pub device_scale_factor: f64,
    /// Whether to emulate a mobile browser.
    pub is_mobile: bool,
    /// Whether to emulate touch input.
    pub has_touch: bool,
}

impl DeviceDescriptor {
    /// iPhone 13 (390×844, 3× DPR).
    pub fn iphone_13() -> Self {
        Self {
            viewport: Viewport {
                width: 390,
                height: 844,
                device_scale_factor: 3.0,
                emulating_mobile: true,
                has_touch: true,
                is_landscape: false,
            },
            user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 15_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.0 Mobile/15E148 Safari/604.1".into(),
            device_scale_factor: 3.0,
            is_mobile: true,
            has_touch: true,
        }
    }

    /// iPhone 15 (393×852, 3× DPR).
    pub fn iphone_15() -> Self {
        Self {
            viewport: Viewport {
                width: 393,
                height: 852,
                device_scale_factor: 3.0,
                emulating_mobile: true,
                has_touch: true,
                is_landscape: false,
            },
            user_agent: "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1".into(),
            device_scale_factor: 3.0,
            is_mobile: true,
            has_touch: true,
        }
    }

    /// Google Pixel 7 (412×915, 2.625× DPR).
    pub fn pixel_7() -> Self {
        Self {
            viewport: Viewport {
                width: 412,
                height: 915,
                device_scale_factor: 2.625,
                emulating_mobile: true,
                has_touch: true,
                is_landscape: false,
            },
            user_agent: "Mozilla/5.0 (Linux; Android 13; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36".into(),
            device_scale_factor: 2.625,
            is_mobile: true,
            has_touch: true,
        }
    }

    /// iPad Pro 12.9″ in landscape (1366×1024, 2× DPR).
    pub fn ipad_pro() -> Self {
        Self {
            viewport: Viewport {
                width: 1366,
                height: 1024,
                device_scale_factor: 2.0,
                emulating_mobile: true,
                has_touch: true,
                is_landscape: true,
            },
            user_agent: "Mozilla/5.0 (iPad; CPU OS 16_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.0 Mobile/15E148 Safari/604.1".into(),
            device_scale_factor: 2.0,
            is_mobile: true,
            has_touch: true,
        }
    }

    /// Samsung Galaxy S23 (360×780, 3× DPR).
    pub fn galaxy_s23() -> Self {
        Self {
            viewport: Viewport {
                width: 360,
                height: 780,
                device_scale_factor: 3.0,
                emulating_mobile: true,
                has_touch: true,
                is_landscape: false,
            },
            user_agent: "Mozilla/5.0 (Linux; Android 13; SM-S911B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/116.0.0.0 Mobile Safari/537.36".into(),
            device_scale_factor: 3.0,
            is_mobile: true,
            has_touch: true,
        }
    }
}
