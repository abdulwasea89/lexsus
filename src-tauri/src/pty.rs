//! PTY command execution.
//!
//! Every command runs as a temporary child in its own PTY — there is no
//! persistent interactive session. Output is captured with a hard timeout
//! and an output cap; [`run_command_stream`] additionally reports each
//! chunk as it arrives so callers can stream a web AI's commands into the
//! UI.

use crate::shell::Shell;

use portable_pty::{native_pty_system, PtySize};

use std::io::Read;

use std::path::Path;

use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};

use std::thread;

use std::time::{Duration, Instant};

pub const DEFAULT_ROWS: u16 = 24;

pub const DEFAULT_COLS: u16 = 80;

/// One-shot command result (serde: mirrors `CommandOutput` in the frontend).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandOutput {
    pub command: String,
    pub exit_code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
    pub truncated: bool,
}

/// Run a shell command, capturing its full output.
pub fn run_command(
    cmd: &str,
    cwd: &Path,
    timeout: Duration,
    max_output: usize,
) -> std::io::Result<CommandOutput> {
    run_command_stream(
        Shell::detect(),
        cmd,
        cwd,
        timeout,
        max_output,
        &mut |_| {},
        None,
    )
}

/// Run a shell command with an explicit shell, streaming each output chunk
/// to `on_output` as it arrives. Completion is the child process exiting,
/// not stream EOF (ConPTY streams linger after exit), followed by a quiet
/// drain for trailing output. `on_spawn` receives the child pid right after
/// spawn, so the caller can register it in the process registry (cancel
/// kills by pid, not by guesswork).
pub fn run_command_stream(
    shell: Shell,
    cmd: &str,
    cwd: &Path,
    timeout: Duration,
    max_output: usize,
    on_output: &mut dyn FnMut(String),
    on_spawn: Option<&mut dyn FnMut(u32)>,
) -> std::io::Result<CommandOutput> {
    let pair = open_pty(DEFAULT_ROWS, DEFAULT_COLS)?;
    let mut builder = shell.run_command(cmd);
    builder.cwd(cwd);
    let mut child = pair
        .slave
        .spawn_command(builder)
        .map_err(|e| io_err(e.to_string()))?;
    drop(pair.slave);
    if let (Some(cb), Some(pid)) = (on_spawn, child.process_id()) {
        cb(pid);
    }
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io_err(e.to_string()))?;
    let (tx, rx) = sync_channel::<Vec<u8>>(4096);
    thread::spawn(move || read_loop(&mut reader, &tx));

    let start = Instant::now();
    let mut output = String::new();
    let mut timed_out = false;
    let mut truncated = false;
    let mut exited = false;

    'read: loop {
        if start.elapsed() >= timeout {
            timed_out = true;
            break 'read;
        }
        if !exited && child.try_wait().ok().flatten().is_some() {
            exited = true;
        }
        let tick = if exited {
            Duration::from_millis(500)
        } else {
            Duration::from_millis(100)
        };

        match rx.recv_timeout(tick) {
            Ok(chunk) => {
                if output.len() + chunk.len() > max_output {
                    truncated = true;
                    break 'read;
                }
                let text = String::from_utf8_lossy(&chunk).into_owned();
                output.push_str(&text);
                on_output(text);
            }
            Err(RecvTimeoutError::Timeout) => {
                if exited {
                    break 'read;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break 'read,
        }
    }
    if timed_out || truncated {
        let _ = child.kill();
    }

    // Drain so the reader thread can finish after the kill.
    while rx.try_recv().is_ok() {}
    let exit_code = child.wait().ok().map(|s| s.exit_code() as i32);
    Ok(CommandOutput {
        command: cmd.to_string(),
        exit_code,
        output,
        timed_out,
        truncated,
    })
}

fn read_loop(reader: &mut dyn Read, tx: &SyncSender<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
}

fn open_pty(rows: u16, cols: u16) -> std::io::Result<portable_pty::PtyPair> {
    let pty_system = native_pty_system();
    pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| io_err(e.to_string()))
}

fn io_err<E: std::fmt::Display>(e: E) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir() -> PathBuf {
        std::env::temp_dir()
    }

    fn echo_cmd(shell: Shell, text: &str) -> String {
        match shell {
            Shell::PowerShell => format!("Write-Output '{text}'"),
            Shell::Cmd => format!("echo {text}"),
            _ => format!("echo {text}"),
        }
    }

    fn slow_cmd(shell: Shell) -> String {
        match shell {
            Shell::PowerShell => "Start-Sleep -Seconds 30".to_string(),
            Shell::Cmd => "timeout /t 30 /nobreak".to_string(),
            _ => "sleep 30".to_string(),
        }
    }

    fn long_cmd(shell: Shell) -> String {
        match shell {
            Shell::PowerShell => "1..20000 | ForEach-Object { 'line' }".to_string(),
            Shell::Cmd => "for /l %i in (1,1,20000) do echo line".to_string(),
            _ => "seq 1 20000".to_string(),
        }
    }

    fn bad_cmd(_shell: Shell) -> String {
        "definitely-not-a-real-command-xyz".to_string()
    }

    #[test]
    fn run_command_echo() {
        let shell = Shell::detect();
        let out = run_command(
            &echo_cmd(shell, "pty-echo"),
            &tmpdir(),
            Duration::from_secs(20),
            1_048_576,
        )
        .unwrap();
        assert_eq!(out.exit_code, Some(0), "output: {:?}", out.output);
        assert!(out.output.contains("pty-echo"));
        assert!(!out.timed_out && !out.truncated);
    }

    #[test]
    fn run_command_streams_output() {
        let shell = Shell::detect();
        let mut chunks = String::new();
        let out = run_command_stream(
            shell,
            &echo_cmd(shell, "pty-stream"),
            &tmpdir(),
            Duration::from_secs(20),
            1_048_576,
            &mut |chunk| chunks.push_str(&chunk),
            None,
        )
        .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(chunks.contains("pty-stream"), "streamed: {chunks:?}");
    }

    #[test]
    fn run_command_reports_child_pid() {
        let shell = Shell::detect();
        let mut pid = None;
        let out = run_command_stream(
            shell,
            &echo_cmd(shell, "pty-pid"),
            &tmpdir(),
            Duration::from_secs(20),
            1_048_576,
            &mut |_| {},
            Some(&mut |p| pid = Some(p)),
        )
        .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(pid.unwrap() > 1, "on_spawn must report the child pid");
    }

    #[test]
    fn run_command_timeout() {
        let shell = Shell::detect();
        let out = run_command(
            &slow_cmd(shell),
            &tmpdir(),
            Duration::from_secs(1),
            1_048_576,
        )
        .unwrap();
        assert!(out.timed_out, "expected timed_out, got {:?}", out.output);
        assert!(out.exit_code.is_some(), "killed process must yield a code");
    }

    #[test]
    fn run_command_output_cap() {
        let shell = Shell::detect();
        let out = run_command(&long_cmd(shell), &tmpdir(), Duration::from_secs(20), 1024).unwrap();
        assert!(
            out.truncated,
            "expected truncated, got {} bytes",
            out.output.len()
        );
        assert!(
            out.output.len() <= 1024 + 4096,
            "output may exceed cap by at most one chunk, got {}",
            out.output.len()
        );
    }

    #[test]
    fn run_command_failure() {
        let shell = Shell::detect();
        let out = run_command(
            &bad_cmd(shell),
            &tmpdir(),
            Duration::from_secs(20),
            1_048_576,
        )
        .unwrap();
        assert_ne!(out.exit_code, Some(0));
    }

    /// Windows is a first-class target: prove both shells work, not just
    /// whatever `detect()` picks.
    #[cfg(windows)]
    #[test]
    fn windows_both_shells_work() {
        for shell in [Shell::Cmd, Shell::PowerShell] {
            let out = run_command_stream(
                shell,
                &echo_cmd(shell, "pty-shells"),
                &tmpdir(),
                Duration::from_secs(30),
                1_048_576,
                &mut |_| {},
                None,
            )
            .unwrap();
            assert_eq!(out.exit_code, Some(0), "shell {shell:?}: {:?}", out.output);
            assert!(out.output.contains("pty-shells"), "shell {shell:?}");
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn posix_sh_works() {
        let out = run_command_stream(
            Shell::Sh,
            "echo pty-sh",
            &tmpdir(),
            Duration::from_secs(20),
            1_048_576,
            &mut |_| {},
            None,
        )
        .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(out.output.contains("pty-sh"));
    }
}
