use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde::Serialize;
use std::{
    io::{self, Read},
    os::fd::AsFd,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 256 * 1024;

#[derive(Clone, Debug, Default, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct CommandResult {
    pub command: String,
    pub returncode: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub duration_seconds: f64,
    pub timed_out: bool,
    pub error: String,
    pub skipped: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub sources: Vec<String>,
}

impl CommandResult {
    pub fn failure_detail(&self) -> &str {
        if !self.error.is_empty() {
            &self.error
        } else if !self.stderr.trim().is_empty() {
            self.stderr.trim()
        } else {
            self.stdout.trim()
        }
    }

    pub fn report_text(&self) -> String {
        if !self.stdout.trim().is_empty() {
            self.stdout.trim().to_owned()
        } else if !self.error.is_empty() {
            self.error.clone()
        } else if !self.stderr.trim().is_empty() {
            self.stderr.trim().to_owned()
        } else {
            "unavailable".to_owned()
        }
    }
}

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
    eof: bool,
}

fn nonblocking(stream: &impl AsFd) -> io::Result<()> {
    let flags = fcntl(stream, FcntlArg::F_GETFL)?;
    fcntl(
        stream,
        FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
    )?;
    Ok(())
}

impl Capture {
    /// Reads what is queued, and reports whether the turn budget ran out with
    /// the pipe still readable.
    fn drain(&mut self, reader: &mut impl Read) -> io::Result<bool> {
        let mut chunk = [0_u8; 8192];
        // Bound each turn so continuous output cannot starve the other pipe or
        // the deadline. Continue consuming after the capture limit is reached.
        for _ in 0..16 {
            if self.eof {
                return Ok(false);
            }
            match reader.read(&mut chunk) {
                Ok(0) => {
                    self.eof = true;
                    return Ok(false);
                }
                Ok(read) => {
                    let kept = read.min(OUTPUT_LIMIT.saturating_sub(self.bytes.len()));
                    self.bytes.extend_from_slice(&chunk[..kept]);
                    self.truncated |= kept < read;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}

pub fn run(program: &Path, arguments: &[&str], timeout: Duration) -> CommandResult {
    let started = Instant::now();
    let command_text = std::iter::once(program.to_string_lossy().into_owned())
        .chain(arguments.iter().map(|value| (*value).to_owned()))
        .collect::<Vec<_>>()
        .join(" ");
    let mut child = match Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
    {
        Ok(value) => value,
        Err(error) => {
            return CommandResult {
                command: command_text,
                duration_seconds: started.elapsed().as_secs_f64(),
                error: error.to_string(),
                ..CommandResult::default()
            };
        }
    };
    let mut stdout = Capture::default();
    let mut stderr = Capture::default();
    let deadline = started + timeout;
    let mut status = None;
    let mut timed_out = false;
    let outcome = (|| -> io::Result<()> {
        let mut out = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing stdout"))?;
        let mut err = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("missing stderr"))?;
        nonblocking(&out)?;
        nonblocking(&err)?;
        loop {
            let stdout_pending = stdout.drain(&mut out)?;
            let stderr_pending = stderr.drain(&mut err)?;
            if status.is_none() {
                status = child.try_wait()?;
            }
            if status.is_some() && stdout.eof && stderr.eof {
                return Ok(());
            }
            if Instant::now() >= deadline {
                timed_out = true;
                return Ok(());
            }
            // Only idle when both pipes ran dry; a turn that hit its budget
            // has more waiting, and sleeping on it would cap capture
            // throughput far below what the pipes deliver.
            if !stdout_pending && !stderr_pending {
                thread::sleep(Duration::from_millis(10));
            }
        }
    })();
    if timed_out || outcome.is_err() {
        if let Ok(pid) = i32::try_from(child.id()) {
            let _ignored = killpg(Pid::from_raw(pid), Signal::SIGKILL);
        }
        if status.is_none() {
            let _ignored = child.kill();
            status = child.wait().ok();
        }
    }
    CommandResult {
        command: command_text,
        // Callers treat exit zero as success; incomplete output must not look
        // successful just because the direct child exited before its pipes.
        returncode: status
            .filter(|_| !timed_out && outcome.is_ok())
            .and_then(|value| value.code()),
        stdout: String::from_utf8_lossy(&stdout.bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
        duration_seconds: started.elapsed().as_secs_f64(),
        timed_out,
        error: if timed_out {
            format!("Command exceeded {} seconds.", timeout.as_secs_f64())
        } else {
            outcome
                .err()
                .map_or_else(String::new, |error| error.to_string())
        },
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        ..CommandResult::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_both_streams_and_exit_status() {
        let result = run(
            Path::new("/bin/sh"),
            &["-c", "printf out; printf err >&2; exit 7"],
            Duration::from_secs(2),
        );
        assert_eq!(result.returncode, Some(7));
        assert_eq!(result.stdout, "out");
        assert_eq!(result.stderr, "err");
        assert!(result.error.is_empty());
    }

    #[test]
    fn timeout_includes_pipes_inherited_by_descendants() {
        let result = run(
            Path::new("/bin/sh"),
            &["-c", "sleep 3 & exit 0"],
            Duration::from_millis(100),
        );
        assert!(result.timed_out);
        assert!(result.returncode.is_none());
        assert!(result.duration_seconds < 2.0);
    }

    #[test]
    fn timeout_reaps_a_running_child() {
        let result = run(
            Path::new("/bin/sh"),
            &["-c", "exec sleep 3"],
            Duration::from_millis(100),
        );
        assert!(result.timed_out);
        assert!(result.duration_seconds < 2.0);
    }

    #[test]
    fn verbose_commands_finish_after_both_capture_limits() {
        let result = run(
            Path::new("/bin/sh"),
            &[
                "-c",
                "head -c 300000 /dev/zero & head -c 300000 /dev/zero >&2 & wait",
            ],
            Duration::from_secs(5),
        );
        assert_eq!(result.returncode, Some(0));
        assert!(!result.timed_out);
        assert_eq!(result.stdout.len(), OUTPUT_LIMIT);
        assert_eq!(result.stderr.len(), OUTPUT_LIMIT);
        assert!(result.stdout_truncated && result.stderr_truncated);
    }

    #[test]
    fn missing_program_reports_spawn_failure() {
        let result = run(
            Path::new("/nonexistent/omt-command-test"),
            &[],
            Duration::from_secs(1),
        );
        assert!(result.returncode.is_none());
        assert!(!result.error.is_empty());
        assert!(!result.timed_out);
    }

    #[test]
    fn interrupted_reads_are_retried_and_exact_limit_is_not_truncated() {
        struct InterruptedOnce(bool, io::Cursor<Vec<u8>>);
        impl Read for InterruptedOnce {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.0 {
                    self.0 = true;
                    return Err(io::ErrorKind::Interrupted.into());
                }
                self.1.read(buffer)
            }
        }
        let mut reader = InterruptedOnce(false, io::Cursor::new(vec![42; OUTPUT_LIMIT]));
        let mut capture = Capture::default();
        while !capture.eof {
            let _pending = capture
                .drain(&mut reader)
                .unwrap_or_else(|error| panic!("{error}"));
        }
        assert_eq!(capture.bytes, vec![42; OUTPUT_LIMIT]);
        assert!(!capture.truncated);
    }

    #[test]
    fn an_exhausted_turn_budget_reports_more_output_pending() {
        let mut reader = io::Cursor::new(vec![42; 8192 * 17]);
        let mut capture = Capture::default();
        assert!(
            capture
                .drain(&mut reader)
                .unwrap_or_else(|error| panic!("{error}"))
        );
        assert_eq!(capture.bytes.len(), 8192 * 16);
        assert!(!capture.eof);
        // The tail still arrives on the next turn, which then reaches the end.
        assert!(
            !capture
                .drain(&mut reader)
                .unwrap_or_else(|error| panic!("{error}"))
        );
        assert_eq!(capture.bytes.len(), 8192 * 17);
        assert!(capture.eof);
    }
}
