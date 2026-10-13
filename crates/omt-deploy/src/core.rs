//! Secrets, settings, validation, and the small helpers the operations
//! share. Validation functions return `None` when a value is acceptable and
//! the message to show the operator when it is not.

use std::fmt;

use omt_common::buf::wipe_string;
use omt_common::{Error, Result};

use crate::sys::{self, PathKind};

pub const MAX_SECRET_BYTES: usize = 4096;
pub const MAX_MANIFEST_MEMBERS: usize = 128;
pub const MAX_MANIFEST_MEMBER_BYTES: usize = 240;
pub const MIN_WEB_PASSWORD_BYTES: usize = 12;
pub const MAX_WEB_PASSWORD_BYTES: usize = 128;
pub const MIN_OS_PASSWORD_BYTES: usize = 8;
pub const MAX_OS_PASSWORD_BYTES: usize = 128;
pub const IMAGE_MEMBER: &str = "omt-client-arm64.tar.gz";

// ----------------------------------------------------------------- secrets

/// Whether text contains a Unicode control character (Cc).
pub fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

/// A credential, wiped when dropped. "Absent" and "explicitly empty" differ:
/// factory Alpine accepts root with an empty SSH password, so "" is a real
/// credential there.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(Option<String>);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "Secret(set)" } else { "Secret(absent)" })
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        if let Some(s) = &mut self.0 {
            wipe_string(s);
        }
    }
}

impl Secret {
    /// Validates (at most 4096 bytes of UTF-8 with no control characters)
    /// and stores a copy.
    pub fn new(text: &str) -> std::result::Result<Self, &'static str> {
        if text.len() > MAX_SECRET_BYTES || has_control(text) {
            return Err("Authentication secret is invalid or exceeds 4096 bytes.");
        }
        Ok(Self(Some(text.to_owned())))
    }

    pub const fn absent() -> Self {
        Self(None)
    }

    pub fn is_set(&self) -> bool {
        self.0.is_some()
    }

    pub fn text(&self) -> &str {
        self.0.as_deref().unwrap_or("")
    }

    pub fn len(&self) -> usize {
        self.text().len()
    }

    pub fn is_empty(&self) -> bool {
        self.text().is_empty()
    }

    pub fn nonempty(&self) -> bool {
        !self.text().is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self(None);
    }
}

// ---------------------------------------------------------------- settings

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Auth {
    #[default]
    Password,
    Key,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Status,
    Logs,
    Restart,
    Reboot,
}

impl Action {
    /// The fixed argv each management action runs. Every token is fixed;
    /// no form value can select a command or argument across this privilege
    /// boundary.
    pub fn argv(self) -> &'static [&'static str] {
        match self {
            Action::Status => &["docker", "ps", "--filter", "name=omt-client"],
            Action::Logs => &["docker", "logs", "--tail", "500", "omt-client"],
            // Manage the OpenRC service rather than assuming its container
            // already exists: a clean install defers first startup until
            // reboot, and rc-service can both start that state and restart
            // an existing container.
            Action::Restart => &["rc-service", "omt-client", "restart"],
            // Return a successful SSH exit status before the kernel tears the
            // connection down.
            Action::Reboot => {
                &["sh", "-c", "nohup sh -c 'sleep 1; /sbin/reboot' </dev/null >/dev/null 2>&1 &"]
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Connection {
    pub host: String,
    pub username: String,
    pub port: u16,
    pub auth: Auth,
    pub password: Secret,
    pub key_path: Option<String>,
    pub key_passphrase: Secret,
    /// `None` uses ~/.ssh/known_hosts.
    pub known_hosts_path: Option<String>,
    pub sudo_password: Secret,
    /// Root password used only to bootstrap untouched Alpine through `su`.
    pub bootstrap_root_password: Secret,
}

impl Connection {
    pub fn new() -> Self {
        Self { port: 22, ..Self::default() }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeployOptions {
    /// `None` deploys the embedded capsule.
    pub project_root: Option<String>,
    pub remote_directory: String,
    pub rebuild_image: bool,
}

#[derive(Clone, Debug, Default)]
pub struct WifiSettings {
    pub ssid: String,
    pub password: Secret,
    pub connect: bool,
    pub preserve_existing_profiles: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AlpineSettings {
    pub hostname: String,
    pub wifi: Option<WifiSettings>,
    pub root_password: Secret,
    pub pi_password: Secret,
}

#[derive(Clone, Debug, Default)]
pub struct SdSettings {
    pub boot_directory: String,
    pub country: String,
    pub wifi_ssid: String,
    pub wifi_password: Secret,
}

// -------------------------------------------------------------- validation

fn ascii_token(value: &str, extra: &[u8]) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

pub fn valid_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && ascii_token(label, b"-")
        })
}

pub fn valid_username(value: &str) -> bool {
    value.len() <= 64 && ascii_token(value, b"._-")
}

pub fn valid_appliance_hostname(value: &str) -> bool {
    (1..=63).contains(&value.len())
        && !value.starts_with('-')
        && !value.ends_with('-')
        && ascii_token(value, b"-")
}

pub fn valid_manifest_name(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_MANIFEST_MEMBER_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
        || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_/".contains(&b))
    {
        return false;
    }
    value.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

pub fn valid_remote_directory(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= MAX_MANIFEST_MEMBER_BYTES
        && value.starts_with('/')
        && !value.ends_with('/')
        && valid_manifest_name(&value[1..])
}

pub fn validate_connection(c: &Connection) -> Option<&'static str> {
    if !valid_host(&c.host) {
        return Some("Pi host must be a valid IPv4 address or DNS host name.");
    }
    if !valid_username(&c.username) {
        return Some("SSH username contains invalid characters.");
    }
    if c.port == 0 {
        return Some("SSH port must be between 1 and 65535.");
    }
    if c.known_hosts_path.as_deref().is_some_and(|p| !sys::is_file(p)) {
        return Some("OpenSSH known_hosts file does not exist.");
    }
    if c.auth == Auth::Password && !c.password.is_set() {
        return Some("SSH password is required for password authentication.");
    }
    if c.auth == Auth::Key && !c.key_path.as_deref().is_some_and(sys::is_file) {
        return Some("SSH private-key file does not exist.");
    }
    None
}

pub fn validate_options(o: &DeployOptions) -> Option<&'static str> {
    if o.project_root.as_deref().is_some_and(|p| !sys::is_dir(p)) {
        return Some("Project root does not exist.");
    }
    if o.project_root.is_none() && o.rebuild_image {
        return Some("Rebuilding the appliance image needs a project root to build it from.");
    }
    if !valid_remote_directory(&o.remote_directory) {
        return Some("Remote install directory is not a normalized safe absolute path.");
    }
    None
}

fn is_hex_psk(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn validate_wifi(w: &WifiSettings) -> Option<&'static str> {
    if w.ssid.is_empty() || w.ssid.len() > 32 || has_control(&w.ssid) {
        return Some("Wi-Fi SSID must contain 1-32 UTF-8 bytes and no control characters.");
    }
    let password = w.password.text();
    let printable =
        (8..=63).contains(&password.len()) && password.bytes().all(|b| (0x20..=0x7e).contains(&b));
    if !is_hex_psk(password) && !printable {
        return Some(
            "Wi-Fi password must be 8-63 printable ASCII characters or a 64-digit hex PSK.",
        );
    }
    None
}

pub fn validate_os_password(s: &Secret) -> Option<&'static str> {
    if !(MIN_OS_PASSWORD_BYTES..=MAX_OS_PASSWORD_BYTES).contains(&s.len()) {
        return Some("Host password must contain 8-128 UTF-8 bytes.");
    }
    if has_control(s.text()) {
        return Some("Host password must not contain control characters.");
    }
    None
}

pub fn validate_alpine_setup(a: &AlpineSettings) -> Option<&'static str> {
    if !valid_appliance_hostname(&a.hostname) {
        return Some("Hostname must be a single DNS label of 1-63 letters, digits, or hyphens.");
    }
    validate_os_password(&a.root_password)
        .or_else(|| validate_os_password(&a.pi_password))
        .or_else(|| a.wifi.as_ref().and_then(validate_wifi))
}

pub fn validate_web_password(s: &Secret) -> Option<&'static str> {
    if !(MIN_WEB_PASSWORD_BYTES..=MAX_WEB_PASSWORD_BYTES).contains(&s.len()) {
        return Some("Web GUI password must contain 12-128 UTF-8 bytes.");
    }
    if has_control(s.text()) {
        return Some("Web GUI password must not contain control characters.");
    }
    None
}

// ----------------------------------------------------------------- helpers

/// The 64-hex-digit WPA PSK for a passphrase.
pub fn derive_wpa_psk(
    ssid: &str,
    passphrase: &Secret,
) -> std::result::Result<Secret, &'static str> {
    if ssid.is_empty() || ssid.len() > 32 || !(8..=63).contains(&passphrase.len()) {
        return Err("Invalid WPA credentials.");
    }
    let mut derived = [0_u8; 32];
    if omt_crypto::pbkdf2_sha1(passphrase.text().as_bytes(), ssid.as_bytes(), 4096, &mut derived)
        .is_err()
    {
        omt_crypto::cleanse(&mut derived);
        return Err("WPA derivation failed.");
    }
    let mut encoded = omt_common::buf::hex(&derived);
    omt_crypto::cleanse(&mut derived);
    let secret = Secret::new(&encoded);
    wipe_string(&mut encoded);
    secret
}

/// `value` in single quotes, safe for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `bytes` random bytes as hex, from OpenSSL's CSPRNG (the same source on
/// Linux and Windows).
pub fn random_token(bytes: usize) -> Result<String> {
    if bytes == 0 || bytes > 64 {
        return Err(Error::new("invalid token size"));
    }
    let mut raw = vec![0_u8; bytes];
    if !omt_sys::evp::safe::random_bytes(&mut raw) {
        return Err(Error::new("randomness is unavailable"));
    }
    Ok(omt_common::buf::hex(&raw))
}

pub fn sha256_hex(data: &[u8]) -> String {
    omt_crypto::sha256(data).map_or_else(|_| "0".repeat(64), |d| omt_common::buf::hex(&d))
}

pub fn sha256_file(path: &str) -> Result<String> {
    let mut f = sys::open_read(path)?;
    let mut ctx = omt_crypto::Sha256::new().map_err(|_| Error::new("SHA-256 is unavailable"))?;
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let n = sys::read_some(&mut f, &mut chunk)?;
        if n == 0 {
            break;
        }
        ctx.update(&chunk[..n]).map_err(|_| Error::new(format!("cannot hash {path}")))?;
    }
    let digest = ctx.finish().map_err(|_| Error::new(format!("cannot hash {path}")))?;
    Ok(omt_common::buf::hex(&digest))
}

// ---------------------------------------------------------------- manifest

/// Parses a v3 manifest: its members, in order.
pub fn parse_manifest(text: &str) -> Result<Vec<String>> {
    // Lines as str::lines splits them: on \n, with one trailing \r removed,
    // and no empty final line after a terminating newline.
    let mut lines = text.lines();
    if lines.next() != Some("version=3") {
        return Err(Error::new("unsupported manifest"));
    }
    let mut names: Vec<String> = Vec::new();
    for line in lines {
        if names.len() == MAX_MANIFEST_MEMBERS
            || line.contains('\0')
            || !valid_manifest_name(line)
            || names.iter().any(|n| n == line)
        {
            return Err(Error::new("unsafe or duplicate manifest member"));
        }
        names.push(line.to_owned());
    }
    if !names.iter().any(|n| n == "deploy/transaction.sh")
        || !names.iter().any(|n| n == "deploy/manifest-v3.txt")
    {
        return Err(Error::new("manifest is missing transaction members"));
    }
    Ok(names)
}

pub fn load_manifest(path: &str) -> Result<Vec<String>> {
    match sys::path_lstat(path) {
        PathKind::Missing => {
            return Err(Error::new(format!("{path}: No such file or directory (os error 2)")));
        }
        PathKind::File if sys::file_size(path).is_some_and(|s| s <= 32 * 1024) => {}
        _ => return Err(Error::new("manifest is not a bounded regular file")),
    }
    let bytes = sys::read_file(path, 32 * 1024)?;
    let text =
        String::from_utf8(bytes).map_err(|_| Error::new("stream did not contain valid UTF-8"))?;
    parse_manifest(&text)
}

/// `root` joined with a member, after re-checking the member is safe.
pub fn secure_relative(root: &str, member: &str) -> Result<String> {
    if !valid_manifest_name(member) {
        return Err(Error::new("unsafe member"));
    }
    if sys::ON_WINDOWS {
        // Path::join on Windows takes the member's slashes as separators too.
        let sep = if root.is_empty() || root.ends_with(['/', '\\']) { "" } else { "\\" };
        Ok(format!("{root}{sep}{}", member.replace('/', "\\")))
    } else {
        Ok(sys::path_join(root, member))
    }
}

// ---------------------------------------------------------------- progress

/// Where operations report progress, one line at a time.
pub type Progress<'a> = &'a (dyn Fn(&str) + Sync);

/// Reports text, which may span several lines: one callback per non-empty
/// line, a trailing carriage return removed.
pub fn report(p: Option<Progress<'_>>, text: &str) {
    let Some(p) = p else { return };
    for line in text.split('\n') {
        let visible = line.strip_suffix('\r').unwrap_or(line);
        if !visible.is_empty() {
            p(visible);
        }
    }
}
