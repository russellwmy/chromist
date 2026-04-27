pub(crate) fn default_args() -> Vec<&'static str> {
    vec![
        "--disable-background-networking",
        "--enable-features=NetworkService,NetworkServiceInProcess",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-breakpad",
        "--disable-client-side-phishing-detection",
        "--disable-component-extensions-with-background-pages",
        "--disable-default-apps",
        "--disable-dev-shm-usage",
        "--disable-extensions",
        "--disable-features=TranslateUI,BlinkGenPropertyTrees",
        "--disable-hang-monitor",
        "--disable-ipc-flooding-protection",
        "--disable-popup-blocking",
        "--disable-prompt-on-repost",
        "--disable-renderer-backgrounding",
        "--disable-sync",
        "--force-color-profile=srgb",
        "--metrics-recording-only",
        "--no-first-run",
        "--enable-automation",
        "--password-store=basic",
        "--use-mock-keychain",
    ]
}

pub(crate) fn headless_args() -> Vec<&'static str> {
    vec!["--headless=new", "--hide-scrollbars", "--mute-audio"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_args_contain_required_flags() {
        let args = default_args();
        assert!(args.contains(&"--no-first-run"));
        assert!(args.contains(&"--enable-automation"));
        assert!(args.contains(&"--password-store=basic"));
    }

    #[test]
    fn default_args_has_no_duplicates() {
        let args = default_args();
        let mut seen = std::collections::HashSet::new();
        for a in &args {
            assert!(seen.insert(*a), "duplicate default arg: {a}");
        }
    }

    #[test]
    fn headless_args_shape() {
        let args = headless_args();
        assert!(args.contains(&"--headless=new"));
        assert!(args.contains(&"--hide-scrollbars"));
        assert!(args.contains(&"--mute-audio"));
    }

    #[test]
    fn all_args_start_with_double_dash() {
        for a in default_args().iter().chain(headless_args().iter()) {
            assert!(a.starts_with("--"), "arg must start with --: {a}");
        }
    }
}
