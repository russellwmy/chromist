//! Developer task runner. Run via `cargo xtask <task>`.
//!
//! # Tasks
//!
//! * `codegen` — regenerate `crates/chromist-cdp/src/cdp.rs` from the vendored PDL files.
//! * `soak [iterations]` — open and close many pages back-to-back to detect
//!   resource leaks. Defaults to 200 iterations. Requires a real Chromium on
//!   PATH; the chromist auto-detection finds it.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let task = args.next().unwrap_or_default();
    match task.as_str() {
        "codegen" => codegen(),
        "soak" => {
            let iterations: usize = args
                .next()
                .as_deref()
                .map(|s| s.parse().expect("soak iteration count must be a number"))
                .unwrap_or(200);
            soak::run(iterations);
        }
        other => {
            eprintln!("unknown task: {other:?}");
            eprintln!("available tasks: codegen, soak [iterations]");
            std::process::exit(1);
        }
    }
}

fn codegen() {
    let workspace = workspace_root();
    let browser_pdl = workspace.join("crates/chromist-cdp/pdl/browser_protocol.pdl");
    let js_pdl = workspace.join("crates/chromist-cdp/pdl/js_protocol.pdl");
    let out = workspace.join("crates/chromist-cdp/src/cdp.rs");

    let code = chromist_pdl::build::Generator::default()
        .compile_pdls(&[browser_pdl, js_pdl])
        .expect("code generation failed");

    std::fs::write(&out, &code).expect("failed to write cdp.rs");
    println!("wrote {}", out.display());
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is the xtask crate; workspace root is one level up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().expect("xtask has no parent").to_owned()
}

mod soak {
    //! Resource-leak soak test.
    //!
    //! Launches a single browser, opens `N` pages back-to-back (each navigates
    //! to `about:blank`, evaluates a trivial expression, then is dropped),
    //! samples resident set size and open file descriptor counts at the start,
    //! mid-run, and end, and fails if growth exceeds a threshold.
    //!
    //! The threshold is intentionally generous: we're catching unbounded
    //! growth (real leaks), not measuring efficiency. A 50 MiB / 200-fd
    //! ceiling between baseline and final sample is plenty of room for
    //! Chromium's normal warmup while still catching a per-page-cloned `Arc`
    //! that's never released.
    //!
    //! Sampling cadence: `0`, `N/4`, `N/2`, `3N/4`, `N`. Each sample is
    //! reported; the fail condition checks (final − baseline) only.

    use chromist::Browser;
    use std::time::Instant;

    const RSS_GROWTH_LIMIT_BYTES: u64 = 50 * 1024 * 1024;
    const FD_GROWTH_LIMIT: i64 = 200;

    pub fn run(iterations: usize) {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(run_async(iterations));
    }

    async fn run_async(iterations: usize) {
        println!("soak: launching browser");
        let browser = Browser::launch_default().await.expect("launch chromium");
        let started = Instant::now();

        let baseline = Sample::take("baseline");
        baseline.print();

        let checkpoints = sample_points(iterations);
        let mut samples = Vec::with_capacity(checkpoints.len() + 2);
        samples.push(baseline.clone());

        for i in 0..iterations {
            let page = browser.new_page("about:blank").await.expect("new_page");
            let _ = page.evaluate("1 + 1").await.expect("evaluate");
            // Page dropped here — listener tasks should abort.
            drop(page);

            if checkpoints.contains(&i) {
                let s = Sample::take(&format!("after {} pages", i + 1));
                s.print();
                samples.push(s);
            }
        }

        let final_sample = Sample::take("final");
        final_sample.print();
        samples.push(final_sample.clone());

        let elapsed = started.elapsed();
        println!(
            "\nsoak: {iterations} pages in {elapsed:.2?} ({:.1} ms/page)",
            elapsed.as_secs_f64() * 1000.0 / iterations as f64
        );

        let rss_growth = final_sample.rss_bytes.saturating_sub(baseline.rss_bytes);
        let fd_growth = final_sample.open_fds as i64 - baseline.open_fds as i64;
        println!(
            "soak: RSS Δ {} ({}MB), FDs Δ {}",
            fmt_bytes(rss_growth),
            rss_growth / (1024 * 1024),
            fd_growth
        );

        let mut failed = false;
        if rss_growth > RSS_GROWTH_LIMIT_BYTES {
            eprintln!(
                "soak FAIL: RSS grew {} (limit {})",
                fmt_bytes(rss_growth),
                fmt_bytes(RSS_GROWTH_LIMIT_BYTES)
            );
            failed = true;
        }
        if fd_growth > FD_GROWTH_LIMIT {
            eprintln!("soak FAIL: open FDs grew {fd_growth} (limit {FD_GROWTH_LIMIT})");
            failed = true;
        }
        if failed {
            std::process::exit(1);
        }
        println!("soak: PASS");
    }

    fn sample_points(n: usize) -> Vec<usize> {
        if n < 4 {
            return Vec::new();
        }
        vec![n / 4 - 1, n / 2 - 1, (3 * n) / 4 - 1]
    }

    fn fmt_bytes(b: u64) -> String {
        if b < 1024 {
            format!("{b} B")
        } else if b < 1024 * 1024 {
            format!("{:.1} KiB", b as f64 / 1024.0)
        } else {
            format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0))
        }
    }

    #[derive(Clone)]
    struct Sample {
        label: String,
        rss_bytes: u64,
        open_fds: u64,
    }

    impl Sample {
        fn take(label: &str) -> Self {
            Sample {
                label: label.to_string(),
                rss_bytes: read_rss_bytes().unwrap_or(0),
                open_fds: count_open_fds().unwrap_or(0),
            }
        }

        fn print(&self) {
            println!(
                "  [{:>22}] RSS={:>10}  open_fds={}",
                self.label,
                fmt_bytes(self.rss_bytes),
                self.open_fds
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn read_rss_bytes() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
                return Some(kib * 1024);
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    fn read_rss_bytes() -> Option<u64> {
        // Shell out to ps — `ps -o rss= -p <pid>` returns RSS in KiB. Avoiding
        // libproc/mach FFI keeps the soak task dependency-light.
        let pid = std::process::id();
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        let kib: u64 = std::str::from_utf8(&out.stdout).ok()?.trim().parse().ok()?;
        Some(kib * 1024)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn read_rss_bytes() -> Option<u64> {
        None
    }

    #[cfg(target_os = "linux")]
    fn count_open_fds() -> Option<u64> {
        Some(std::fs::read_dir("/proc/self/fd").ok()?.count() as u64)
    }

    #[cfg(target_os = "macos")]
    fn count_open_fds() -> Option<u64> {
        // /dev/fd is a synthetic filesystem on macOS that lists this process's
        // open descriptors — same shape as /proc/self/fd on Linux.
        Some(std::fs::read_dir("/dev/fd").ok()?.count() as u64)
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn count_open_fds() -> Option<u64> {
        None
    }
}
