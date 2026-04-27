//! Process-spawn helpers extracted from [`Browser::launch`] so the public entry
//! point is a thin coordinator instead of a 250-line monolith.
//!
//! The functions in this module are `pub(super)` — only `browser/mod.rs` calls
//! them. Splitting by responsibility:
//!
//! - [`resolve_user_data_dir`] — read the configured dir or create a tempdir.
//! - [`build_command`] — translate `BrowserConfig` into Chromium CLI flags.
//! - [`spawn_pipe`] (Unix only) — owns the unsafe pipe-fd dance.
//! - [`spawn_websocket`] — spawns + parses stderr for `ws://…`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chromist_types::CdpJsonEventMessage;

use super::argument;
use super::config::{self, BrowserConfig};
use crate::conn::{AnyConnection, Connection};
use crate::error::CdpError;

/// Maximum stderr lines retained for inclusion in launch errors. Cap prevents
/// a chatty browser from growing the buffer unboundedly.
const MAX_STDERR_LINES: usize = 200;

/// Resolve the user-data directory: explicit path from config, or a fresh
/// tempdir owned by this `Browser` (the second tuple element is `Some` when
/// the dir is owned and should be cleaned up on drop).
pub(super) fn resolve_user_data_dir(
    config: &BrowserConfig,
) -> crate::Result<(PathBuf, Option<PathBuf>)> {
    match &config.user_data_dir {
        Some(dir) => Ok((dir.clone(), None)),
        None => {
            let tmp = super::tempdir()?;
            Ok((tmp.clone(), Some(tmp)))
        }
    }
}

/// Translate a [`BrowserConfig`] into a `std::process::Command` ready to
/// `spawn()`. The caller is responsible for transport-specific stdio wiring
/// (pipe mode redirects stderr to null; WebSocket mode pipes stderr).
pub(super) fn build_command(config: &BrowserConfig, exe: &Path, user_data_dir: &Path) -> Command {
    let mut cmd = Command::new(exe);

    for (k, v) in &config.process_envs {
        cmd.env(k, v);
    }

    // Transport selection: pipe mode skips --remote-debugging-port; the
    // caller adds `--remote-debugging-pipe` when entering [`spawn_pipe`].
    if !config.use_pipe {
        cmd.arg(format!("--remote-debugging-port={}", config.port));
    }
    cmd.arg(format!("--user-data-dir={}", user_data_dir.display()));

    if !config.sandbox {
        cmd.arg("--no-sandbox");
    }

    if let Some((w, h)) = config.window_size {
        cmd.arg(format!("--window-size={w},{h}"));
    } else if let Some(vp) = config.viewport {
        cmd.arg(format!("--window-size={},{}", vp.width, vp.height));
    }

    if config.incognito {
        cmd.arg("--incognito");
    }

    if !config.extensions.is_empty() {
        let list =
            config.extensions.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(",");
        cmd.arg(format!("--load-extension={list}"));
        cmd.arg(format!("--disable-extensions-except={list}"));
    }

    if !config.ignore_default_args {
        for a in argument::default_args() {
            if !config.disable_default_args.iter().any(|d| d == a) {
                cmd.arg(a);
            }
        }
    }

    match config.headless {
        config::HeadlessMode::New => {
            for a in argument::headless_args() {
                cmd.arg(a);
            }
        }
        config::HeadlessMode::Legacy => {
            cmd.arg("--headless");
            cmd.arg("--hide-scrollbars");
            cmd.arg("--mute-audio");
        }
        config::HeadlessMode::False => {}
    }

    if let Some(ref proxy) = config.proxy_server {
        cmd.arg(format!("--proxy-server={proxy}"));
    }
    if let Some(ref bypass) = config.proxy_bypass_list {
        cmd.arg(format!("--proxy-bypass-list={bypass}"));
    }

    for a in &config.args {
        cmd.arg(a);
    }
    cmd.arg("about:blank");

    cmd
}

/// Spawn the browser via the `--remote-debugging-pipe` transport on Unix.
///
/// Owns the unsafe pipe-fd dance: creates the two pipes, dup2s into fds 3/4
/// in the child via `pre_exec`, closes parent-side ends after spawn, and
/// wraps the parent ends as tokio async files. An RAII `PipeFds` guard
/// closes everything on `spawn()` failure to prevent fd leaks.
#[cfg(unix)]
pub(super) fn spawn_pipe(mut cmd: Command) -> crate::Result<(Child, AnyConnection)> {
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::CommandExt;

    cmd.arg("--remote-debugging-pipe");
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    // Pipe A: parent writes commands → Chrome reads from fd 3.
    let mut fds_a = [0i32; 2];
    // Pipe B: Chrome writes responses to fd 4 → parent reads.
    let mut fds_b = [0i32; 2];

    // SAFETY: `libc::pipe` is async-signal-safe; the return value is checked,
    // and `fds_a` is a properly aligned `[i32; 2]` array owned by this stack
    // frame, satisfying the OS contract that the pointer must reference at
    // least two writable `i32`s.
    if unsafe { libc::pipe(fds_a.as_mut_ptr()) } < 0 {
        return Err(CdpError::LaunchPipeError { source: std::io::Error::last_os_error() });
    }
    // SAFETY: same conditions as `fds_a` immediately above — `fds_b` is a
    // freshly stack-allocated `[i32; 2]` array.
    if unsafe { libc::pipe(fds_b.as_mut_ptr()) } < 0 {
        // SAFETY: `fds_a` was successfully opened immediately above and has
        // not been duplicated or moved out of this scope; the fds are still
        // valid and exclusively owned here.
        unsafe {
            libc::close(fds_a[0]);
            libc::close(fds_a[1]);
        }
        return Err(CdpError::LaunchPipeError { source: std::io::Error::last_os_error() });
    }

    // fds_a[0] = read end  → Chrome reads commands from fd 3
    // fds_a[1] = write end → parent writes commands
    // fds_b[0] = read end  → parent reads responses
    // fds_b[1] = write end → Chrome writes responses to fd 4
    let chrome_read_fd: i32 = fds_a[0];
    let parent_write_fd: i32 = fds_a[1];
    let parent_read_fd: i32 = fds_b[0];
    let chrome_write_fd: i32 = fds_b[1];

    // Guard ensures the four pipe fds are closed if anything between here and
    // `cmd.spawn()` fails — without it, a spawn error would leak all four
    // into the parent process. Released (`defuse`d) only after spawn succeeds
    // and ownership has transferred.
    struct PipeFds([i32; 4]);
    impl Drop for PipeFds {
        fn drop(&mut self) {
            for fd in self.0 {
                if fd >= 0 {
                    // SAFETY: each fd was opened by `libc::pipe` in this
                    // function, has not been transferred to another owner
                    // (otherwise `defuse` would have zeroed it), and is
                    // closed at most once because `Drop` runs at most once.
                    unsafe {
                        libc::close(fd);
                    }
                }
            }
        }
    }
    impl PipeFds {
        fn defuse(mut self) {
            self.0 = [-1; 4];
        }
    }
    let pipe_guard = PipeFds([chrome_read_fd, parent_write_fd, parent_read_fd, chrome_write_fd]);

    // SAFETY: `pre_exec` closures execute in the child between `fork(2)` and
    // `execve(2)`. Only async-signal-safe libc calls (`dup2`, `close`) are
    // invoked. `chrome_read_fd` and `chrome_write_fd` are captured by `Copy`,
    // so the closure does not reference any heap state that fork-time
    // copy-on-write semantics could invalidate. The fds were opened in the
    // parent before fork and are therefore valid in the child's fd table.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(chrome_read_fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::dup2(chrome_write_fd, 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if chrome_read_fd != 3 {
                libc::close(chrome_read_fd);
            }
            if chrome_write_fd != 4 {
                libc::close(chrome_write_fd);
            }
            Ok(())
        });
    }

    let child = cmd.spawn().map_err(|e| CdpError::LaunchSpawnError { source: e })?;

    // Spawn succeeded — disarm the leak guard. The child-side ends are
    // closed in the parent below; the parent-side ends are moved into the
    // tokio `File` wrappers, which take ownership.
    pipe_guard.defuse();

    // Close the child-side fds in the parent process — Chrome now owns them
    // exclusively via the `dup2`-installed copies.
    // SAFETY: these fds were opened by `libc::pipe` in this function, have
    // not been transferred to any other owner in the parent (only `dup2` in
    // the child copied them, which does not affect the parent's table), and
    // have not yet been closed.
    unsafe {
        libc::close(chrome_read_fd);
        libc::close(chrome_write_fd);
    }

    // Wrap the parent-side fds as async tokio files.
    // SAFETY: `parent_write_fd` and `parent_read_fd` are valid open file
    // descriptors that this function holds exclusively at this point — no
    // other code path closes them. The `File::from_raw_fd` contract requires
    // exactly that. After this point ownership transfers to the `File`,
    // which closes them on drop.
    let writer = tokio::fs::File::from_std(unsafe { std::fs::File::from_raw_fd(parent_write_fd) });
    // SAFETY: same conditions as `parent_write_fd` above.
    let reader = tokio::fs::File::from_std(unsafe { std::fs::File::from_raw_fd(parent_read_fd) });

    let pipe_conn = crate::pipe_conn::PipeConnection::<CdpJsonEventMessage>::new(writer, reader);
    Ok((child, AnyConnection::Pipe(pipe_conn)))
}

/// Spawn the browser via the `--remote-debugging-port=…` transport, parse
/// `DevTools listening on ws://…` from stderr, and connect.
///
/// Returns the child process, the established WebSocket connection, and the
/// discovered `ws://` URL.
pub(super) async fn spawn_websocket(
    mut cmd: Command,
    timeout: Duration,
) -> crate::Result<(Child, AnyConnection, String)> {
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| CdpError::LaunchSpawnError { source: e })?;

    let stderr = child.stderr.take().ok_or_else(|| CdpError::LaunchSpawnError {
        source: std::io::Error::other("child process has no stderr handle"),
    })?;

    let (tx, rx) = mpsc::channel::<Option<String>>();
    let stderr_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_log_thread = Arc::clone(&stderr_log);
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        let mut sent = false;
        for line in reader.lines().map_while(Result::ok) {
            if !sent {
                if let Some(idx) = line.find("ws://") {
                    let url = line[idx..].trim().to_string();
                    let _ = tx.send(Some(url));
                    sent = true;
                }
            }
            if let Ok(mut log) = stderr_log_thread.lock() {
                if log.len() >= MAX_STDERR_LINES {
                    log.remove(0);
                }
                log.push(line);
            }
        }
        if !sent {
            let _ = tx.send(None);
        }
    });
    let collect_stderr =
        || -> String { stderr_log.lock().map(|l| l.join("\n")).unwrap_or_default() };

    let deadline = std::time::Instant::now() + timeout;
    let ws_url = loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            tracing::debug!("timed out waiting for browser WebSocket URL");
            let _ = child.kill();
            return Err(CdpError::LaunchTimeout { stderr: collect_stderr() });
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(url)) => break url,
            Ok(None) => {
                tracing::debug!("browser exited before WebSocket URL was found");
                let exit_status =
                    child.try_wait().ok().flatten().map(|s| s.to_string()).unwrap_or_default();
                let _ = child.kill();
                return Err(CdpError::LaunchExit { exit_status, stderr: collect_stderr() });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::debug!("stderr reader disconnected before WebSocket URL was found");
                let exit_status =
                    child.try_wait().ok().flatten().map(|s| s.to_string()).unwrap_or_default();
                let _ = child.kill();
                return Err(CdpError::LaunchExit { exit_status, stderr: collect_stderr() });
            }
        }
    };

    let conn: Connection<CdpJsonEventMessage> = Connection::connect(&ws_url).await?;
    Ok((child, AnyConnection::Ws(conn), ws_url))
}
