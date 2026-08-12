use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use serde::Serialize;
use std::{
    io::Read,
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

fn drain<R: Read>(mut reader: R) -> (Vec<u8>, bool) {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 65_536];
    let mut truncated = false;
    while let Ok(read) = reader.read(&mut chunk) {
        if read == 0 {
            break;
        }
        let room = OUTPUT_LIMIT.saturating_sub(output.len());
        if read > room {
            output.extend_from_slice(&chunk[..room]);
            truncated = true;
        } else if room > 0 {
            output.extend_from_slice(&chunk[..read]);
        } else {
            truncated = true;
        }
    }
    (output, truncated)
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
    let stdout = child
        .stdout
        .take()
        .map(|stream| thread::spawn(move || drain(stream)));
    let stderr = child
        .stderr
        .take()
        .map(|stream| thread::spawn(move || drain(stream)));
    let deadline = started + timeout;
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let pid = i32::try_from(child.id()).unwrap_or(i32::MAX);
                let _ignored = killpg(Pid::from_raw(pid), Signal::SIGKILL);
                break (child.wait().ok(), true);
            }
            Err(error) => {
                let _ignored = child.kill();
                return CommandResult {
                    command: command_text,
                    duration_seconds: started.elapsed().as_secs_f64(),
                    error: error.to_string(),
                    ..CommandResult::default()
                };
            }
        }
    };
    let (stdout, stdout_truncated) = stdout
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    let (stderr, stderr_truncated) = stderr
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    CommandResult {
        command: command_text,
        returncode: status.and_then(|value| value.code()),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        duration_seconds: started.elapsed().as_secs_f64(),
        timed_out,
        error: if timed_out {
            format!("Command exceeded {} seconds.", timeout.as_secs_f64())
        } else {
            String::new()
        },
        stdout_truncated,
        stderr_truncated,
        ..CommandResult::default()
    }
}
