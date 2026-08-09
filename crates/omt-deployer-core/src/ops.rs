//! Deployment, management, and Wi-Fi operations over [`crate::ssh::SshSession`].

use crate::ssh::{RemoteResult, SshSession};
use crate::{
    Connection, DeployOptions, ManagementAction, Secret, WifiSettings, derive_wpa_psk, hex_encode,
    load_manifest, random_token, run_process, secure_relative, sha256_file, shell_quote,
    validate_connection, validate_options, validate_wifi,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use zeroize::Zeroizing;

const PLATFORM_PROBE: &str = "uname -m && . /etc/os-release && printf '%s\\n' \"$ID\" && cat /etc/alpine-release && tr -d '\\000' < /proc/device-tree/model && printf '\\n'";

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
    "wpa_cli -i wlan0 ping | grep -Fxq PONG || { echo 'wpa_supplicant is unavailable on wlan0' >&2; exit 12; }\n",
    "wpa_cli -i wlan0 scan >/dev/null || true\n",
    "network_id=\n",
    "for candidate in $(wpa_cli -i wlan0 list_networks | awk 'NR > 1 {print $1}'); do\n",
    "  current=$(wpa_cli -i wlan0 get_network \"$candidate\" ssid 2>/dev/null || true)\n",
    "  if [ \"$current\" = \"$ssid_hex\" ]; then network_id=$candidate; break; fi\n",
    "done\n",
    "if [ -z \"$network_id\" ]; then network_id=$(wpa_cli -i wlan0 add_network); fi\n",
    "case \"$network_id\" in ''|*[!0-9]*) echo 'Unable to allocate Wi-Fi profile' >&2; exit 13;; esac\n",
    "wpa_cli -i wlan0 set_network \"$network_id\" ssid \"$ssid_hex\" | grep -Fxq OK\n",
    "wpa_cli -i wlan0 set_network \"$network_id\" key_mgmt WPA-PSK | grep -Fxq OK\n",
    "wpa_cli -i wlan0 set_network \"$network_id\" psk \"$wifi_password\" | grep -Fxq OK\n",
    "unset wifi_password\n",
    "wpa_cli -i wlan0 enable_network \"$network_id\" | grep -Fxq OK\n",
    "wpa_cli -i wlan0 save_config | grep -Fxq OK\n",
    "if [ \"$activate\" = yes ]; then\n",
    "  wpa_cli -i wlan0 select_network \"$network_id\" >/dev/null\n",
    "  wpa_cli -i wlan0 reassociate >/dev/null\n",
    "fi\n"
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
    if connection
        .sudo_password
        .as_ref()
        .is_some_and(|value| !value.expose().is_empty())
    {
        "sudo -S -p ''"
    } else {
        "sudo -n"
    }
}

/// The sudo password as the remote shell expects it on stdin.
///
/// Zeroizing rather than a bare `String`: `deploy` holds this for the whole
/// upload-verify-promote-install sequence, and a plain buffer would leave the
/// operator's sudo password in freed heap for the life of the process.
fn sudo_input(connection: &Connection) -> Zeroizing<String> {
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
    let escalation = bootstrap_escalation(&tooling)?;
    let local = secure_relative(project_root, "deploy/host/bootstrap.sh")?;
    let remote = format!("/tmp/omt-bootstrap-{}.sh", random_token(8)?);
    let remote_q = shell_quote(&remote);
    session.upload(&local, &remote, cancellation)?;

    // /bin/sh explicitly: this is the one script that must run before bash does.
    let command = format!("{escalation} /bin/sh {remote_q}; rc=$?; rm -f -- {remote_q}; exit $rc");
    let result = session.run(&command, &sudo_input(connection), cancellation)?;
    require_success(&result, "Alpine bootstrap")?;
    Ok(())
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
                    "{} not found; run make build-arm64 or enable image build",
                    tarball.display()
                ),
            ));
        }
        return Ok(());
    }
    progress("Building the ARM64 appliance image...");
    let result = run_process(
        "make",
        &["build-arm64".into()],
        &options.project_root,
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
    });
    let mut session = connect(connection)?;
    let command = action
        .remote_argv()
        .iter()
        .copied()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    let result = session.run(&command, "", cancellation)?;
    require_success(&result, "Remote management action")?;
    let output = result.combined();
    if !output.trim().is_empty() {
        progress(&output);
    }
    Ok(output)
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
    let sudo = sudo_prefix(connection);
    let mut stdin = sudo_input(connection);
    stdin.push_str(&marker);
    stdin.push('\n');
    stdin.push_str(psk.expose());
    stdin.push('\n');

    let command = format!(
        "{sudo} -v && sudo -n sh -eu -c {} sh {} {} {}",
        shell_quote(WIFI_SCRIPT),
        shell_quote(&ssid_hex),
        shell_quote(if settings.connect { "yes" } else { "no" }),
        shell_quote(&marker),
    );
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
    let sudo = sudo_prefix(connection);
    let sudo_data = sudo_input(connection);
    let prepare = format!("{sudo} install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" {remote_q}");
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
    let install = format!("{chmod} && {sudo} sh -c {install_script}");
    require_success(
        &session.run(&install, &sudo_data, cancellation)?,
        "Remote installer",
    )?;
    progress("Deployment complete; use the installer URL shown above.");
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

    /// `deploy` holds the sudo stdin for the whole upload-verify-promote-install
    /// sequence. The return type is the guarantee that it is wiped afterwards --
    /// a bare `String` here left the operator's sudo password in freed heap --
    /// so this asserts the type as well as the framing the remote shell reads.
    #[test]
    fn sudo_input_is_zeroized_and_newline_terminated() {
        fn assert_zeroizing(_value: &Zeroizing<String>) {}

        let mut connection = Connection {
            host: "pi.local".into(),
            username: "root".into(),
            port: 22,
            auth: crate::AuthMethod::Password,
            password: None,
            key_path: None,
            key_passphrase: None,
            sudo_password: Secret::new("hunter2".into()).ok(),
        };
        let input = sudo_input(&connection);
        assert_zeroizing(&input);
        assert_eq!(input.as_str(), "hunter2\n");
        assert_eq!(sudo_prefix(&connection), "sudo -S -p ''");

        // No password means passwordless sudo: nothing on stdin, and the
        // non-interactive prefix rather than one that waits for a prompt.
        connection.sudo_password = None;
        assert!(sudo_input(&connection).is_empty());
        assert_eq!(sudo_prefix(&connection), "sudo -n");

        // An empty password is the same as none, not an empty line on stdin.
        connection.sudo_password = Secret::new(String::new()).ok();
        assert!(sudo_input(&connection).is_empty());
        assert_eq!(sudo_prefix(&connection), "sudo -n");
    }
}
