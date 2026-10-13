//! Bounded subprocesses.
//!
//! The program runs from an argv -- never through a shell -- with stdin on
//! /dev/null unless input is given, in its own process group. Both output
//! streams are captured up to a per-stream limit and drained past it, so a
//! verbose child can never block on a full pipe. At the deadline the whole
//! group is killed, which also stops descendants that inherited the pipes.
//! An exit code is reported only when the command finished in time and both
//! pipes reached end of file: output cut short must not look like success.

use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use omt_sys::posix::{self, POLLIN, POLLOUT, SIGKILL, pollfd};

pub const OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    /// `argv[0]` is the program path.
    pub argv: &'a [&'a str],
    pub timeout_ms: u64,
    /// Per stream; 0 means [`OUTPUT_LIMIT`].
    pub output_limit: usize,
    /// `None` connects stdin to /dev/null.
    pub stdin: Option<&'a [u8]>,
    /// `None` inherits the environment; otherwise exactly these variables.
    pub env: Option<&'a [(&'a str, &'a str)]>,
}

impl<'a> Options<'a> {
    pub fn new(argv: &'a [&'a str], timeout_ms: u64) -> Self {
        Self { argv, timeout_ms, output_limit: 0, stdin: None, env: None }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    /// argv joined with spaces, for reports.
    pub command: String,
    pub returncode: Option<i32>,
    /// Lossily decoded UTF-8.
    pub stdout: String,
    pub stderr: String,
    pub duration_seconds: f64,
    pub timed_out: bool,
    /// Why the command could not run or was cut short; empty otherwise.
    pub error: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

impl Output {
    /// The most useful one-line explanation of a failure.
    pub fn failure_detail(&self) -> &str {
        if !self.error.is_empty() {
            return &self.error;
        }
        let err = self.stderr.trim();
        if err.is_empty() { self.stdout.trim() } else { err }
    }

    pub fn report_text(&self) -> &str {
        let out = self.stdout.trim();
        if !out.is_empty() {
            return out;
        }
        if !self.error.is_empty() {
            return &self.error;
        }
        let err = self.stderr.trim();
        if err.is_empty() { "unavailable" } else { err }
    }

    /// True when the command ran to completion and exited 0.
    pub fn succeeded(&self) -> bool {
        self.returncode == Some(0)
    }
}

struct Capture<R> {
    stream: R,
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
    eof: bool,
}

enum Drained {
    Idle,
    MorePending,
}

impl<R: Read> Capture<R> {
    /// Reads what is queued, at most 16 chunks per turn so one busy stream
    /// cannot starve the other or the deadline.
    fn drain(&mut self) -> std::io::Result<Drained> {
        let mut chunk = [0_u8; 8192];
        for _ in 0..16 {
            if self.eof {
                return Ok(Drained::Idle);
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(Drained::Idle);
                }
                Ok(n) => {
                    let room = self.limit - self.bytes.len();
                    let kept = n.min(room);
                    self.bytes.extend_from_slice(&chunk[..kept]);
                    self.truncated |= kept < n;
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(Drained::Idle),
                Err(e) => return Err(e),
            }
        }
        Ok(Drained::MorePending)
    }
}

/// Runs a command to completion or its deadline.
pub fn run(o: &Options<'_>) -> Output {
    let started = Instant::now();
    let mut result = Output { command: o.argv.join(" "), ..Output::default() };
    let limit = if o.output_limit == 0 { OUTPUT_LIMIT } else { o.output_limit };
    let Some((program, args)) = o.argv.split_first() else {
        "empty command".clone_into(&mut result.error);
        return result;
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(if o.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(env) = o.env {
        command.env_clear().envs(env.iter().copied());
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            result.error = e.to_string();
            result.duration_seconds = started.elapsed().as_secs_f64();
            return result;
        }
    };
    let deadline = started + Duration::from_millis(o.timeout_ms);
    let outcome = supervise(&mut child, o, limit, deadline);
    let (status, failure, timed_out, out, err) = match outcome {
        Ok(x) => x,
        Err(e) => {
            kill_group(&mut child);
            result.error = e.to_string();
            result.duration_seconds = started.elapsed().as_secs_f64();
            return result;
        }
    };
    if timed_out || failure.is_some() {
        kill_group(&mut child);
    }
    if !timed_out && failure.is_none() {
        result.returncode = status.and_then(|s| s.code());
    }
    result.stdout = String::from_utf8_lossy(&out.bytes).into_owned();
    result.stderr = String::from_utf8_lossy(&err.bytes).into_owned();
    result.stdout_truncated = out.truncated;
    result.stderr_truncated = err.truncated;
    result.timed_out = timed_out;
    result.duration_seconds = started.elapsed().as_secs_f64();
    if timed_out {
        // Exact for every timeout below 2^53 ms, i.e. any that could be configured.
        #[allow(clippy::cast_precision_loss)]
        let seconds = o.timeout_ms as f64 / 1000.0;
        result.error = format!("Command exceeded {seconds} seconds.");
    } else if let Some(e) = failure {
        result.error = e.to_string();
    }
    result
}

type Supervised = (
    Option<std::process::ExitStatus>,
    Option<std::io::Error>,
    bool,
    Capture<std::process::ChildStdout>,
    Capture<std::process::ChildStderr>,
);

fn supervise(
    child: &mut Child,
    o: &Options<'_>,
    limit: usize,
    deadline: Instant,
) -> std::io::Result<Supervised> {
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(std::io::Error::other("child pipes are missing"));
    };
    posix::set_nonblocking(stdout.as_raw_fd())?;
    posix::set_nonblocking(stderr.as_raw_fd())?;
    let mut out =
        Capture { stream: stdout, bytes: Vec::new(), limit, truncated: false, eof: false };
    let mut err =
        Capture { stream: stderr, bytes: Vec::new(), limit, truncated: false, eof: false };
    let mut stdin = child.stdin.take();
    if let Some(s) = &stdin {
        posix::set_nonblocking(s.as_raw_fd())?;
    }
    let input = o.stdin.unwrap_or(&[]);
    let mut written = 0;
    let mut status = None;
    loop {
        let a = out.drain();
        let b = err.drain();
        let (a, b) = match (a, b) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => return Ok((status, Some(e), false, out, err)),
        };
        if let Some(pipe) = stdin.as_mut() {
            if written < input.len() {
                match pipe.write(&input[written..]) {
                    Ok(n) => written += n,
                    Err(e)
                        if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                    Err(_) => written = input.len(),
                }
            }
            if written >= input.len() {
                stdin = None;
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(s) => status = s,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Ok((status, Some(e), false, out, err)),
            }
        }
        if status.is_some() && out.eof && err.eof {
            return Ok((status, None, false, out, err));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok((status, None, true, out, err));
        }
        if matches!((a, b), (Drained::Idle, Drained::Idle)) {
            // Sleep until a pipe has data or 10 ms pass, whichever is first;
            // the short cap is what notices the child's exit.
            let wait = (deadline - now).min(Duration::from_millis(10));
            let wait_ms = i32::try_from(wait.as_millis()).unwrap_or(10);
            let mut fds = Vec::with_capacity(3);
            if !out.eof {
                fds.push(pollfd { fd: out.stream.as_raw_fd(), events: POLLIN, revents: 0 });
            }
            if !err.eof {
                fds.push(pollfd { fd: err.stream.as_raw_fd(), events: POLLIN, revents: 0 });
            }
            if let Some(s) = &stdin {
                fds.push(pollfd { fd: s.as_raw_fd(), events: POLLOUT, revents: 0 });
            }
            if fds.is_empty() {
                std::thread::sleep(wait);
            } else {
                posix::poll_fds(&mut fds, wait_ms)?;
            }
        }
    }
}

/// Kills the child's whole process group, then reaps the child.
fn kill_group(child: &mut Child) {
    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = posix::send_signal(-pid, SIGKILL);
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
    }
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_argv(argv: &[&str], timeout_ms: u64) -> Output {
        run(&Options::new(argv, timeout_ms))
    }

    #[test]
    fn captures_both_streams_and_exit_status() {
        let r = run_argv(&["/bin/sh", "-c", "printf out; printf err >&2; exit 7"], 2000);
        assert_eq!(r.returncode, Some(7));
        assert_eq!(r.stdout, "out");
        assert_eq!(r.stderr, "err");
        assert_eq!(r.error, "");
        assert_eq!(r.command, "/bin/sh -c printf out; printf err >&2; exit 7");
    }

    #[test]
    fn timeout_includes_pipes_inherited_by_descendants() {
        let r = run_argv(&["/bin/sh", "-c", "sleep 3 & exit 0"], 100);
        assert!(r.timed_out);
        assert_eq!(r.returncode, None);
        assert!(r.duration_seconds < 2.0);
        assert_eq!(r.error, "Command exceeded 0.1 seconds.");
    }

    #[test]
    fn timeout_reaps_a_running_child() {
        let r = run_argv(&["/bin/sh", "-c", "exec sleep 3"], 100);
        assert!(r.timed_out);
        assert!(r.duration_seconds < 2.0);
    }

    #[test]
    fn verbose_commands_finish_after_both_capture_limits() {
        let r = run_argv(
            &["/bin/sh", "-c", "head -c 300000 /dev/zero & head -c 300000 /dev/zero >&2 & wait"],
            5000,
        );
        assert_eq!(r.returncode, Some(0));
        assert!(!r.timed_out);
        assert_eq!(r.stdout.len(), OUTPUT_LIMIT);
        assert_eq!(r.stderr.len(), OUTPUT_LIMIT);
        assert!(r.stdout_truncated && r.stderr_truncated);
    }

    #[test]
    fn missing_program_reports_spawn_failure() {
        let r = run_argv(&["/nonexistent/omt-command-test"], 1000);
        assert_eq!(r.returncode, None);
        assert!(!r.error.is_empty());
        assert!(!r.timed_out);
    }

    #[test]
    fn exact_limit_is_not_truncated_and_stdin_is_fed() {
        let r = run_argv(&["/bin/sh", "-c", "head -c 262144 /dev/zero"], 5000);
        assert_eq!(r.stdout.len(), OUTPUT_LIMIT);
        assert!(!r.stdout_truncated);
        let mut o = Options::new(&["/bin/cat"], 2000);
        o.stdin = Some(b"fed\n");
        assert_eq!(run(&o).stdout, "fed\n");
    }

    #[test]
    fn invalid_utf8_is_replaced() {
        let r = run_argv(&["/bin/sh", "-c", "printf 'a\\377b'"], 2000);
        assert_eq!(r.stdout, "a\u{fffd}b");
        assert_eq!(r.report_text(), "a\u{fffd}b");
    }

    #[test]
    fn environment_is_exact_when_given() {
        let mut o = Options::new(&["/usr/bin/env"], 2000);
        o.env = Some(&[("OMT_ONLY", "1")]);
        assert_eq!(run(&o).stdout, "OMT_ONLY=1\n");
    }
}
