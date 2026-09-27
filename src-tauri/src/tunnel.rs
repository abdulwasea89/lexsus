//! A public tunnel for the MCP connector, driven from the desktop.
//!
//! The connector binds loopback only, so a cloud-hosted provider (Claude.ai)
//! can only reach it through an HTTPS tunnel. That used to be entirely manual:
//! the user ran `cloudflared` or `ngrok` in their own terminal, read the URL
//! off the screen, and — because rmcp bakes the DNS-rebinding allowlist into
//! the service at construction — had to put that host in
//! `LEXSUS_MCP_ALLOWED_HOSTS` and *relaunch the app* before it would be
//! accepted. The failure mode was a bare `403` that a connector reads as a
//! sign-in problem.
//!
//! This module closes that loop: it spawns the tunnel, scrapes the public URL
//! out of its output, and hands the host back to `lib.rs`, which allowlists it
//! and restarts the connector.
//!
//! **This is the one part of the app that can make a local tool server
//! reachable from the internet**, so it is never automatic. Starting a tunnel
//! is an explicit, confirmed action; it is announced in the UI the whole time
//! it runs; and it is killed on Stop, on app exit, and on `Drop`, so a tunnel
//! cannot outlive the window that opened it.
//!
//! The tunnel itself is not a security boundary we control — the bearer token
//! required on every request is. A tunnel widens *reachability*, not
//! *authority*: an unauthenticated request through the tunnel is still a 401.

use crate::process;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
#[cfg(not(unix))]
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime};

/// How many output lines to keep for the UI's log tail.
const LOG_CAP: usize = 200;
/// How long a tunnel gets to exit on SIGTERM before it is killed outright.
const KILL_GRACE: Duration = Duration::from_millis(1500);
/// How often the reaper checks whether a tunnel exited on its own.
const REAP_INTERVAL: Duration = Duration::from_millis(250);
/// The event the UI listens on. Carries a [`TunnelStatus`].
pub const UPDATE_EVENT: &str = "tunnel://update";

/// The tunnel tools we know how to drive.
struct Preset {
    id: &'static str,
    label: &'static str,
    /// The binary that must be on `PATH`.
    binary: &'static str,
    /// Arguments with `{port}` substituted at launch.
    args: &'static [&'static str],
    /// One line describing what this provider will do.
    hint: &'static str,
}

const PRESETS: &[Preset] = &[
    Preset {
        id: "cloudflared",
        label: "cloudflared",
        binary: "cloudflared",
        // A throwaway `trycloudflare.com` URL: no account, no config file. The
        // same tool does named tunnels, which the user can wire up as a custom
        // command when they want a stable host.
        args: &["tunnel", "--url", "http://127.0.0.1:{port}"],
        hint: "Anonymous trycloudflare.com URL. No account needed; the URL changes every run.",
    },
    Preset {
        id: "ngrok",
        label: "ngrok",
        binary: "ngrok",
        // `--log stdout` puts the forwarding URL on stdout rather than in a
        // logfile, which is what makes the scrape below work. An ngrok account
        // (and `ngrok config add-authtoken`) is required for a session.
        args: &["http", "{port}", "--log", "stdout"],
        hint: "ngrok HTTPS URL. Needs a free ngrok account and authtoken.",
    },
];

/// One provider the desktop can offer, and whether this machine can run it.
#[derive(Clone, serde::Serialize)]
pub struct Detection {
    pub provider: String,
    pub label: String,
    /// Whether the binary is on `PATH`. The `custom` provider is always usable.
    pub available: bool,
    pub path: Option<String>,
    pub hint: String,
    /// The command that would run, so the UI can show it before you commit to
    /// exposing anything.
    pub preview: String,
}

/// A live tunnel's state, as the UI renders it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TunnelStatus {
    pub running: bool,
    pub provider: String,
    /// The public URL, once the provider has printed one.
    pub url: Option<String>,
    /// The hostname to allowlist — the URL without scheme, port or path, which
    /// is what arrives in the `Host` header.
    pub host: Option<String>,
    /// The local port being forwarded.
    pub port: u16,
    /// Recent output, oldest first, for the log tail.
    pub log: Vec<String>,
}

/// Shared state. The reader threads and the reaper hold the same `Arc`, which
/// is why this is not behind a plain `Mutex` on the struct.
struct Inner {
    /// The tunnel process. `None` once it has been stopped or reaped.
    child: Option<Child>,
    provider: String,
    url: Option<String>,
    host: Option<String>,
    log: VecDeque<String>,
    port: u16,
    /// Set once the user has asked to stop, so the reaper does not report a
    /// tidy shutdown as an unexpected exit.
    stopping: bool,
}

/// The desktop's tunnel, if any. One at a time: two tunnels to the same
/// endpoint would mean two public URLs with two hosts to allowlist, and no
/// user intent that produces that.
pub struct Tunnel {
    inner: Arc<Mutex<Inner>>,
}

impl Default for Tunnel {
    fn default() -> Self {
        Self::new()
    }
}

impl Tunnel {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                child: None,
                provider: String::new(),
                url: None,
                host: None,
                log: VecDeque::new(),
                port: 0,
                stopping: false,
            })),
        }
    }

    /// Which providers this machine can actually run.
    pub fn detect(port: u16) -> Vec<Detection> {
        let mut found: Vec<Detection> = PRESETS
            .iter()
            .map(|preset| {
                let path = which(preset.binary);
                Detection {
                    provider: preset.id.to_string(),
                    label: preset.label.to_string(),
                    available: path.is_some(),
                    path,
                    hint: preset.hint.to_string(),
                    preview: render_command(preset, port).join(" "),
                }
            })
            .collect();
        // Always last, and always available: if the user runs a tunnel we do
        // not know about, they can still drive it from here.
        found.push(Detection {
            provider: "custom".to_string(),
            label: "Custom command".to_string(),
            available: true,
            path: None,
            hint: "Run any command you like, with {port} replaced by the connector's port. \
                   The first https:// URL it prints is taken as the public one."
                .to_string(),
            preview: String::new(),
        });
        found
    }

    pub fn status(&self) -> TunnelStatus {
        let inner = self.inner.lock().unwrap();
        TunnelStatus {
            running: inner.child.is_some(),
            provider: inner.provider.clone(),
            url: inner.url.clone(),
            host: inner.host.clone(),
            port: inner.port,
            log: inner.log.iter().cloned().collect(),
        }
    }

    /// Spawn a tunnel and start watching its output for the public URL.
    ///
    /// Returns as soon as the process is up. The URL arrives later, over the
    /// [`UPDATE_EVENT`] event and in [`Tunnel::status`], because that is when
    /// the provider prints it — blocking here would mean a UI that hangs for
    /// however long the provider's TLS handshake takes.
    pub fn start<R: Runtime>(
        &self,
        app: AppHandle<R>,
        provider: &str,
        custom: Option<&str>,
        port: u16,
    ) -> Result<TunnelStatus, String> {
        {
            let inner = self.inner.lock().unwrap();
            if inner.child.is_some() {
                return Err("a tunnel is already running".to_string());
            }
        }

        let mut command = match provider {
            "custom" => {
                let template = custom
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| "a custom tunnel needs a command".to_string())?;
                let rendered = template.replace("{port}", &port.to_string());
                // The user typed this command themselves, on their own
                // machine, to run as themselves — there is no privilege
                // boundary here to protect. A shell is required because a
                // custom command is a string, not an argv we can split safely.
                let mut c = Command::new("sh");
                c.arg("-c").arg(rendered);
                c
            }
            id => {
                let preset = PRESETS
                    .iter()
                    .find(|p| p.id == id)
                    .ok_or_else(|| format!("unknown tunnel provider: {id}"))?;
                let argv = render_command(preset, port);
                let path = which(preset.binary)
                    .ok_or_else(|| format!("{} is not installed", preset.label))?;
                let mut c = Command::new(path);
                c.args(&argv[1..]);
                c
            }
        };

        // Its own process group, so `process::kill_tree` can take the whole
        // tree down — a custom command is `sh -c`, which is a shell *and* what
        // it spawned.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot start the tunnel: {e}"))?;
        let pid = child.id();

        // The pipes are taken before the child is stored, so the reader
        // threads own them outright.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        {
            let mut inner = self.inner.lock().unwrap();
            inner.child = Some(child);
            inner.provider = provider.to_string();
            inner.url = None;
            inner.host = None;
            inner.port = port;
            inner.stopping = false;
            inner.log.clear();
            inner
                .log
                .push_back(format!("[lexsus] tunnel started (pid {pid})"));
        }

        for stream in [
            stdout.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            stderr.map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let inner = self.inner.clone();
            let app = app.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    let inner_clone = inner.clone();
                    let app_clone = app.clone();
                    absorb_line(&inner_clone, &app_clone, line);
                }
            });
        }

        // Reaper: notices a tunnel that exits on its own — a bad authtoken, a
        // provider outage — so the UI does not go on claiming it is live.
        {
            let inner = self.inner.clone();
            let app = app.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(REAP_INTERVAL);
                let mut guard = inner.lock().unwrap();
                let Some(child) = guard.child.as_mut() else {
                    return; // stopped by hand; nothing left to watch
                };
                match child.try_wait() {
                    Ok(Some(exit)) => {
                        let stopping = guard.stopping;
                        guard.child = None;
                        guard.url = None;
                        guard.host = None;
                        let note = if stopping {
                            format!("[lexsus] tunnel stopped ({exit})")
                        } else {
                            format!("[lexsus] tunnel exited on its own ({exit})")
                        };
                        push_log(&mut guard, note);
                        drop(guard);
                        let _ = app.emit(UPDATE_EVENT, ());
                        return;
                    }
                    // Still running, or the status could not be read; either
                    // way keep watching rather than declaring it dead.
                    _ => {}
                }
            });
        }

        let status = self.status();
        let _ = app.emit(UPDATE_EVENT, ());
        Ok(status)
    }

    /// Stop the tunnel and everything it spawned.
    ///
    /// Idempotent, so the quit path can call it whether or not one is running.
    pub fn stop(&self) -> TunnelStatus {
        let child = {
            let mut inner = self.inner.lock().unwrap();
            inner.stopping = true;
            inner.child.take()
        };
        if let Some(mut child) = child {
            let pid = child.id();
            // Group first, then reap. `kill_tree` handles the TERM→wait→KILL
            // escalation; the `wait` below collects the leader so it does not
            // linger as a zombie.
            process::kill_tree(pid, KILL_GRACE);
            let _ = child.wait();
            let mut inner = self.inner.lock().unwrap();
            inner.url = None;
            inner.host = None;
            // The message is only worth recording if this was not already
            // reaped — but the log is cleared on the next start either way.
            push_log(&mut inner, format!("[lexsus] tunnel stopped (pid {pid})"));
        }
        self.status()
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Record one output line: keep it for the log tail, and if it is the first
/// line carrying a public URL, adopt it as the tunnel's address.
fn absorb_line<R: Runtime>(inner: &Arc<Mutex<Inner>>, app: &AppHandle<R>, line: String) {
    let mut guard = inner.lock().unwrap();
    let discovered = if guard.url.is_none() {
        public_url(&line)
    } else {
        None
    };
    push_log(&mut guard, line);
    if let Some(url) = discovered {
        guard.host = host_of(&url);
        guard.url = Some(url);
        drop(guard);
        let _ = app.emit(UPDATE_EVENT, ());
    }
}

fn push_log(inner: &mut Inner, line: String) {
    // Providers are chatty; the tail is for a human glancing at a card, not an
    // audit record.
    while inner.log.len() >= LOG_CAP {
        inner.log.pop_front();
    }
    inner.log.push_back(line);
}

/// The public URL in one line of tunnel output, if there is one.
///
/// Both providers print the forwarding URL among plenty of other noise, so the
/// filter matters more than the match: the *local* address is rejected
/// (loopback, or ngrok's own `127.0.0.1:4040` dashboard), and so is ngrok's
/// `dashboard.ngrok.com` — a tunnel host is never under `ngrok.com`.
fn public_url(line: &str) -> Option<String> {
    static URL: OnceLock<regex::Regex> = OnceLock::new();
    let re = URL.get_or_init(|| {
        regex::Regex::new(r"https://[A-Za-z0-9][A-Za-z0-9.-]*[A-Za-z0-9]")
            .expect("the URL pattern is a literal")
    });
    for found in re.find_iter(line) {
        let url = found.as_str().to_string();
        let Some(host) = host_of(&url) else { continue };
        if is_local(&host) || host == "ngrok.com" || host.ends_with(".ngrok.com") {
            continue;
        }
        return Some(url);
    }
    None
}

/// The hostname out of a URL, without scheme, port or path — which is the form
/// the `Host` header and the allowlist both use.
fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let authority = rest.split(['/', '?', '#']).next()?;
    // Reject userinfo, which no tunnel URL has but a crafted line might.
    if authority.contains('@') {
        return None;
    }
    let host = authority.split(':').next()?;
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

fn is_local(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "127.0.0.1"
        || host.starts_with("127.")
        || host == "::1"
        || host == "0.0.0.0"
}

/// Arguments for a preset, with `{port}` already substituted. Element 0 is the
/// program, so the UI can print the whole thing as one line.
fn render_command(preset: &Preset, port: u16) -> Vec<String> {
    std::iter::once(preset.binary.to_string())
        .chain(
            preset
                .args
                .iter()
                .map(|a| a.replace("{port}", &port.to_string())),
        )
        .collect()
}

/// Find a binary on `PATH` without shelling out to `which`.
fn which(binary: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable(candidate))
        .map(|p| p.display().to_string())
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &PathBuf) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The URL scrape is the piece with real edge cases: providers bracket the
    /// address in banners and log noise, and the first match has to be the
    /// public one rather than a local dashboard.
    #[test]
    fn the_public_url_is_scraped_from_real_provider_output() {
        // cloudflared's banner, verbatim in shape.
        assert_eq!(
            public_url("|  https://steady-piano-abc.trycloudflare.com  |"),
            Some("https://steady-piano-abc.trycloudflare.com".to_string())
        );
        assert_eq!(
            public_url("2026-09-27T10:00:00Z INF +-----+"),
            None,
            "banner chrome is not a URL"
        );
        // ngrok logs the local dashboard and the forward on separate lines.
        assert_eq!(
            public_url("Web Interface                 http://127.0.0.1:4040"),
            None,
            "the local dashboard is http, and loopback besides"
        );
        assert_eq!(
            public_url("Forwarding                    https://abc-123.ngrok-free.app -> http://127.0.0.1:45147"),
            Some("https://abc-123.ngrok-free.app".to_string()),
            "the forwarding target, not the destination it forwards to"
        );
    }

    /// A loopback or ngrok-dashboard URL must never be adopted as the tunnel's
    /// public address — allowlisting either would be meaningless or wrong.
    #[test]
    fn local_and_dashboard_urls_are_rejected() {
        for rejected in [
            "https://localhost:8443",
            "https://127.0.0.1",
            "https://dashboard.ngrok.com/get-started",
            "https://ngrok.com",
        ] {
            assert_eq!(public_url(rejected), None, "{rejected} should be rejected");
        }
    }

    #[test]
    fn a_host_is_the_url_without_scheme_port_or_path() {
        assert_eq!(
            host_of("https://abc.ngrok-free.app").as_deref(),
            Some("abc.ngrok-free.app")
        );
        assert_eq!(
            host_of("https://abc.example.test:8443/mcp").as_deref(),
            Some("abc.example.test"),
            "a port is not part of the Host header"
        );
        assert_eq!(
            host_of("https://ABC.Example.Test/").as_deref(),
            Some("abc.example.test")
        );
        assert_eq!(
            host_of("https://user@evil.test"),
            None,
            "userinfo is not a host"
        );
    }

    /// Detection always offers a custom command, so an unknown tunnel tool is
    /// still drivable, and never claims an absent binary is available.
    #[test]
    fn detection_reports_availability_honestly() {
        let found = Tunnel::detect(45147);
        let custom = found
            .iter()
            .find(|d| d.provider == "custom")
            .expect("custom is always offered");
        assert!(custom.available);

        for preset in PRESETS {
            let detected = found
                .iter()
                .find(|d| d.provider == preset.id)
                .expect("every preset is reported");
            assert_eq!(
                detected.available,
                which(preset.binary).is_some(),
                "{} availability must match PATH",
                preset.id
            );
            // A preset's preview must carry the port, or the UI shows a
            // command that would forward nothing.
            assert!(detected.preview.contains("45147"), "{}", detected.preview);
        }
    }

    /// A never-started tunnel reports stopped, and stopping it is a no-op — so
    /// the quit path can call it unconditionally.
    #[test]
    fn a_fresh_tunnel_is_stopped_and_stopping_is_idempotent() {
        let tunnel = Tunnel::new();
        assert!(!tunnel.status().running);
        let after = tunnel.stop();
        assert!(!after.running);
        assert!(after.url.is_none());
        // Twice, to prove the second call is not the one that works.
        assert!(!tunnel.stop().running);
    }

    /// A custom command is required rather than defaulted: silently running
    /// something is not an option for the one action that exposes the machine.
    #[test]
    fn a_custom_tunnel_without_a_command_is_refused() {
        let tunnel = Tunnel::new();
        let app = crate::test_app();
        let error = tunnel
            .start(app.clone(), "custom", None, 45147)
            .expect_err("no command");
        assert!(error.contains("needs a command"), "unexpected: {error}");
        assert!(!tunnel.status().running);

        let blank = tunnel
            .start(app, "custom", Some("   "), 45147)
            .expect_err("blank command");
        assert!(blank.contains("needs a command"), "unexpected: {blank}");
    }

    /// An unknown provider is refused rather than silently falling back to
    /// something that might work.
    #[test]
    fn an_unknown_provider_is_refused() {
        let tunnel = Tunnel::new();
        let app = crate::test_app();
        let error = tunnel
            .start(app, "definitely-not-a-tunnel", None, 45147)
            .expect_err("unknown provider");
        assert!(
            error.contains("unknown tunnel provider"),
            "unexpected: {error}"
        );
    }

    /// The whole point of the module: a real process is spawned, its URL is
    /// scraped, and stopping it kills the process rather than just forgetting
    /// it. Uses `sh` rather than a tunnel binary so it runs anywhere.
    #[cfg(unix)]
    #[test]
    fn a_started_tunnel_is_scraped_and_then_killed() {
        let tunnel = Tunnel::new();
        let app = crate::test_app();
        // A custom command standing in for a provider: prints a forwarding
        // line, then stays alive like a tunnel does.
        let script = "echo 'Forwarding  https://unit-test-tunnel.example.test -> http://127.0.0.1:{port}'; sleep 300";
        tunnel
            .start(app, "custom", Some(script), 45147)
            .expect("the tunnel starts");
        assert!(tunnel.status().running);

        // The URL is adopted off the process's own output, asynchronously.
        let discovered = wait_for_url(&tunnel);
        assert_eq!(
            discovered.as_deref(),
            Some("https://unit-test-tunnel.example.test")
        );
        assert_eq!(
            tunnel.status().host.as_deref(),
            Some("unit-test-tunnel.example.test"),
            "the host the allowlist needs is derived from the URL"
        );

        let pid = {
            let guard = tunnel.inner.lock().unwrap();
            guard.child.as_ref().map(|c| c.id()).expect("still running")
        };
        let stopped = tunnel.stop();
        assert!(!stopped.running);
        assert!(stopped.url.is_none());
        assert!(
            !process_alive(pid),
            "stopping must kill the tunnel process, not just forget it"
        );
    }

    /// A tunnel that exits on its own must stop being reported as running,
    /// rather than leaving the UI claiming the endpoint is exposed.
    #[cfg(unix)]
    #[test]
    fn a_tunnel_that_exits_on_its_own_is_reaped() {
        let tunnel = Tunnel::new();
        let app = crate::test_app();
        tunnel
            .start(app, "custom", Some("echo 'no url here'; exit 3"), 45147)
            .expect("the tunnel starts");

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while tunnel.status().running && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = tunnel.status();
        assert!(!status.running, "an exited tunnel must not read as running");
        assert!(
            status.log.iter().any(|l| l.contains("on its own")),
            "why it went away belongs in the log: {:?}",
            status.log
        );
    }

    #[cfg(unix)]
    fn wait_for_url(tunnel: &Tunnel) -> Option<String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if let Some(url) = tunnel.status().url {
                return Some(url);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        None
    }

    /// Whether `pid` is still alive, as a `kill(pid, 0)` probe.
    #[cfg(unix)]
    fn process_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
}
