#![forbid(unsafe_code)]

mod ops;
mod ssh;
mod tools;

pub use ops::{apply_wifi, connect, deploy, manage, test_connection};
pub use ssh::{RemoteResult, SshSession};
pub use tools::{
    BuildPlan, DOCKER_DESKTOP, GIT_FOR_WINDOWS, ON_WINDOWS, PYTHON, Package, Prerequisite,
    ensure_arm64_emulation, find_bash, find_container_engine, find_executable, image_build_plan,
    install_packages, missing_packages, prerequisites,
};

use hmac::Hmac;
use pbkdf2::pbkdf2;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use zeroize::{Zeroize, Zeroizing};

pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
pub const MAX_SECRET_BYTES: usize = 4096;
pub const MAX_MANIFEST_MEMBERS: usize = 128;
pub const MAX_MANIFEST_MEMBER_BYTES: usize = 240;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum AuthMethod {
    Password,
    Key,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagementAction {
    Status,
    Logs,
    Restart,
    Reboot,
}
impl ManagementAction {
    pub const fn remote_argv(self) -> &'static [&'static str] {
        match self {
            Self::Status => &["docker", "ps", "--filter", "name=omt-client"],
            Self::Logs => &["docker", "logs", "--tail", "500", "omt-client"],
            // Manage the OpenRC service rather than assuming its container
            // already exists. A clean install deliberately defers first
            // startup until reboot, and rc-service can both start that state
            // and restart an existing container.
            Self::Restart => &["rc-service", "omt-client", "restart"],
            // Return a successful SSH exit status before the kernel tears the
            // connection down. Every token remains fixed; no form value can
            // select a command or argument across this privilege boundary.
            Self::Reboot => &[
                "sh",
                "-c",
                "nohup sh -c 'sleep 1; /sbin/reboot' </dev/null >/dev/null 2>&1 &",
            ],
        }
    }
}

#[derive(Default, Zeroize)]
#[zeroize(drop)]
pub struct Secret(Zeroizing<String>);
impl Secret {
    pub fn new(value: String) -> Result<Self, ValidationError> {
        if value.len() > MAX_SECRET_BYTES || value.chars().any(char::is_control) {
            return Err(ValidationError(
                "Authentication secret is invalid or exceeds 4096 bytes.",
            ));
        }
        Ok(Self(Zeroizing::new(value)))
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

pub struct Connection {
    pub host: String,
    pub username: String,
    pub port: u16,
    pub auth: AuthMethod,
    pub password: Option<Secret>,
    pub key_path: Option<PathBuf>,
    pub key_passphrase: Option<Secret>,
    pub known_hosts_path: Option<PathBuf>,
    pub sudo_password: Option<Secret>,
    /// Root password used only to bootstrap untouched Alpine through `su`.
    /// Once sudo exists, privileged operations use `sudo_password` instead.
    pub bootstrap_root_password: Option<Secret>,
}
#[derive(Clone, Debug)]
pub struct DeployOptions {
    pub project_root: PathBuf,
    pub remote_directory: String,
    pub tarball_name: String,
    pub build_image: bool,
}
pub struct WifiSettings {
    pub ssid: String,
    pub password: Secret,
    pub connect: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidationError(pub &'static str);
impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ValidationError {}

fn ascii_token(value: &str, extra: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.as_bytes().contains(&b))
}
pub fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && ascii_token(label, "-")
        })
}
pub fn valid_username(value: &str) -> bool {
    value.len() <= 64 && ascii_token(value, "._-")
}
pub fn valid_remote_directory(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= MAX_MANIFEST_MEMBER_BYTES
        && value.starts_with('/')
        && !value.ends_with('/')
        && valid_manifest_name(&value[1..])
}
pub fn valid_manifest_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MANIFEST_MEMBER_BYTES
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains("//")
        && value.is_ascii()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}
pub fn validate_connection(value: &Connection) -> Result<(), ValidationError> {
    if !valid_host(&value.host) {
        return Err(ValidationError(
            "Pi host must be a valid IPv4 address or DNS host name.",
        ));
    }
    if !valid_username(&value.username) {
        return Err(ValidationError("SSH username contains invalid characters."));
    }
    if value.port == 0 {
        return Err(ValidationError("SSH port must be between 1 and 65535."));
    }
    if value
        .known_hosts_path
        .as_ref()
        .is_some_and(|path| !path.is_file())
    {
        return Err(ValidationError("OpenSSH known_hosts file does not exist."));
    }
    match value.auth {
        AuthMethod::Password
            if value
                .password
                .as_ref()
                .is_none_or(|v| v.expose().is_empty()) =>
        {
            return Err(ValidationError(
                "SSH password is required for password authentication.",
            ));
        }
        AuthMethod::Key if value.key_path.as_ref().is_none_or(|v| !v.is_file()) => {
            return Err(ValidationError("SSH private-key file does not exist."));
        }
        _ => {}
    }
    Ok(())
}
pub fn validate_options(
    value: &DeployOptions,
    require_project: bool,
) -> Result<(), ValidationError> {
    if require_project && !value.project_root.is_dir() {
        return Err(ValidationError("Project root does not exist."));
    }
    if !valid_remote_directory(&value.remote_directory) {
        return Err(ValidationError(
            "Remote install directory is not a normalized safe absolute path.",
        ));
    }
    if !ascii_token(&value.tarball_name, "._-") {
        return Err(ValidationError("Archive name contains unsafe characters."));
    }
    Ok(())
}
pub fn validate_wifi(value: &WifiSettings) -> Result<(), ValidationError> {
    if value.ssid.is_empty() || value.ssid.len() > 32 || value.ssid.chars().any(char::is_control) {
        return Err(ValidationError(
            "Wi-Fi SSID must contain 1-32 UTF-8 bytes and no control characters.",
        ));
    }
    let password = value.password.expose();
    let hex = password.len() == 64 && password.bytes().all(|b| b.is_ascii_hexdigit());
    if !(hex
        || (8..=63).contains(&password.len())
            && password.bytes().all(|b| (0x20..=0x7e).contains(&b)))
    {
        return Err(ValidationError(
            "Wi-Fi password must be 8-63 printable ASCII characters or a 64-digit hex PSK.",
        ));
    }
    Ok(())
}
pub fn derive_wpa_psk(ssid: &str, passphrase: &Secret) -> Result<Secret, ValidationError> {
    if ssid.is_empty() || ssid.len() > 32 || !(8..=63).contains(&passphrase.expose().len()) {
        return Err(ValidationError("Invalid WPA credentials."));
    }
    let mut derived = [0_u8; 32];
    pbkdf2::<Hmac<Sha1>>(
        passphrase.expose().as_bytes(),
        ssid.as_bytes(),
        4096,
        &mut derived,
    )
    .map_err(|_| ValidationError("WPA derivation failed."))?;
    let encoded = hex_encode(&derived);
    derived.zeroize();
    Secret::new(encoded)
}
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Lower-case hex, in one allocation.
///
/// The three callers (`derive_wpa_psk`, `random_token`, and the Wi-Fi SSID
/// encoder) each open-coded `.map(|b| format!("{b:02x}")).collect()`, which
/// allocates a `String` per byte and then throws all of them away.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

pub fn load_manifest(path: &Path) -> io::Result<Vec<String>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > 32 * 1024
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "manifest is not a bounded regular file",
        ));
    }
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    if lines.next() != Some("version=3") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported manifest",
        ));
    }
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for line in lines {
        if result.len() == MAX_MANIFEST_MEMBERS || !valid_manifest_name(line) || !seen.insert(line)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsafe or duplicate manifest member",
            ));
        }
        result.try_reserve(1).map_err(io::Error::other)?;
        result.push(line.to_owned());
    }
    if !seen.contains("deploy/transaction.sh") || !seen.contains("deploy/manifest-v3.txt") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "manifest is missing transaction members",
        ));
    }
    Ok(result)
}
pub fn discover_project_root(starts: &[PathBuf]) -> Option<PathBuf> {
    starts.iter().find_map(|start| {
        let mut current = start.as_path();
        for _ in 0..=8 {
            if current.join("deploy/manifest-v3.txt").is_file() {
                return Some(current.to_path_buf());
            }
            current = current.parent()?;
        }
        None
    })
}
pub fn secure_relative(root: &Path, member: &str) -> io::Result<PathBuf> {
    if !valid_manifest_name(member)
        || Path::new(member)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsafe member"));
    }
    Ok(root.join(member))
}
pub fn random_token(bytes: usize) -> io::Result<String> {
    if bytes == 0 || bytes > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid token size",
        ));
    }
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(hex_encode(&value))
}
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[derive(Debug)]
pub struct ProcessResult {
    pub exit_code: i32,
    pub output: String,
}
/// Run `program` to completion, streaming both its output streams into a
/// bounded buffer and killing it if `cancelled` is set.
///
/// A failed spawn is reported with the program in the message. `ErrorKind`'s
/// own text is "program not found", which on a workstation missing one of
/// several possible build tools names neither the tool nor the remedy.
pub fn run_process(
    program: &Path,
    args: &[String],
    directory: &Path,
    env: &[(String, String)],
    cancelled: Arc<AtomicBool>,
) -> io::Result<ProcessResult> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot run {}: {error}", program.display()),
        )
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing stderr"))?;
    let (tx, rx) = mpsc::channel();
    spawn_reader(stdout, tx.clone());
    spawn_reader(stderr, tx.clone());
    drop(tx);
    loop {
        if cancelled.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "operation cancelled",
            ));
        }
        if let Some(status) = child.try_wait()? {
            let mut output = Vec::new();
            for part in rx {
                output.extend_from_slice(
                    &part[..part.len().min(OUTPUT_LIMIT.saturating_sub(output.len()))],
                );
            }
            return Ok(ProcessResult {
                exit_code: status.code().unwrap_or(1),
                output: String::from_utf8_lossy(&output).into_owned(),
            });
        }
        thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn spawn_reader(mut input: impl Read + Send + 'static, tx: mpsc::Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut bounded = Vec::new();
        let mut chunk = [0_u8; 8192];
        while bounded.len() < OUTPUT_LIMIT {
            match input.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => bounded.extend_from_slice(&chunk[..n.min(OUTPUT_LIMIT - bounded.len())]),
            }
        }
        let _ = tx.send(bounded);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validation_contract() {
        assert!(valid_host("pi.local"));
        assert!(!valid_host("-pi.local"));
        assert!(valid_username("pi_admin-1"));
        assert!(valid_remote_directory("/opt/omt-client"));
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
    #[test]
    fn restart_manages_the_service_even_before_first_container_creation() {
        assert_eq!(
            ManagementAction::Restart.remote_argv(),
            &["rc-service", "omt-client", "restart"]
        );
        assert!(!ManagementAction::Restart.remote_argv().contains(&"docker"));
    }
    #[test]
    fn reboot_is_a_fixed_deferred_host_action() {
        assert_eq!(
            ManagementAction::Reboot.remote_argv(),
            &[
                "sh",
                "-c",
                "nohup sh -c 'sleep 1; /sbin/reboot' </dev/null >/dev/null 2>&1 &"
            ]
        );
    }
    /// One encoder now serves the PSK, the staging token, and the SSID, so its
    /// output has to stay byte-for-byte what each of those callers published
    /// before: lower case, two digits per byte, no separators.
    #[test]
    fn hex_is_lower_case_and_fixed_width() {
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex_encode(b"studio"), "73747564696f");
        let all: String = hex_encode(&(0..=255).collect::<Vec<u8>>());
        assert_eq!(all.len(), 512);
        assert!(
            all.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn psk_vector() {
        let p = Secret::new("password".into()).unwrap_or_else(|e| panic!("{e}"));
        let derived = derive_wpa_psk("IEEE", &p).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            derived.expose(),
            "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"
        );
    }
    #[test]
    fn manifest_requires_transaction_members() {
        let dir = std::env::temp_dir().join(format!("omt-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        let path = dir.join("manifest-v3.txt");
        fs::write(&path, "version=3\nLICENSE\n").unwrap_or_else(|e| panic!("{e}"));
        assert!(load_manifest(&path).is_err());
        fs::write(
            &path,
            "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let members = load_manifest(&path).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(members.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
