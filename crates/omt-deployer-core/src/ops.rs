//! Deployment, management, and Wi-Fi operations over [`crate::ssh::SshSession`].

use crate::ssh::{RemoteResult, SshSession};
use crate::{
    AlpineSetupSettings, Connection, DeployOptions, ManagementAction, ON_WINDOWS, Secret,
    WifiSettings, derive_wpa_psk, ensure_arm64_emulation, hex_encode, image_build_plan,
    load_manifest, random_token, run_process, secure_relative, sha256_file, shell_quote,
    validate_alpine_setup, validate_connection, validate_options, validate_web_password,
    validate_wifi,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use zeroize::Zeroizing;

const PLATFORM_PROBE: &str = "uname -m && . /etc/os-release && printf '%s\\n' \"$ID\" && cat /etc/alpine-release && tr -d '\\000' < /proc/device-tree/model && printf '\\n'";
const BOOTSTRAP_PASSWORD_READY: &str = "omt-bootstrap-password-ready";
const SETUP_SYS_MEMBER: &str = "deploy/host/setup-sys.sh";
const SETUP_SYS_COMPLETE: &str = "=== Alpine sys install complete ===";
const WEB_PASSWORD_COMMAND: &str = "sh -eu -c 'docker exec -i omt-client /usr/local/bin/omt-web set-password && rc-service omt-client restart'";

const WIFI_SCRIPT: &str = concat!(
    "marker=$3\n",
    "found_marker=no\n",
    "while IFS= read -r line; do\n",
    "  if [ \"$line\" = \"$marker\" ]; then found_marker=yes; break; fi\n",
    "done\n",
    "if [ \"$found_marker\" != yes ]; then echo \"Wi-Fi password marker not found\" >&2; exit 11; fi\n",
    "if ! IFS= read -r wifi_password; then echo \"Wi-Fi password not provided\" >&2; exit 11; fi\n",
    "ssid_hex=$1\n",
    "activate=$2\n",
    "command -v wpa_cli >/dev/null 2>&1 || { echo 'wpa_cli is unavailable' >&2; exit 12; }\n",
    "iface=\n",
    "if command -v iw >/dev/null 2>&1; then\n",
    "  iface=$(iw dev 2>/dev/null | awk '$1 == \"Interface\" { print $2; exit }')\n",
    "fi\n",
    "if [ -z \"$iface\" ]; then\n",
    "  for path in /sys/class/net/*/wireless; do\n",
    "    [ -e \"$path\" ] || continue\n",
    "    iface=${path#/sys/class/net/}\n",
    "    iface=${iface%/wireless}\n",
    "    break\n",
    "  done\n",
    "fi\n",
    "[ -n \"$iface\" ] || iface=wlan0\n",
    "wpa_cli -i \"$iface\" ping | grep -Fxq PONG || { echo \"wpa_supplicant is unavailable on $iface\" >&2; exit 12; }\n",
    "wpa_cli -i \"$iface\" scan >/dev/null || true\n",
    "network_id=\n",
    "for candidate in $(wpa_cli -i \"$iface\" list_networks | awk 'NR > 1 {print $1}'); do\n",
    "  current=$(wpa_cli -i \"$iface\" get_network \"$candidate\" ssid 2>/dev/null || true)\n",
    "  if [ \"$current\" = \"$ssid_hex\" ]; then network_id=$candidate; break; fi\n",
    "done\n",
    "if [ -z \"$network_id\" ]; then network_id=$(wpa_cli -i \"$iface\" add_network); fi\n",
    "case \"$network_id\" in ''|*[!0-9]*) echo 'Unable to allocate Wi-Fi profile' >&2; exit 13;; esac\n",
    "wpa_cli -i \"$iface\" set_network \"$network_id\" ssid \"$ssid_hex\" | grep -Fxq OK\n",
    "wpa_cli -i \"$iface\" set_network \"$network_id\" key_mgmt WPA-PSK | grep -Fxq OK\n",
    "wpa_cli -i \"$iface\" set_network \"$network_id\" psk \"$wifi_password\" | grep -Fxq OK\n",
    "unset wifi_password\n",
    "wpa_cli -i \"$iface\" enable_network \"$network_id\" | grep -Fxq OK\n",
    "wpa_cli -i \"$iface\" save_config | grep -Fxq OK\n",
    "if [ \"$activate\" = yes ]; then\n",
    "  wpa_cli -i \"$iface\" select_network \"$network_id\" >/dev/null\n",
    "  wpa_cli -i \"$iface\" reassociate >/dev/null\n",
    "fi\n",
    "command -v iw >/dev/null 2>&1 && iw dev \"$iface\" set power_save off || true\n"
);

#[derive(Clone, Debug)]
struct ArtifactIdentity {
    digest: String,
    fingerprint: String,
}

fn map_validation(error: crate::ValidationError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error.0)
}

fn cancelled(flag: &AtomicBool) -> io::Result<()> {
    if flag.load(Ordering::Relaxed) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "operation cancelled",
        ))
    } else {
        Ok(())
    }
}

fn require_success(result: &RemoteResult, operation: &str) -> io::Result<()> {
    if result.is_success() {
        return Ok(());
    }
    let detail = result.combined();
    Err(io::Error::other(format!(
        "{operation} failed{}",
        if detail.trim().is_empty() {
            String::new()
        } else {
            format!(":\n{detail}")
        }
    )))
}

fn sudo_prefix(connection: &Connection) -> &'static str {
    if connection.username == "root" {
        ""
    } else if connection
        .sudo_password
        .as_ref()
        .is_some_and(|value| !value.expose().is_empty())
    {
        "sudo -S -p ''"
    } else {
        "sudo -n"
    }
}

fn privileged_command(connection: &Connection, command: &str) -> String {
    let sudo = sudo_prefix(connection);
    if sudo.is_empty() {
        command.to_owned()
    } else {
        format!("{sudo} {command}")
    }
}

fn privileged_stdin_command(connection: &Connection, command: &str) -> String {
    // Keep authentication and execution in one process so the remaining
    // stdin is delivered to the command after sudo consumes its password.
    privileged_command(connection, command)
}

/// The sudo password as the remote shell expects it on stdin.
///
/// Zeroizing rather than a bare `String`: `deploy` holds this for the whole
/// upload-verify-promote-install sequence, and a plain buffer would leave the
/// operator's sudo password in freed heap for the life of the process.
fn sudo_input(connection: &Connection) -> Zeroizing<String> {
    if connection.username == "root" {
        return Zeroizing::new(String::new());
    }
    Zeroizing::new(
        connection
            .sudo_password
            .as_ref()
            .filter(|value| !value.expose().is_empty())
            .map_or_else(String::new, |value| format!("{}\n", value.expose())),
    )
}

/// Probe output describing how, and whether, this host can run the installer.
struct HostTooling {
    uid: String,
    has_bash: bool,
    has_sudo: bool,
    has_doas: bool,
}

/// Reports the deploy account's uid, then bash, sudo, and doas as `yes`/`no`.
///
/// doas is reported by whether it can escalate, not by whether it exists.
/// Alpine ships the binary on every image with each rule in `/etc/doas.conf`
/// commented out, so presence proved nothing and `bootstrap_escalation` picked
/// doas on exactly the stock hosts where it cannot work. `/etc/doas.conf` is
/// mode 0640 root:root, so the deploy account cannot read it to find out
/// either. Ask doas instead: a rule that matched but wants a password reports
/// an authentication failure and will succeed once a password is supplied,
/// while an unmatched rule reports that the operation is not permitted.
const TOOLING_PROBE: &str = "id -u; for tool in bash sudo; do \
     if command -v \"$tool\" >/dev/null 2>&1; then echo yes; else echo no; fi; done; \
     if ! command -v doas >/dev/null 2>&1; then echo no; \
     elif doas -n true >/dev/null 2>&1; then echo yes; \
     else case \"$(doas -n true 2>&1)\" in \
       *[Aa]uthenticat*|*[Aa]uthoriz*) echo yes ;; *) echo no ;; esac; fi";

fn parse_host_tooling(output: &str) -> HostTooling {
    let mut lines = output.lines();
    HostTooling {
        uid: lines.next().unwrap_or_default().trim().to_owned(),
        has_bash: lines.next().unwrap_or_default().trim() == "yes",
        has_sudo: lines.next().unwrap_or_default().trim() == "yes",
        has_doas: lines.next().unwrap_or_default().trim() == "yes",
    }
}

/// How to become root on a host that does not have sudo yet.
///
/// A stock Alpine image has neither bash nor sudo, so the escalation used for
/// the bootstrap cannot be the sudo prefix the rest of this module relies on.
fn bootstrap_escalation(tooling: &HostTooling) -> io::Result<&'static str> {
    if tooling.uid == "0" {
        Ok("")
    } else if tooling.has_sudo {
        Ok("sudo -S -p ''")
    } else if tooling.has_doas {
        Ok("doas")
    } else {
        Err(io::Error::other(
            "this Raspberry Pi has no sudo, no doas, and the deploy account is \
             not root, so the appliance cannot be bootstrapped remotely. Alpine \
             ships neither by default. Connect as root, or run \
             `su -c '/bin/sh bootstrap.sh'` once on the Pi with \
             deploy/host/bootstrap.sh copied across.",
        ))
    }
}

fn needs_su_bootstrap(tooling: &HostTooling, connection: &Connection) -> bool {
    tooling.uid != "0"
        && !tooling.has_sudo
        && connection
            .bootstrap_root_password
            .as_ref()
            .is_some_and(|value| !value.expose().is_empty())
}

/// Install bash and sudo when the target is a stock Alpine image.
///
/// `install.sh` and `transaction.sh` are both bash scripts invoked through
/// sudo, so on an untouched Alpine host every later step of this deployment
/// would fail on a missing interpreter rather than on anything to do with the
/// appliance.
fn ensure_host_bootstrapped(
    session: &mut SshSession,
    connection: &Connection,
    project_root: &Path,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    let probe = session.run(TOOLING_PROBE, "", cancellation)?;
    require_success(&probe, "Remote tooling probe")?;
    let tooling = parse_host_tooling(&probe.stdout);
    if tooling.has_bash && tooling.has_sudo {
        return Ok(());
    }

    progress("Bootstrapping bash and sudo on the Raspberry Pi...");
    let local = secure_relative(project_root, "deploy/host/bootstrap.sh")?;
    let remote = format!("/tmp/omt-bootstrap-{}.sh", random_token(8)?);
    let remote_q = shell_quote(&remote);
    session.upload(&local, &remote, cancellation)?;

    // /bin/sh explicitly: this is the one script that must run before bash does.
    let result = if needs_su_bootstrap(&tooling, connection) {
        // BusyBox su reads from /dev/tty, so a PTY is required. Disable echo
        // before sending the password: channel input may arrive before su has
        // displayed its prompt and must never be reflected into captured logs.
        let root_password = connection
            .bootstrap_root_password
            .as_ref()
            .ok_or_else(|| io::Error::other("bootstrap root password is missing"))?;
        let inner = shell_quote(&format!("/bin/sh {remote_q}"));
        let command = format!(
            "stty -echo; printf '{BOOTSTRAP_PASSWORD_READY}\\n'; su -c {inner}; rc=$?; stty echo; rm -f -- {remote_q}; exit $rc"
        );
        let input = Zeroizing::new(format!("{}\n", root_password.expose()));
        session.run_pty_after_marker(&command, BOOTSTRAP_PASSWORD_READY, &input, cancellation)?
    } else {
        let escalation = bootstrap_escalation(&tooling)?;
        let command =
            format!("{escalation} /bin/sh {remote_q}; rc=$?; rm -f -- {remote_q}; exit $rc");
        session.run(&command, &sudo_input(connection), cancellation)?
    };
    require_success(&result, "Alpine bootstrap")?;
    Ok(())
}

fn privileged_argv_command(connection: &Connection, argv: &[&str]) -> String {
    privileged_command(
        connection,
        &argv
            .iter()
            .copied()
            .map(shell_quote)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn first_web_password(logs: &str) -> Option<&str> {
    let mut lines = logs.lines();
    while let Some(line) = lines.next() {
        if !line.contains("Web UI password (save this now)") {
            continue;
        }
        let value = lines.next()?.trim();
        if value.is_empty() || value.bytes().all(|byte| byte == b'=') {
            return None;
        }
        return Some(value);
    }
    None
}

fn wait_for_reboot(
    connections: &[&Connection],
    timeout: Duration,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<SshSession> {
    progress("Waiting for the Raspberry Pi to reboot...");
    let deadline = Instant::now() + timeout;
    let mut saw_down = false;
    while Instant::now() < deadline {
        cancelled(cancellation)?;
        for connection in connections {
            match connect(connection) {
                Ok(session) if saw_down => return Ok(session),
                Ok(_) => {}
                Err(_) => saw_down = true,
            }
        }
        thread::sleep(Duration::from_secs(2));
    }
    Err(io::Error::other(
        "the Raspberry Pi did not come back after reboot within the wait",
    ))
}

fn wait_for_appliance(
    connection: &Connection,
    session: &mut SshSession,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    progress("Waiting for the OMT appliance to start...");
    let command = privileged_command(
        connection,
        "docker inspect -f '{{.State.Status}}' omt-client",
    );
    let stdin = sudo_input(connection);
    let deadline = Instant::now() + Duration::from_mins(3);
    while Instant::now() < deadline {
        cancelled(cancellation)?;
        let result = session.run(&command, &stdin, cancellation)?;
        if result.is_success() && result.stdout.trim() == "running" {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(2));
    }
    Err(io::Error::other(
        "the OMT appliance container did not reach running state within three minutes",
    ))
}

fn fetch_initial_web_password(
    connection: &Connection,
    session: &mut SshSession,
    cancellation: &AtomicBool,
) -> io::Result<Option<String>> {
    let command = privileged_argv_command(connection, ManagementAction::Logs.remote_argv());
    let result = session.run(&command, &sudo_input(connection), cancellation)?;
    if !result.is_success() {
        return Ok(None);
    }
    Ok(first_web_password(&result.combined()).map(str::to_owned))
}

fn file_fingerprint(path: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "deployment artifact is missing or unsafe: {}",
                path.display()
            ),
        ));
    }
    let modified = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |value| value.as_nanos());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "{}:{}:{}:{modified}",
            metadata.dev(),
            metadata.ino(),
            metadata.len()
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(format!("{}:{modified}", metadata.len()))
    }
}

fn capture_identity(path: &Path) -> io::Result<ArtifactIdentity> {
    Ok(ArtifactIdentity {
        fingerprint: file_fingerprint(path)?,
        digest: sha256_file(path)?,
    })
}

fn identity_unchanged(path: &Path, identity: &ArtifactIdentity) -> io::Result<bool> {
    Ok(file_fingerprint(path)? == identity.fingerprint && sha256_file(path)? == identity.digest)
}

fn parse_sha256_line(output: &str) -> Option<String> {
    let digest = output.split_whitespace().next()?;
    if digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(digest.to_ascii_lowercase())
    } else {
        None
    }
}

/// The boards this appliance supports, as device-tree model prefixes.
///
/// This is the Rust half of the table in `deploy/lib/board-profile.sh`; the
/// installer refuses the same set on the host itself. Each prefix ends at a
/// word boundary, which is why the Pi 5 entry is not simply `Raspberry Pi 5`:
/// that also matches `Raspberry Pi 500`. Two spellings are easy to get wrong --
/// the Pi 3 B+ reports `Model B Plus`, and early Zero 2 W boards report
/// `Zero 2` with no W.
const SUPPORTED_BOARDS: [&str; 4] = [
    "Raspberry Pi 5",
    "Raspberry Pi 4 Model B",
    "Raspberry Pi 3 Model ",
    "Raspberry Pi Zero 2",
];

/// Whether a device-tree model names a board this appliance supports.
fn is_supported_board(model: &str) -> bool {
    SUPPORTED_BOARDS.iter().any(|prefix| {
        model
            .strip_prefix(prefix)
            // A prefix that already ends in a space has consumed its own
            // boundary; otherwise the next character must start a new word.
            .is_some_and(|rest| prefix.ends_with(' ') || rest.is_empty() || rest.starts_with(' '))
    })
}

fn require_supported_appliance(output: &str) -> io::Result<()> {
    let mut lines = output.lines();
    let architecture = lines.next().unwrap_or_default();
    let system = lines.next().unwrap_or_default();
    let release = lines.next().unwrap_or_default();
    let model = lines.next().unwrap_or_default();
    if architecture == "aarch64"
        && system == "alpine"
        && release.starts_with("3.24.")
        && is_supported_board(model)
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "remote host must run Alpine Linux 3.24 aarch64 on a Raspberry Pi 5, \
             Raspberry Pi 4 Model B, Raspberry Pi 3, or Raspberry Pi Zero 2 W",
        ))
    }
}

/// The board named by a platform probe, for progress reporting.
fn probed_board(output: &str) -> Option<&str> {
    output.lines().nth(3).filter(|model| !model.is_empty())
}

fn redact(message: &str, secrets: &[&str]) -> String {
    let mut safe = message.to_owned();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        let mut offset = 0;
        while let Some(index) = safe[offset..].find(secret) {
            let at = offset + index;
            safe.replace_range(at..at + secret.len(), "[redacted]");
            offset = at + "[redacted]".len();
        }
    }
    safe
}

fn installer_summary(output: &str) -> Option<&str> {
    output
        .find("=== Installation Complete ===")
        .map(|start| output[start..].trim())
        .filter(|summary| !summary.is_empty())
}

fn build_image(
    options: &DeployOptions,
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    if !options.build_image {
        let tarball = options.project_root.join(&options.tarball_name);
        if !tarball.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "{} not found; build the appliance image or copy an archive built \
                     elsewhere into the project root",
                    tarball.display()
                ),
            ));
        }
        return Ok(());
    }
    progress("Building the ARM64 appliance image...");
    // Windows engines forget their binfmt registration whenever the VM
    // restarts, so a Setup view that was green an hour ago proves nothing
    // about now. Re-establishing it here costs one small container and spares
    // the operator a build that fails minutes in with "exec format error".
    // Linux hosts register it persistently through
    // `scripts/install-arm64-emulation.sh`, and `scripts/build-arm64.sh`
    // checks it there.
    if ON_WINDOWS {
        ensure_arm64_emulation(cancellation, progress)?;
    }
    // Resolved before the spawn, so a workstation without the build tooling is
    // told which tool is missing and how to get it. The spawn's own error is
    // "program not found", which named neither.
    let plan = image_build_plan(&options.tarball_name)?;
    progress(&format!("Running {}", plan.summary()));
    let result = run_process(
        &plan.program,
        &plan.args,
        &options.project_root,
        &plan.env,
        Arc::clone(cancellation),
    )?;
    if result.exit_code != 0 {
        return Err(io::Error::other(format!(
            "ARM64 image build failed:\n{}",
            result.output
        )));
    }
    Ok(())
}

/// Open an authenticated SSH session with strict `known_hosts` verification.
pub fn connect(connection: &Connection) -> io::Result<SshSession> {
    validate_connection(connection).map_err(map_validation)?;
    SshSession::connect(connection)
}

/// Probe the remote platform and confirm Alpine 3.24 aarch64 on a supported Pi.
pub fn test_connection(
    connection: &Connection,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    cancelled(cancellation)?;
    progress("Testing SSH connection...");
    let mut session = connect(connection)?;
    let result = session.run(PLATFORM_PROBE, "", cancellation)?;
    require_success(&result, "Remote platform probe")?;
    require_supported_appliance(&result.stdout)?;
    match probed_board(&result.stdout) {
        Some(board) => progress(&format!("SSH connection succeeded. Detected {board}.")),
        None => progress("SSH connection succeeded."),
    }
    Ok(())
}

fn password_connection(
    base: &Connection,
    username: &str,
    password: &Secret,
    sudo_password: Option<&Secret>,
) -> io::Result<Connection> {
    Ok(Connection {
        host: base.host.clone(),
        username: username.to_owned(),
        port: base.port,
        auth: crate::AuthMethod::Password,
        password: Some(Secret::new(password.expose().to_owned()).map_err(map_validation)?),
        key_path: None,
        key_passphrase: None,
        known_hosts_path: base.known_hosts_path.clone(),
        sudo_password: match sudo_password {
            Some(value) => Some(Secret::new(value.expose().to_owned()).map_err(map_validation)?),
            None => None,
        },
        bootstrap_root_password: None,
    })
}

/// Configure a factory Alpine image: hostname, IPv4 DHCP, optional Wi-Fi, user
/// `pi`, root/pi passwords, US HTTPS apk mirrors, and persistent sys mode.
pub fn alpine_setup(
    connection: &Connection,
    settings: &AlpineSetupSettings,
    project_root: &Path,
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    validate_connection(connection).map_err(map_validation)?;
    validate_alpine_setup(settings).map_err(map_validation)?;
    cancelled(cancellation)?;

    progress("Connecting and checking the Raspberry Pi...");
    let mut session = connect(connection)?;
    let probe = session.run(PLATFORM_PROBE, "", cancellation)?;
    require_success(&probe, "Remote platform probe")?;
    require_supported_appliance(&probe.stdout)?;
    if let Some(board) = probed_board(&probe.stdout) {
        progress(&format!("Installing Alpine sys mode on {board}."));
    }

    let local = secure_relative(project_root, SETUP_SYS_MEMBER)?;
    let remote = format!("/tmp/omt-setup-sys-{}.sh", random_token(8)?);
    let remote_q = shell_quote(&remote);
    progress("Uploading the Alpine sys-setup script...");
    session.upload(&local, &remote, cancellation)?;

    let ssid_hex = settings
        .wifi
        .as_ref()
        .map(|wifi| hex_encode(wifi.ssid.as_bytes()))
        .unwrap_or_default();
    let derived_psk;
    let psk = if let Some(wifi) = settings.wifi.as_ref() {
        let password = wifi.password.expose();
        if password.len() == 64 && password.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Zeroizing::new(password.to_ascii_lowercase())
        } else {
            derived_psk = derive_wpa_psk(&wifi.ssid, &wifi.password).map_err(map_validation)?;
            Zeroizing::new(derived_psk.expose().to_owned())
        }
    } else {
        Zeroizing::new(String::new())
    };

    let mut stdin = Zeroizing::new(String::new());
    stdin.push_str(&settings.hostname);
    stdin.push('\n');
    stdin.push_str(settings.root_password.expose());
    stdin.push('\n');
    stdin.push_str(settings.pi_password.expose());
    stdin.push('\n');
    stdin.push_str(&ssid_hex);
    stdin.push('\n');
    stdin.push_str(&psk);
    stdin.push('\n');

    progress("Running hostname, DHCP, user, and sys-mode install...");
    let command = format!("/bin/sh {remote_q}; rc=$?; rm -f -- {remote_q}; exit $rc");
    let result = session.run(&command, &stdin, cancellation)?;
    let wifi_secret = settings
        .wifi
        .as_ref()
        .map(|wifi| wifi.password.expose())
        .unwrap_or_default();
    let secrets = [
        settings.root_password.expose(),
        settings.pi_password.expose(),
        wifi_secret,
        psk.as_str(),
    ];
    if !result.is_success() {
        let detail = redact(&result.combined(), &secrets);
        return Err(io::Error::other(format!(
            "Alpine sys install failed{}",
            if detail.trim().is_empty() {
                String::new()
            } else {
                format!(":\n{detail}")
            }
        )));
    }
    if !result.combined().contains(SETUP_SYS_COMPLETE) {
        return Err(io::Error::other(
            "Alpine sys install finished without the completion marker",
        ));
    }
    progress("Alpine sys install finished. Rebooting into the persistent root...");
    let reboot = privileged_argv_command(connection, ManagementAction::Reboot.remote_argv());
    require_success(
        &session.run(&reboot, &sudo_input(connection), cancellation)?,
        "Post-sys-install reboot",
    )?;
    drop(session);

    let pi = password_connection(
        connection,
        "pi",
        &settings.pi_password,
        Some(&settings.pi_password),
    )?;
    let root = password_connection(connection, "root", &settings.root_password, None)?;
    wait_for_reboot(
        &[&pi, &root],
        Duration::from_mins(8),
        cancellation,
        progress,
    )?;
    progress(
        "Persistent sys mode is running. Connect as pi with the password you set, then Deploy. \
         The pi account is in wheel; the first deploy installs sudo.",
    );
    Ok(())
}

/// Run a fixed management action against the appliance container.
pub fn manage(
    connection: &Connection,
    action: ManagementAction,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<String> {
    cancelled(cancellation)?;
    progress(match action {
        ManagementAction::Status => "Fetching container status...",
        ManagementAction::Logs => "Fetching recent logs...",
        ManagementAction::Restart => "Restarting service...",
        ManagementAction::Reboot => "Scheduling operating-system reboot...",
    });
    let mut session = connect(connection)?;
    let docker_command = action
        .remote_argv()
        .iter()
        .copied()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    let command = privileged_command(connection, &docker_command);
    let stdin = sudo_input(connection);
    let result = session.run(&command, &stdin, cancellation)?;
    require_success(&result, "Remote management action")?;
    let output = result.combined();
    if !output.trim().is_empty() {
        progress(&output);
    }
    Ok(output)
}

/// Replace the appliance Web credential over stdin and restart the service.
///
/// The password is never interpolated into a command or emitted as progress.
/// A single privileged process leaves stdin attached to `docker exec` after
/// sudo consumes its own first line, matching the established Wi-Fi secret
/// transport.
pub fn change_web_password(
    connection: &Connection,
    password: &Secret,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    validate_connection(connection).map_err(map_validation)?;
    validate_web_password(password).map_err(map_validation)?;
    cancelled(cancellation)?;
    progress("Changing Web GUI password and restarting the service...");
    let mut session = connect(connection)?;
    let command = privileged_stdin_command(connection, WEB_PASSWORD_COMMAND);
    let mut stdin = sudo_input(connection);
    stdin.push_str(password.expose());
    stdin.push('\n');
    let result = session.run(&command, &stdin, cancellation)?;
    if !result.is_success() {
        let detail = redact(&result.combined(), &[password.expose()]);
        return Err(io::Error::other(format!(
            "Web GUI password change failed{}",
            if detail.trim().is_empty() {
                String::new()
            } else {
                format!(":\n{detail}")
            }
        )));
    }
    progress("Web GUI password changed. Existing Web sessions were revoked.");
    Ok(())
}

/// Apply Wi-Fi settings through `wpa_cli`, sending a derived PSK on stdin.
pub fn apply_wifi(
    connection: &Connection,
    settings: &WifiSettings,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    validate_connection(connection).map_err(map_validation)?;
    validate_wifi(settings).map_err(map_validation)?;
    cancelled(cancellation)?;
    progress(if settings.connect {
        "Applying Wi-Fi settings and requesting a connection..."
    } else {
        "Saving Wi-Fi profile without connecting..."
    });
    if settings.connect {
        progress("SSH may disconnect if the Raspberry Pi switches networks.");
    }

    let password = settings.password.expose();
    let psk = if password.len() == 64 && password.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Secret::new(password.to_ascii_lowercase()).map_err(map_validation)?
    } else {
        derive_wpa_psk(&settings.ssid, &settings.password).map_err(map_validation)?
    };
    let marker = format!("__OMT_WIFI_PASSWORD_FOLLOWS_{}__", random_token(12)?);
    let ssid_hex = hex_encode(settings.ssid.as_bytes());
    let mut stdin = sudo_input(connection);
    stdin.push_str(&marker);
    stdin.push('\n');
    stdin.push_str(psk.expose());
    stdin.push('\n');

    let wifi_command = format!(
        "sh -eu -c {} sh {} {} {}",
        shell_quote(WIFI_SCRIPT),
        shell_quote(&ssid_hex),
        shell_quote(if settings.connect { "yes" } else { "no" }),
        shell_quote(&marker),
    );
    // Authenticate and execute in one sudo process. Alpine's default sudo
    // timestamp policy may not carry a non-interactive `sudo -v` ticket into
    // a second `sudo -n` invocation, even in the same SSH channel. The first
    // input line is consumed by sudo; the marker and PSK remain available to
    // the privileged shell on the same stdin stream.
    let command = privileged_stdin_command(connection, &wifi_command);
    let secrets = [
        connection
            .password
            .as_ref()
            .map(Secret::expose)
            .unwrap_or_default(),
        connection
            .key_passphrase
            .as_ref()
            .map(Secret::expose)
            .unwrap_or_default(),
        connection
            .sudo_password
            .as_ref()
            .map(Secret::expose)
            .unwrap_or_default(),
        psk.expose(),
        password,
    ];

    let mut session = connect(connection)?;
    let result = session.run(&command, stdin.as_str(), cancellation)?;
    if !result.is_success() {
        let detail = redact(&result.combined(), &secrets);
        return Err(io::Error::other(format!("Wi-Fi update failed:\n{detail}")));
    }
    progress(if settings.connect {
        "Wi-Fi settings applied and connection requested."
    } else {
        "Wi-Fi settings saved."
    });
    Ok(())
}

/// Where one deployment's files are staged before they are promoted.
struct Staging<'a> {
    stage: &'a str,
    token: &'a str,
    remote: &'a str,
}

fn remove_stage(session: &mut SshSession, stage_q: &str) {
    let command = format!(
        "if [ -d {stage_q} ] && [ ! -L {stage_q} ]; then find -P {stage_q} -xdev -depth -delete; fi"
    );
    // Deliberately uncancellable: this is the failure path, and the operator
    // cancelling is one of the reasons it runs.
    let _ = session.run(&command, "", &AtomicBool::new(false));
}

/// Uploads every manifest member, verifies each one's remote digest, and
/// promotes the set. The caller removes the staging directory if this fails.
fn stage_and_promote(
    session: &mut SshSession,
    identities: &[(String, PathBuf, ArtifactIdentity)],
    staging: &Staging<'_>,
    cancellation: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    let stage = staging.stage;
    for (name, local, identity) in identities {
        cancelled(cancellation)?;
        if let Some(parent) = Path::new(name)
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            let remote_parent = format!("{stage}/{}", parent.display());
            let mkdir = format!("mkdir -p -- {}", shell_quote(&remote_parent));
            require_success(
                &session.run(&mkdir, "", cancellation)?,
                "Remote staging preparation",
            )?;
        }

        progress(&format!("Uploading {name}..."));
        let remote_path = format!("{stage}/{name}");
        session.upload(local, &remote_path, cancellation)?;
        if !identity_unchanged(local, identity)? {
            return Err(io::Error::other(format!(
                "local deployment artifact changed during upload: {name}"
            )));
        }
        let checksum = session.run(
            &format!("sha256sum -- {}", shell_quote(&remote_path)),
            "",
            cancellation,
        )?;
        require_success(&checksum, "Remote checksum")?;
        if parse_sha256_line(&checksum.stdout).as_ref() != Some(&identity.digest) {
            return Err(io::Error::other(format!(
                "SHA-256 mismatch after uploading {name}"
            )));
        }
        progress(&format!("Verified SHA-256 for {name}."));
    }

    let promote = format!(
        "bash {} promote {} {} {}",
        shell_quote(&format!("{stage}/deploy/transaction.sh")),
        staging.remote,
        shell_quote(staging.token),
        shell_quote(&format!("{stage}/deploy/manifest-v3.txt")),
    );
    require_success(
        &session.run(&promote, "", cancellation)?,
        "Deployment promotion",
    )
}

/// Build (optional), upload, verify, recover, promote, and install the capsule.
pub fn deploy(
    connection: &Connection,
    options: &DeployOptions,
    cancellation: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str),
) -> io::Result<()> {
    validate_connection(connection).map_err(map_validation)?;
    validate_options(options, true).map_err(map_validation)?;
    cancelled(cancellation)?;

    let manifest_path = options.project_root.join("deploy/manifest-v3.txt");
    let members = load_manifest(&manifest_path)?;
    if !members.iter().any(|name| name == &options.tarball_name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "deployment artifact manifest does not include {}",
                options.tarball_name
            ),
        ));
    }

    build_image(options, cancellation, progress)?;
    cancelled(cancellation)?;

    let mut identities = Vec::with_capacity(members.len());
    for name in &members {
        let local = secure_relative(&options.project_root, name)?;
        identities.push((name.clone(), local.clone(), capture_identity(&local)?));
    }

    progress("Connecting and checking the Raspberry Pi...");
    let mut session = connect(connection)?;
    let probe = session.run(PLATFORM_PROBE, "", cancellation)?;
    require_success(&probe, "Remote platform probe")?;
    require_supported_appliance(&probe.stdout)?;
    if let Some(board) = probed_board(&probe.stdout) {
        progress(&format!("Deploying to {board}."));
    }

    ensure_host_bootstrapped(
        &mut session,
        connection,
        &options.project_root,
        cancellation,
        progress,
    )?;

    let remote_directory = options.remote_directory.trim_end_matches('/').to_owned();
    let remote_q = shell_quote(&remote_directory);
    let sudo_data = sudo_input(connection);
    let prepare = privileged_command(
        connection,
        &format!("install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" {remote_q}"),
    );
    require_success(
        &session.run(&prepare, &sudo_data, cancellation)?,
        "Remote directory preparation",
    )?;

    let token = random_token(12)?;
    let staging_root = format!("{remote_directory}/.deploy-staging");
    let stage = format!("{staging_root}/{token}");
    let staging_q = shell_quote(&staging_root);
    let stage_q = shell_quote(&stage);

    let recovery_command = format!(
        "if [ -x {legacy} ] && [ -f {legacy_manifest} ]; then {legacy} recover {remote_q} {legacy_manifest}; fi; \
         if [ -x {current} ]; then {current} recover {remote_q}; fi",
        legacy = shell_quote(&format!("{remote_directory}/deploy-transaction.sh")),
        legacy_manifest = shell_quote(&format!("{remote_directory}/deploy-artifacts.txt")),
        current = shell_quote(&format!("{remote_directory}/deploy/transaction.sh")),
    );
    require_success(
        &session.run(&recovery_command, "", cancellation)?,
        "Interrupted deployment recovery",
    )?;

    let stage_prepare = format!(
        "if [ -L {staging_q} ] || {{ [ -e {staging_q} ] && [ ! -d {staging_q} ]; }}; then exit 14; fi; \
         install -d -m 700 -- {staging_q}; mkdir -- {stage_q}"
    );
    require_success(
        &session.run(&stage_prepare, "", cancellation)?,
        "Remote staging root validation",
    )?;

    // One owner for the staging directory: every failure between here and a
    // completed promotion removes it, including a transport error, which the
    // per-step cleanup this replaced could not see.
    let staged = stage_and_promote(
        &mut session,
        &identities,
        &Staging {
            stage: &stage,
            token: &token,
            remote: &remote_q,
        },
        cancellation,
        progress,
    );
    if staged.is_err() {
        remove_stage(&mut session, &stage_q);
    }
    staged?;

    let executable_paths = [
        "deploy/host/bootstrap.sh",
        "deploy/host/setup-sys.sh",
        "deploy/host/install.sh",
        "deploy/host/uninstall.sh",
        "deploy/host/host-diagnostics.sh",
        "deploy/host/host-event-watcher.sh",
        "deploy/host/host-reboot.sh",
        "deploy/transaction.sh",
    ];
    let mut chmod = String::from("chmod +x");
    for relative in executable_paths {
        chmod.push(' ');
        chmod.push_str(&shell_quote(&format!("{remote_directory}/{relative}")));
    }
    let installer = shell_quote(&format!("{remote_directory}/deploy/host/install.sh"));
    let install_script = shell_quote(&format!("printf 'n\\n' | {installer}"));
    let install = format!(
        "{chmod} && {}",
        privileged_command(connection, &format!("sh -c {install_script}"))
    );
    let install_result = session.run(&install, &sudo_data, cancellation)?;
    require_success(&install_result, "Remote installer")?;
    // The installer prints its operator summary on stdout. OpenRC and Compose
    // warnings arrive on stderr and are deliberately excluded here; combining
    // the streams appended those warnings after an otherwise clean summary.
    if let Some(summary) = installer_summary(&install_result.stdout) {
        let secrets = [
            connection
                .password
                .as_ref()
                .map(Secret::expose)
                .unwrap_or_default(),
            connection
                .key_passphrase
                .as_ref()
                .map(Secret::expose)
                .unwrap_or_default(),
            connection
                .sudo_password
                .as_ref()
                .map(Secret::expose)
                .unwrap_or_default(),
            connection
                .bootstrap_root_password
                .as_ref()
                .map(Secret::expose)
                .unwrap_or_default(),
        ];
        progress(&redact(summary, &secrets));
    }
    progress("Rebooting to apply kernel, firmware, and KMS settings...");
    let reboot = privileged_argv_command(connection, ManagementAction::Reboot.remote_argv());
    require_success(
        &session.run(&reboot, &sudo_data, cancellation)?,
        "Post-install reboot",
    )?;
    drop(session);
    let mut session = wait_for_reboot(
        &[connection],
        Duration::from_mins(6),
        cancellation,
        progress,
    )?;
    match wait_for_appliance(connection, &mut session, cancellation, progress) {
        Ok(()) => {
            if let Some(password) =
                fetch_initial_web_password(connection, &mut session, cancellation)?
            {
                progress(&format!("Web UI password (save this now): {password}"));
            }
            progress("Appliance is running.");
        }
        Err(error) => {
            progress(&format!(
                "Install succeeded, but the appliance did not start within the wait: {error}. \
                 Check `sudo rc-service omt-client status` on the Pi."
            ));
        }
    }
    progress("Deployment complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(model: &str) -> String {
        format!("aarch64\nalpine\n3.24.1\n{model}\n")
    }

    /// The same matrix as `tests/unit/test_board_profile.sh`. These two gates
    /// run on different machines -- this one on the operator's workstation
    /// before upload, the shell one on the Pi -- so a board either passes both
    /// or the deployment fails halfway.
    #[test]
    fn accepts_every_supported_board() {
        for model in [
            "Raspberry Pi 5 Model B Rev 1.0",
            "Raspberry Pi 4 Model B Rev 1.4",
            "Raspberry Pi 3 Model B Rev 1.2",
            "Raspberry Pi 3 Model B Plus Rev 1.3",
            "Raspberry Pi 3 Model A Plus Rev 1.0",
            "Raspberry Pi Zero 2 W Rev 1.0",
            "Raspberry Pi Zero 2 Rev 1.0",
        ] {
            assert!(
                require_supported_appliance(&probe(model)).is_ok(),
                "rejected a supported board: {model}"
            );
        }
    }

    /// The near misses matter most: a `Raspberry Pi 5` prefix without a word
    /// boundary also matches the Pi 500, and a loosened Zero prefix would
    /// catch the 32-bit-only original Zero W.
    #[test]
    fn refuses_unsupported_boards_and_near_misses() {
        for model in [
            "Raspberry Pi 500 Rev 1.0",
            "Raspberry Pi 400 Rev 1.0",
            "Raspberry Pi 2 Model B Rev 1.1",
            "Raspberry Pi Zero W Rev 1.1",
            "Raspberry Pi Model B Plus Rev 1.2",
            "Raspberry Pi Compute Module 4 Rev 1.0",
            "Raspberry Pi Compute Module 5 Rev 1.0",
            "Orange Pi 5",
            "",
        ] {
            assert!(
                require_supported_appliance(&probe(model)).is_err(),
                "accepted an unsupported board: {model}"
            );
        }
    }

    #[test]
    fn still_refuses_the_wrong_architecture_or_distribution() {
        let model = "Raspberry Pi 5 Model B Rev 1.0";
        for output in [
            format!("armv7l\nalpine\n3.24.1\n{model}\n"),
            format!("aarch64\ndebian\n3.24.1\n{model}\n"),
            format!("aarch64\nalpine\n3.22.1\n{model}\n"),
            // 3.23 was the previously pinned series. Package names moved in
            // 3.24, so an older host must fail the probe rather than reach an
            // installer whose apk list it cannot resolve.
            format!("aarch64\nalpine\n3.23.5\n{model}\n"),
            "aarch64\nalpine\n3.24.1\n".to_owned(),
        ] {
            assert!(
                require_supported_appliance(&output).is_err(),
                "accepted an unsupported platform: {output:?}"
            );
        }
    }

    /// A stock Alpine image is the case this has to get right: no bash, no
    /// sudo, and a non-root deploy account is not a deployable host.
    #[test]
    fn picks_an_escalation_for_each_stock_alpine_shape() {
        let tooling = |uid: &str, bash, sudo, doas| HostTooling {
            uid: uid.to_owned(),
            has_bash: bash,
            has_sudo: sudo,
            has_doas: doas,
        };

        // Already root: escalating again would only add a failure mode.
        assert_eq!(
            bootstrap_escalation(&tooling("0", false, false, false)).ok(),
            Some("")
        );
        assert_eq!(
            bootstrap_escalation(&tooling("1000", false, true, false)).ok(),
            Some("sudo -S -p ''")
        );
        // doas is the only escalation a hand-bootstrapped Alpine host has.
        assert_eq!(
            bootstrap_escalation(&tooling("1000", false, false, true)).ok(),
            Some("doas")
        );
        assert!(bootstrap_escalation(&tooling("1000", false, false, false)).is_err());
    }

    #[test]
    fn parses_the_tooling_probe() {
        let tooling = parse_host_tooling("1000\nno\nno\nyes\n");
        assert_eq!(tooling.uid, "1000");
        assert!(!tooling.has_bash);
        assert!(!tooling.has_sudo);
        assert!(tooling.has_doas);
    }

    #[test]
    fn reports_the_detected_board_for_progress() {
        assert_eq!(
            probed_board(&probe("Raspberry Pi 4 Model B Rev 1.4")),
            Some("Raspberry Pi 4 Model B Rev 1.4")
        );
        assert_eq!(probed_board("aarch64\nalpine\n3.24.1\n"), None);
    }

    #[test]
    fn digest_parser_accepts_sha256sum_output() {
        assert_eq!(
            parse_sha256_line(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  file\n"
            )
            .as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert!(parse_sha256_line("not-a-digest").is_none());
    }

    #[test]
    fn redaction_avoids_infinite_loops_on_marker_substrings() {
        let safe = redact("secret [redacted] secret", &["secret", "eda"]);
        assert!(!safe.contains("secret"));
        assert!(safe.contains("[redacted]"));
    }

    #[test]
    fn successful_installs_surface_only_the_final_summary() {
        let output = "apk noise\n=== Installation Complete ===\nWeb UI: https://pi:5000\n";
        assert_eq!(
            installer_summary(output),
            Some("=== Installation Complete ===\nWeb UI: https://pi:5000")
        );
        assert_eq!(installer_summary("apk noise only"), None);
    }

    /// `deploy` holds the sudo stdin for the whole upload-verify-promote-install
    /// sequence. The return type is the guarantee that it is wiped afterwards --
    /// a bare `String` here left the operator's sudo password in freed heap --
    /// so this asserts the type as well as the framing the remote shell reads.
    #[test]
    fn sudo_input_is_zeroized_and_newline_terminated() {
        fn assert_zeroizing(_value: &Zeroizing<String>) {}

        let mut connection = Connection {
            host: "pi.local".into(),
            username: "pi".into(),
            port: 22,
            auth: crate::AuthMethod::Password,
            password: None,
            key_path: None,
            key_passphrase: None,
            known_hosts_path: None,
            sudo_password: Secret::new("hunter2".into()).ok(),
            bootstrap_root_password: None,
        };
        let input = sudo_input(&connection);
        assert_zeroizing(&input);
        assert_eq!(input.as_str(), "hunter2\n");
        assert_eq!(sudo_prefix(&connection), "sudo -S -p ''");
        assert_eq!(
            privileged_command(&connection, "docker ps"),
            "sudo -S -p '' docker ps"
        );
        assert_eq!(
            privileged_stdin_command(&connection, "sh wifi-update"),
            "sudo -S -p '' sh wifi-update"
        );

        // No password means passwordless sudo: nothing on stdin, and the
        // non-interactive prefix rather than one that waits for a prompt.
        connection.sudo_password = None;
        assert!(sudo_input(&connection).is_empty());
        assert_eq!(sudo_prefix(&connection), "sudo -n");

        // An empty password is the same as none, not an empty line on stdin.
        connection.sudo_password = Secret::new(String::new()).ok();
        assert!(sudo_input(&connection).is_empty());
        assert_eq!(sudo_prefix(&connection), "sudo -n");

        // A root SSH session executes the same fixed command directly, even
        // if a caller supplied a redundant sudo password.
        connection.username = "root".into();
        connection.sudo_password = Secret::new("unused".into()).ok();
        assert!(sudo_input(&connection).is_empty());
        assert_eq!(sudo_prefix(&connection), "");
        assert_eq!(privileged_command(&connection, "docker ps"), "docker ps");
    }

    #[test]
    fn web_password_action_uses_only_a_fixed_stdin_command() {
        assert_eq!(
            WEB_PASSWORD_COMMAND,
            "sh -eu -c 'docker exec -i omt-client /usr/local/bin/omt-web set-password && rc-service omt-client restart'"
        );
        assert!(!WEB_PASSWORD_COMMAND.contains("$1"));
        assert!(!WEB_PASSWORD_COMMAND.contains("printf"));
    }

    #[test]
    fn untouched_alpine_can_bootstrap_through_su_only_with_a_root_secret() {
        let tooling = HostTooling {
            uid: "1000".into(),
            has_bash: false,
            has_sudo: false,
            has_doas: false,
        };
        let mut connection = Connection {
            host: "pi.local".into(),
            username: "pi".into(),
            port: 22,
            auth: crate::AuthMethod::Password,
            password: Secret::new("ssh-password".into()).ok(),
            key_path: None,
            key_passphrase: None,
            known_hosts_path: None,
            sudo_password: Secret::new("user-password".into()).ok(),
            bootstrap_root_password: None,
        };
        assert!(!needs_su_bootstrap(&tooling, &connection));
        connection.bootstrap_root_password = Secret::new("root-password".into()).ok();
        assert!(needs_su_bootstrap(&tooling, &connection));

        // Stock Alpine's doas can report an authorization-style error to the
        // probe and then refuse the command because it has no TTY. A supplied
        // root secret makes the explicit PTY-backed su path authoritative.
        let ambiguous_doas = HostTooling {
            has_doas: true,
            ..tooling
        };
        assert!(needs_su_bootstrap(&ambiguous_doas, &connection));

        let root_tooling = HostTooling {
            uid: "0".into(),
            ..ambiguous_doas
        };
        assert!(!needs_su_bootstrap(&root_tooling, &connection));
    }

    #[test]
    fn wifi_discovers_the_wireless_interface_instead_of_hardcoding_wlan0() {
        assert!(WIFI_SCRIPT.contains("iface=${path#/sys/class/net/}"));
        assert!(WIFI_SCRIPT.contains("wpa_cli -i \"$iface\" ping"));
        assert!(WIFI_SCRIPT.contains("iw dev \"$iface\" set power_save off"));
        assert!(!WIFI_SCRIPT.contains("wpa_cli -i wlan0"));
    }

    #[test]
    fn first_web_password_reads_the_entrypoint_banner() {
        let logs = "\
============================================
 Web UI password (save this now):
 hunter2-web
============================================
omt-web listening on https://0.0.0.0:5000
";
        assert_eq!(first_web_password(logs), Some("hunter2-web"));
        assert_eq!(first_web_password("no password here"), None);
    }

    #[test]
    fn post_install_reboot_is_the_same_fixed_action_as_manage() {
        let connection = Connection {
            host: "pi.local".into(),
            username: "pi".into(),
            port: 22,
            auth: crate::AuthMethod::Password,
            password: None,
            key_path: None,
            key_passphrase: None,
            known_hosts_path: None,
            sudo_password: Secret::new("hunter2".into()).ok(),
            bootstrap_root_password: None,
        };
        assert_eq!(
            privileged_argv_command(&connection, ManagementAction::Reboot.remote_argv()),
            privileged_command(
                &connection,
                &ManagementAction::Reboot
                    .remote_argv()
                    .iter()
                    .copied()
                    .map(shell_quote)
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        );
    }

    #[test]
    fn alpine_sys_setup_uses_the_fixed_script_and_marker() {
        assert_eq!(SETUP_SYS_MEMBER, "deploy/host/setup-sys.sh");
        assert!(SETUP_SYS_COMPLETE.contains("Alpine sys install complete"));
    }
}
