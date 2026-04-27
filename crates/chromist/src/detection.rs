use std::path::PathBuf;

/// Options controlling [`Page::enable_stealth_mode`](crate::page::Page::enable_stealth_mode).
///
/// Default: minimal four-script set, no user-agent override.
/// Use `StealthOptions::new().full(true)` for the full set, or
/// `.user_agent(ua)` to also override the UA in the same call.
#[derive(Debug, Clone, Default)]
pub struct StealthOptions {
    pub(crate) full: bool,
    pub(crate) user_agent: Option<String>,
}

impl StealthOptions {
    /// Construct a new options struct with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the full five-script stealth set instead of the minimal default.
    pub fn full(mut self, full: bool) -> Self {
        self.full = full;
        self
    }

    /// Override the user-agent string before installing stealth scripts.
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }
}

/// Knobs controlling Chrome / Chromium executable auto-detection.
#[derive(Debug, Clone)]
pub struct DetectionOptions {
    /// Consider Microsoft Edge installations as fallback (`true` by
    /// default — Edge is Chromium-based and supports CDP).
    pub msedge: bool,
    /// Consider Canary / Dev / Beta channels in addition to Stable.
    pub unstable: bool,
}

impl Default for DetectionOptions {
    fn default() -> Self {
        DetectionOptions { msedge: true, unstable: false }
    }
}

pub(crate) fn detect_executable_with_options(opts: &DetectionOptions) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME") {
        return Some(PathBuf::from(p));
    }
    if let Ok(p) = std::env::var("CHROMIUM") {
        return Some(PathBuf::from(p));
    }
    #[cfg(target_os = "macos")]
    {
        let mut candidates = vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta",
        ];
        if opts.unstable {
            candidates
                .push("/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary");
        }
        if opts.msedge {
            candidates.push("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge");
        }
        for c in candidates {
            let p = PathBuf::from(c);
            if p.exists() {
                return Some(p);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let mut names = vec!["google-chrome", "chromium", "chromium-browser", "chrome"];
        if opts.msedge {
            names.extend_from_slice(&["microsoft-edge", "microsoft-edge-stable", "msedge"]);
        }
        for name in names {
            if let Some(p) = which(name) {
                return Some(p);
            }
        }
        let mut candidates =
            vec!["/usr/bin/google-chrome", "/usr/bin/chromium", "/snap/bin/chromium"];
        if opts.msedge {
            candidates
                .extend_from_slice(&["/usr/bin/microsoft-edge", "/usr/bin/microsoft-edge-stable"]);
        }
        for c in candidates {
            let p = PathBuf::from(c);
            if p.exists() {
                return Some(p);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        let mut candidates = vec![
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files\Chromium\Application\chrome.exe",
        ];
        if opts.msedge {
            candidates.extend_from_slice(&[
                r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
                r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            ]);
        }
        for c in candidates {
            let p = PathBuf::from(c);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
