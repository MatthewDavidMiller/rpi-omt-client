//! Deployment, management, and Wi-Fi operations over an SSH session.

use omt_common::buf::wipe_vec;
use omt_common::{Error, Result};

use crate::capsule::Capsule;
use crate::core::{
    self, Action, AlpineSettings, Auth, Connection, DeployOptions, IMAGE_MEMBER, Progress, Secret,
    WifiSettings, derive_wpa_psk, random_token, report, sha256_file, sha256_hex, shell_quote,
};
use crate::ssh::wire::contains_bytes;
use crate::ssh::{Session, SshResult};
use crate::sys::{self, Cancel, now_ms};
use crate::tools;

pub const SET_HOSTNAME_MEMBER: &str = "deploy/host/set-hostname.sh";
pub const SET_HOSTNAME_COMPLETE: &str = "=== Appliance hostname set ===";
pub const SETUP_SYS_MEMBER: &str = "deploy/host/setup-sys.sh";
pub const SETUP_SYS_COMPLETE: &str = "=== Alpine sys install complete ===";
const BOOTSTRAP_PASSWORD_READY: &str = "omt-bootstrap-password-ready";
const PLATFORM_PROBE: &str = "uname -m && . /etc/os-release && printf '%s\\n' \"$ID\" && cat \
     /etc/alpine-release && tr -d '\\000' < /proc/device-tree/model && printf '\\n'";

pub const WEB_PASSWORD_COMMAND: &str = "sh -eu -c 'docker exec -i omt-client \
     /usr/local/bin/omt-web set-password && rc-service omt-client restart'";

/// The appliance's 5 GHz band policy, as wpa_supplicant's freq_list.
///
/// Real-world testing settled that 2.4 GHz cannot carry an OMT stream: the
/// loss rate makes playback unusable however strong the signal is. Keep this
/// identical to HOST_WIFI_FREQ_LIST in deploy/lib/service-install.sh and
/// WIFI_FREQ_LIST in deploy/host/setup-sys.sh; the three cannot share a
/// definition because they run on a workstation, an installed appliance,
/// and a factory image respectively. tests/unit/test_setup_sys.sh compares
/// them.
macro_rules! wifi_freq_list {
    () => {
        "5180 5200 5220 5240 5260 5280 5300 5320 5500 5520 5540 5560 5580 5600 5620 5640 5660 \
         5680 5700 5720 5745 5765 5785 5805 5825"
    };
}

pub const WIFI_SCRIPT: &str = concat!(
    "marker=$4\n",
    "found_marker=no\n",
    "while IFS= read -r line; do\n",
    "  if [ \"$line\" = \"$marker\" ]; then found_marker=yes; break; fi\n",
    "done\n",
    "if [ \"$found_marker\" != yes ]; then echo \"Wi-Fi password marker not found\" >&2; exit 11; fi\n",
    "if ! IFS= read -r wifi_password; then echo \"Wi-Fi password not provided\" >&2; exit 11; fi\n",
    "ssid_hex=$1\n",
    "activate=$2\n",
    "preserve=$3\n",
    "ssid_text=$5\n",
    "config_path=/etc/wpa_supplicant/wpa_supplicant.conf\n",
    "secure_saved_config() {\n",
    "  [ -f \"$config_path\" ] && [ ! -L \"$config_path\" ] || { echo 'Unsafe wpa_supplicant configuration path' >&2; exit 15; }\n",
    "  chown root:root \"$config_path\"\n",
    "  chmod 600 \"$config_path\"\n",
    "}\n",
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
    "secure_saved_config\n",
    // Establish the regulatory country before scanning, defaulting to US.
    // Without one the radio is in the world domain, which has no channels
    // 149-165 at all -- so a scan run first cannot see the 5 GHz access
    // points most US sites use, and the operator is offered 2.4 GHz networks
    // only. A country the board already declares is left alone; it is where
    // the appliance actually is, and managing Wi-Fi must not relabel the
    // radio.
    "country=$(wpa_cli -i \"$iface\" get country 2>/dev/null || true)\n",
    "case \"$country\" in\n",
    "  [A-Z][A-Z]) ;;\n",
    "  *) wpa_cli -i \"$iface\" set country US >/dev/null 2>&1 || true;;\n",
    "esac\n",
    // Ask supplicant to narrow the scan where its build supports the global
    // control. Alpine currently returns FAIL for this spelling, so the
    // per-network setting below is the authoritative enforcement point.
    "wpa_cli -i \"$iface\" set freq_list \"",
    wifi_freq_list!(),
    "\" >/dev/null 2>&1 || true\n",
    "wpa_cli -i \"$iface\" scan >/dev/null || true\n",
    "network_id=\n",
    "if [ \"$preserve\" = yes ]; then\n",
    "  network_id=$(wpa_cli -i \"$iface\" list_networks | SSID_TARGET=\"$ssid_text\" awk -F '\\t' 'NR > 1 && $2 == ENVIRON[\"SSID_TARGET\"] { print $1; exit }')\n",
    "fi\n",
    "if [ -z \"$network_id\" ]; then network_id=$(wpa_cli -i \"$iface\" add_network); fi\n",
    "case \"$network_id\" in ''|*[!0-9]*) echo 'Unable to allocate Wi-Fi profile' >&2; exit 13;; esac\n",
    "wpa_cli -i \"$iface\" set_network \"$network_id\" ssid \"$ssid_hex\" | grep -Fxq OK\n",
    "wpa_cli -i \"$iface\" set_network \"$network_id\" key_mgmt WPA-PSK | grep -Fxq OK\n",
    "wpa_cli -i \"$iface\" set_network \"$network_id\" psk \"$wifi_password\" | grep -Fxq OK\n",
    "wpa_cli -i \"$iface\" set_network \"$network_id\" freq_list \"",
    wifi_freq_list!(),
    "\" | grep -Fxq OK\n",
    "unset wifi_password\n",
    "if [ \"$activate\" = no ]; then\n",
    "  wpa_cli -i \"$iface\" disconnect | grep -Fxq OK\n",
    "fi\n",
    "wpa_cli -i \"$iface\" enable_network \"$network_id\" | grep -Fxq OK\n",
    "if [ \"$preserve\" = yes ]; then\n",
    "  wpa_cli -i \"$iface\" save_config | grep -Fxq OK\n",
    "  secure_saved_config\n",
    "fi\n",
    "if [ \"$activate\" = yes ]; then\n",
    "  wpa_cli -i \"$iface\" select_network \"$network_id\" >/dev/null\n",
    "  wpa_cli -i \"$iface\" reassociate >/dev/null\n",
    "fi\n",
    "if [ \"$preserve\" = no ] && [ \"$activate\" = yes ]; then\n",
    "  association_ready=no\n",
    "  attempt=0\n",
    "  while [ \"$attempt\" -lt 30 ]; do\n",
    "    status=$(wpa_cli -i \"$iface\" status 2>/dev/null || true)\n",
    "    status_id=$(printf '%s\\n' \"$status\" | awk -F= '$1 == \"id\" { print $2; exit }')\n",
    "    status_state=$(printf '%s\\n' \"$status\" | awk -F= '$1 == \"wpa_state\" { print $2; exit }')\n",
    "    if [ \"$status_id\" = \"$network_id\" ] && [ \"$status_state\" = COMPLETED ]; then association_ready=yes; break; fi\n",
    "    attempt=$((attempt + 1))\n",
    "    sleep 1\n",
    "  done\n",
    "  if [ \"$association_ready\" != yes ]; then\n",
    "    wpa_cli -i \"$iface\" reconfigure >/dev/null 2>&1 || true\n",
    "    echo 'New Wi-Fi profile did not associate; existing profiles were retained' >&2\n",
    "    exit 14\n",
    "  fi\n",
    "fi\n",
    "if [ \"$preserve\" = no ]; then\n",
    "  for candidate in $(wpa_cli -i \"$iface\" list_networks | awk 'NR > 1 {print $1}'); do\n",
    "    [ \"$candidate\" = \"$network_id\" ] && continue\n",
    "    wpa_cli -i \"$iface\" remove_network \"$candidate\" | grep -Fxq OK\n",
    "  done\n",
    "  wpa_cli -i \"$iface\" save_config | grep -Fxq OK\n",
    "  secure_saved_config\n",
    "fi\n",
    "command -v iw >/dev/null 2>&1 && iw dev \"$iface\" set power_save off || true\n",
);

/// Where one capsule member's bytes come from: compiled in, where nothing
/// can change them between the digest and the upload, or a file in a
/// developer's working tree, which is re-verified once its upload
/// completes.
#[derive(Debug)]
enum Payload {
    Embedded(&'static [u8]),
    File { path: String, fingerprint: String },
}

#[derive(Debug)]
struct Artifact {
    name: String,
    digest: String,
    data: Payload,
}

/// A sudo password and other stdin secrets, wiped when dropped.
#[derive(Default)]
struct Input(Vec<u8>);

impl Drop for Input {
    fn drop(&mut self) {
        wipe_vec(&mut self.0);
    }
}

/// `what`, then ":\n<detail>" unless the detail is only whitespace.
fn fail_with_detail(what: &str, detail: &str) -> Error {
    if detail.trim().is_empty() {
        Error::new(what)
    } else {
        Error::new(format!("{what}:\n{detail}"))
    }
}

pub fn require_success(result: &SshResult, operation: &str) -> Result<()> {
    if result.success() {
        return Ok(());
    }
    Err(fail_with_detail(&format!("{operation} failed"), &result.combined()))
}

pub fn sudo_prefix(c: &Connection) -> &'static str {
    if c.username == "root" {
        ""
    } else if c.sudo_password.nonempty() {
        "sudo -S -p ''"
    } else {
        "sudo -n"
    }
}

pub fn privileged_command(c: &Connection, command: &str) -> String {
    let sudo = sudo_prefix(c);
    if sudo.is_empty() { command.to_owned() } else { format!("{sudo} {command}") }
}

/// Keep authentication and execution in one process so the remaining stdin
/// is delivered to the command after sudo consumes its password.
pub fn privileged_stdin_command(c: &Connection, command: &str) -> String {
    privileged_command(c, command)
}

pub fn privileged_argv_command(c: &Connection, argv: &[&str]) -> String {
    let joined: Vec<String> = argv.iter().map(|a| shell_quote(a)).collect();
    privileged_command(c, &joined.join(" "))
}

/// The sudo password as the remote shell expects it on stdin.
pub fn sudo_input(c: &Connection) -> Vec<u8> {
    let mut out = Vec::new();
    if c.username != "root" && c.sudo_password.nonempty() {
        out.extend_from_slice(c.sudo_password.text().as_bytes());
        out.push(b'\n');
    }
    out
}

fn sudo_input_owned(c: &Connection) -> Input {
    Input(sudo_input(c))
}

// ----------------------------------------------------------------- tooling

/// Reports the deploy account's uid, then bash, sudo, and doas as yes/no.
///
/// doas is reported by whether it can escalate, not by whether it exists.
/// Alpine ships the binary on every image with each rule in /etc/doas.conf
/// commented out, so presence proved nothing. Ask doas instead: a rule that
/// matched but wants a password reports an authentication failure and will
/// succeed once a password is supplied, while an unmatched rule reports that
/// the operation is not permitted.
const TOOLING_PROBE: &str = "id -u; for tool in bash sudo; do if command -v \"$tool\" \
     >/dev/null 2>&1; then echo yes; else echo no; fi; done; if ! command -v doas >/dev/null \
     2>&1; then echo no; elif doas -n true >/dev/null 2>&1; then echo yes; else case \"$(doas -n \
     true 2>&1)\" in *[Aa]uthenticat*|*[Aa]uthoriz*) echo yes ;; *) echo no ;; esac; fi";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostTooling {
    pub uid: String,
    pub has_bash: bool,
    pub has_sudo: bool,
    pub has_doas: bool,
}

/// The n-th line (from 0), as str::lines splits it.
fn nth_line(text: &str, n: usize) -> Option<&str> {
    text.lines().nth(n)
}

pub fn parse_host_tooling(output: &str) -> HostTooling {
    let line = |n| nth_line(output, n).map_or("", str::trim);
    HostTooling {
        uid: line(0).chars().take(31).collect(),
        has_bash: line(1) == "yes",
        has_sudo: line(2) == "yes",
        has_doas: line(3) == "yes",
    }
}

/// How to become root on a host without sudo.
pub fn bootstrap_escalation(tooling: &HostTooling) -> Result<&'static str> {
    if tooling.uid == "0" {
        return Ok("");
    }
    if tooling.has_sudo {
        return Ok("sudo -S -p ''");
    }
    if tooling.has_doas {
        return Ok("doas");
    }
    Err(Error::new(
        "this Raspberry Pi has no sudo, no doas, and the deploy account is not root, so the \
         appliance cannot be bootstrapped remotely. Alpine ships neither by default. Fill the \
         Alpine view's root password (`bootstrap_root_password` in the CLI `--secrets-stdin`) and \
         the first deploy installs them through `su`. Otherwise connect as root, or run `su -c \
         '/bin/sh bootstrap.sh'` once on the Pi with deploy/host/bootstrap.sh copied across.",
    ))
}

pub fn needs_su_bootstrap(tooling: &HostTooling, c: &Connection) -> bool {
    tooling.uid != "0" && !tooling.has_sudo && c.bootstrap_root_password.nonempty()
}

// ----------------------------------------------------------------- capsule

fn tree_payload(root: &str, name: &str) -> Result<Payload> {
    let path = core::secure_relative(root, name)?;
    let fingerprint = sys::file_fingerprint(&path)?;
    Ok(Payload::File { path, fingerprint })
}

/// A single member, for the scripts uploaded outside a staged deployment.
fn capsule_member(capsule: &Capsule, root: Option<&str>, name: &str) -> Result<Payload> {
    if let Some(root) = root {
        return tree_payload(root, name);
    }
    capsule
        .member_named(name)
        .map(|m| Payload::Embedded(m.bytes))
        .ok_or_else(|| Error::new(format!("{name} is not part of the embedded capsule")))
}

/// Every member this deployment uploads, in the order it sends them. A
/// project root replaces the whole capsule rather than any part of it:
/// mixing this binary's host scripts with a working tree's image would
/// deploy a combination nobody built.
fn capsule_artifacts(capsule: &Capsule, o: &DeployOptions) -> Result<Vec<Artifact>> {
    let Some(root) = &o.project_root else {
        return Ok(capsule
            .members
            .iter()
            .map(|m| Artifact {
                name: m.name.to_owned(),
                digest: sha256_hex(m.bytes),
                data: Payload::Embedded(m.bytes),
            })
            .collect());
    };
    let manifest = sys::path_join(&sys::path_join(root, "deploy"), "manifest-v3.txt");
    let names = core::load_manifest(&manifest)?;
    if !names.iter().any(|n| n == IMAGE_MEMBER) {
        return Err(Error::new(format!(
            "deployment artifact manifest does not include {IMAGE_MEMBER}"
        )));
    }
    names
        .into_iter()
        .map(|name| {
            let path = core::secure_relative(root, &name)?;
            let fingerprint = sys::file_fingerprint(&path)?;
            let digest = sha256_file(&path)?;
            Ok(Artifact { name, digest, data: Payload::File { path, fingerprint } })
        })
        .collect()
}

/// What this deployment is about to send, named before it sends any of it.
/// The archive's digest is in the line because it is the one member an
/// operator cannot inspect.
fn capsule_summary(a: &[Artifact], o: &DeployOptions) -> String {
    let origin = match &o.project_root {
        Some(root) => format!("from {root}"),
        None => "embedded in this deployer".to_owned(),
    };
    let mut image = format!("no {IMAGE_MEMBER}");
    for art in a.iter().filter(|art| art.name == IMAGE_MEMBER) {
        let bytes = match &art.data {
            Payload::Embedded(b) => b.len() as u64,
            Payload::File { path, .. } => sys::file_size(path).unwrap_or(0),
        };
        image =
            format!("{IMAGE_MEMBER} {} MiB, sha256 {}", bytes / (1024 * 1024), &art.digest[..12]);
    }
    format!("Deploying the manifest-v3 capsule {origin}: {} files, {image}.", a.len())
}

fn upload_payload(
    s: &mut Session<'_>,
    p: &Payload,
    remote: &str,
    cancel: Option<&Cancel>,
) -> Result<()> {
    match p {
        Payload::Embedded(bytes) => s.upload_bytes(bytes, remote, cancel),
        Payload::File { path, .. } => s.upload_file(path, remote, cancel),
    }
}

/// Whether an artifact is still what it was when its digest was taken.
/// Always true for the embedded capsule: those bytes are part of the running
/// program.
fn artifact_unchanged(a: &Artifact) -> Result<bool> {
    match &a.data {
        Payload::Embedded(_) => Ok(true),
        Payload::File { path, fingerprint } => {
            let now = sys::file_fingerprint(path)?;
            let digest = sha256_file(path)?;
            Ok(now == *fingerprint && digest == a.digest)
        }
    }
}

/// The digest at the start of `sha256sum` output, lower-cased.
pub fn parse_sha256_line(output: &str) -> Option<String> {
    let p = output.trim_start_matches([' ', '\t', '\n', '\r']);
    let n = p.find([' ', '\t', '\r', '\n']).unwrap_or(p.len());
    let digest = &p[..n];
    (digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

// ---------------------------------------------------------------- platform

/// The boards this appliance supports, as device-tree model prefixes. This
/// is the Rust half of the table in deploy/lib/board-profile.sh; the
/// installer refuses the same set on the host itself. Each prefix ends at a
/// word boundary, which is why the Pi 5 entry is not simply matched loosely:
/// "Raspberry Pi 5" is also the start of "Raspberry Pi 500".
///
/// Both entries have a dual-band radio, and that is a support criterion
/// rather than a coincidence: the appliance is 5 GHz only, so a board that
/// cannot leave 2.4 GHz cannot be a host. That removed the Pi Zero 2 W and
/// the Pi 3 tier.
const SUPPORTED_BOARDS: [&str; 2] = ["Raspberry Pi 5", "Raspberry Pi 4 Model B"];

pub fn is_supported_board(model: &str) -> bool {
    SUPPORTED_BOARDS.iter().any(|prefix| {
        model.strip_prefix(prefix).is_some_and(|rest| {
            // A prefix that already ends in a space has consumed its own
            // boundary; otherwise the next character must start a new word.
            prefix.ends_with(' ') || rest.is_empty() || rest.starts_with(' ')
        })
    })
}

pub fn require_supported_appliance(output: &str) -> Result<()> {
    let line = |n| nth_line(output, n);
    let ok = line(0) == Some("aarch64")
        && line(1) == Some("alpine")
        && line(2).is_some_and(|r| r.starts_with("3.24."))
        && line(3).is_some_and(is_supported_board);
    if ok {
        Ok(())
    } else {
        Err(Error::new(
            "remote host must run Alpine Linux 3.24 aarch64 on a Raspberry Pi 5 or Raspberry Pi 4 \
             Model B",
        ))
    }
}

pub fn probed_board(output: &str) -> Option<String> {
    nth_line(output, 3).filter(|m| !m.is_empty()).map(|m| m.chars().take(255).collect())
}

pub fn redact(message: &str, secrets: &[&str]) -> String {
    let mut out = message.to_owned();
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        out = out.replace(secret, "[redacted]");
    }
    out
}

pub fn installer_summary(output: &str) -> Option<String> {
    let start = output.find("=== Installation Complete ===")?;
    let t = output[start..].trim();
    (!t.is_empty()).then(|| t.to_owned())
}

pub fn first_web_password(logs: &str) -> Option<String> {
    let mut rest = logs;
    loop {
        let nl = rest.find('\n')?;
        let banner = rest[..nl].contains("Web UI password (save this now)");
        rest = &rest[nl + 1..];
        if !banner {
            continue;
        }
        let value = nth_line(rest, 0)?.trim();
        if value.is_empty() || value.bytes().all(|b| b == b'=') {
            return None;
        }
        return Some(value.to_owned());
    }
}

/// The rename command: only the uploaded script's path, never the name.
pub fn ssh_rename_command(remote_q: &str) -> String {
    format!("/bin/sh {remote_q}; rc=$?; rm -f -- {remote_q}; exit $rc")
}

// ---------------------------------------------------------------- sessions

fn connect_checked<'c>(c: &Connection, cancel: Option<&'c Cancel>) -> Result<Session<'c>> {
    if let Some(problem) = core::validate_connection(c) {
        return Err(Error::new(problem));
    }
    Session::connect(c, cancel)
}

fn read_boot_id(s: &mut Session<'_>, cancel: Option<&Cancel>) -> Result<String> {
    let r = s.run("cat /proc/sys/kernel/random/boot_id", b"", cancel)?;
    require_success(&r, "Read Linux boot ID")?;
    let t = r.out.trim();
    if t.is_empty() {
        return Err(Error::new("Linux boot ID was empty"));
    }
    Ok(t.to_owned())
}

/// Per connection, because these are different accounts on the same board
/// and they do not have to fail together. One shared flag would let a
/// refusal on one account stand in as proof that the host went away, and
/// the next account to answer would then return a session belonging to the
/// pre-reboot system that is still shutting down.
fn wait_for_reboot<'c>(
    connections: &[&Connection],
    previous_boot_id: &str,
    timeout_ms: u64,
    cancel: Option<&'c Cancel>,
    p: Option<Progress<'_>>,
) -> Result<Session<'c>> {
    report(p, "Waiting for the Raspberry Pi to reboot...");
    let deadline = now_ms() + timeout_ms;
    let mut saw_down = vec![false; connections.len()];
    while now_ms() < deadline {
        sys::check_cancel(cancel)?;
        for (i, c) in connections.iter().enumerate() {
            let Ok(mut s) = connect_checked(c, cancel) else {
                saw_down[i] = true;
                continue;
            };
            match read_boot_id(&mut s, cancel) {
                Ok(id) if id != previous_boot_id => return Ok(s),
                Err(_) if saw_down[i] => return Ok(s),
                _ => {}
            }
        }
        sys::sleep_ms(2000);
    }
    sys::check_cancel(cancel)?;
    Err(Error::new("the Raspberry Pi did not come back after reboot within the wait"))
}

fn wait_for_appliance(
    c: &Connection,
    s: &mut Session<'_>,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    report(p, "Waiting for the OMT appliance to start...");
    let command = privileged_command(c, "docker inspect -f '{{.State.Status}}' omt-client");
    let input = sudo_input_owned(c);
    let deadline = now_ms() + 3 * 60 * 1000;
    while now_ms() < deadline {
        sys::check_cancel(cancel)?;
        let r = s.run(&command, &input.0, cancel)?;
        if r.success() && r.out.trim() == "running" {
            return Ok(());
        }
        sys::sleep_ms(2000);
    }
    Err(Error::new("the OMT appliance container did not reach running state within three minutes"))
}

/// Docker reports the container running as soon as the entrypoint starts,
/// and the entrypoint prints the banner only after it has made the TLS
/// certificate -- seconds later on a Pi 4. So read the logs until either
/// the banner or the server's own start line appears; a redeploy that keeps
/// the stored hash prints no banner at all.
fn fetch_initial_web_password(
    c: &Connection,
    s: &mut Session<'_>,
    cancel: Option<&Cancel>,
) -> Result<Option<String>> {
    let command = privileged_argv_command(c, Action::Logs.argv());
    let input = sudo_input_owned(c);
    let deadline = now_ms() + 60 * 1000;
    loop {
        sys::check_cancel(cancel)?;
        let r = s.run(&command, &input.0, cancel)?;
        if !r.success() {
            return Ok(None);
        }
        let combined = r.combined();
        if let Some(pw) = first_web_password(&combined) {
            return Ok(Some(pw));
        }
        if combined.contains("omt-web listening on") || now_ms() >= deadline {
            return Ok(None);
        }
        sys::sleep_ms(2000);
    }
}

/// Rebuild the ARM64 image in a developer's working tree. Only reachable
/// with a project root: an operator's deployment uploads the archive
/// compiled into this binary and needs no build tooling at all.
fn build_image(o: &DeployOptions, cancel: Option<&Cancel>, p: Option<Progress<'_>>) -> Result<()> {
    if !o.rebuild_image {
        return Ok(());
    }
    let Some(root) = &o.project_root else {
        return Err(Error::new(
            "rebuilding the appliance image needs a project root to build it from",
        ));
    };
    report(p, "Building the ARM64 appliance image...");
    // Windows engines forget their binfmt registration whenever the VM
    // restarts, so it is re-established here; Linux hosts register it
    // persistently through scripts/install-arm64-emulation.sh.
    if sys::ON_WINDOWS {
        tools::ensure_arm64_emulation(cancel, p)?;
    }
    let plan = tools::image_build_plan()?;
    report(p, &format!("Running {}", plan.summary()));
    let args: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    let env: Vec<(&str, &str)> = plan.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let result = sys::run_process(&plan.program, &args, Some(root), &env, cancel)?;
    if result.exit_code != 0 {
        return Err(Error::new(format!("ARM64 image build failed:\n{}", result.output)));
    }
    Ok(())
}

// --------------------------------------------------------------- operations

fn probe_platform(s: &mut Session<'_>, cancel: Option<&Cancel>) -> Result<Option<String>> {
    let r = s.run(PLATFORM_PROBE, b"", cancel)?;
    require_success(&r, "Remote platform probe")?;
    require_supported_appliance(&r.out)?;
    Ok(probed_board(&r.out))
}

pub fn test_connection(
    c: &Connection,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    sys::check_cancel(cancel)?;
    report(p, "Testing SSH connection...");
    let mut s = connect_checked(c, cancel)?;
    match probe_platform(&mut s, cancel)? {
        Some(board) => report(p, &format!("SSH connection succeeded. Detected {board}.")),
        None => report(p, "SSH connection succeeded."),
    }
    Ok(())
}

/// Install bash and sudo when the target is a stock Alpine image: install.sh
/// and transaction.sh are bash scripts invoked through sudo, so on an
/// untouched Alpine host every later step would fail on a missing
/// interpreter.
fn ensure_host_bootstrapped(
    s: &mut Session<'_>,
    c: &Connection,
    capsule: &Capsule,
    project_root: Option<&str>,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    let r = s.run(TOOLING_PROBE, b"", cancel)?;
    require_success(&r, "Remote tooling probe")?;
    let tooling = parse_host_tooling(&r.out);
    if tooling.has_bash && tooling.has_sudo {
        return Ok(());
    }
    report(p, "Bootstrapping bash and sudo on the Raspberry Pi...");
    let pl = capsule_member(capsule, project_root, "deploy/host/bootstrap.sh")?;
    let token = random_token(8)?;
    let remote = format!("/tmp/omt-bootstrap-{token}.sh");
    let remote_q = shell_quote(&remote);
    upload_payload(s, &pl, &remote, cancel)?;
    // /bin/sh explicitly: this is the one script that must run before bash
    // does.
    let r = if needs_su_bootstrap(&tooling, c) {
        // BusyBox su reads from /dev/tty, so a PTY is required. Disable echo
        // before sending the password: channel input may arrive before su
        // has displayed its prompt and must never be reflected into captured
        // logs.
        let inner = shell_quote(&format!("/bin/sh {remote_q}"));
        let command = format!(
            "stty -echo; printf '{BOOTSTRAP_PASSWORD_READY}\\n'; su -c {inner}; rc=$?; stty echo; \
             rm -f -- {remote_q}; exit $rc"
        );
        let mut input = Input(c.bootstrap_root_password.text().as_bytes().to_vec());
        input.0.push(b'\n');
        s.run_pty_after_marker(&command, BOOTSTRAP_PASSWORD_READY, &input.0, cancel)?
    } else {
        let escalation = bootstrap_escalation(&tooling)?;
        let command =
            format!("{escalation} /bin/sh {remote_q}; rc=$?; rm -f -- {remote_q}; exit $rc");
        let input = sudo_input_owned(c);
        s.run(&command, &input.0, cancel)?
    };
    require_success(&r, "Alpine bootstrap")
}

fn password_connection(
    base: &Connection,
    username: &str,
    password: &Secret,
    sudo: Option<&Secret>,
) -> Connection {
    Connection {
        host: base.host.clone(),
        username: username.to_owned(),
        port: base.port,
        auth: Auth::Password,
        password: password.clone(),
        known_hosts_path: base.known_hosts_path.clone(),
        sudo_password: sudo.cloned().unwrap_or_default(),
        ..Connection::default()
    }
}

fn reboot_host(
    s: &mut Session<'_>,
    c: &Connection,
    input: &[u8],
    what: &str,
    cancel: Option<&Cancel>,
) -> Result<()> {
    let command = privileged_argv_command(c, Action::Reboot.argv());
    let r = s.run(&command, input, cancel)?;
    require_success(&r, what)
}

/// The WPA PSK for the settings: a 64-digit hex key as given (lower-cased),
/// otherwise derived from the passphrase.
fn wifi_psk(ssid: &str, password: &Secret) -> Result<Secret> {
    let pw = password.text();
    let result = if pw.len() == 64 && pw.bytes().all(|b| b.is_ascii_hexdigit()) {
        Secret::new(&pw.to_ascii_lowercase())
    } else {
        derive_wpa_psk(ssid, password)
    };
    result.map_err(Error::new)
}

pub fn alpine_setup(
    c: &Connection,
    settings: &AlpineSettings,
    capsule: &Capsule,
    project_root: Option<&str>,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if let Some(problem) =
        core::validate_connection(c).or_else(|| core::validate_alpine_setup(settings))
    {
        return Err(Error::new(problem));
    }
    sys::check_cancel(cancel)?;
    report(p, "Connecting and checking the Raspberry Pi...");
    let mut s = connect_checked(c, cancel)?;
    if let Some(board) = probe_platform(&mut s, cancel)? {
        report(p, &format!("Installing Alpine sys mode on {board}."));
    }
    let pl = capsule_member(capsule, project_root, SETUP_SYS_MEMBER)?;
    let token = random_token(8)?;
    let remote = format!("/tmp/omt-setup-sys-{token}.sh");
    let remote_q = shell_quote(&remote);
    report(p, "Uploading the Alpine sys-setup script...");
    upload_payload(&mut s, &pl, &remote, cancel)?;
    let (ssid_hex, psk) = match &settings.wifi {
        Some(w) => (omt_common::buf::hex(w.ssid.as_bytes()), wifi_psk(&w.ssid, &w.password)?),
        None => (String::new(), Secret::absent()),
    };
    let mut stdin_text = Input(Vec::new());
    for part in [
        settings.hostname.as_str(),
        settings.root_password.text(),
        settings.pi_password.text(),
        ssid_hex.as_str(),
        psk.text(),
    ] {
        stdin_text.0.extend_from_slice(part.as_bytes());
        stdin_text.0.push(b'\n');
    }
    report(p, "Running hostname, DHCP, user, and sys-mode install...");
    let command = format!("/bin/sh {remote_q}; rc=$?; rm -f -- {remote_q}; exit $rc");
    let r = s.run(&command, &stdin_text.0, cancel)?;
    let wifi_password = settings.wifi.as_ref().map_or("", |w| w.password.text());
    let secrets =
        [settings.root_password.text(), settings.pi_password.text(), wifi_password, psk.text()];
    let combined = r.combined();
    if !r.success() {
        return Err(fail_with_detail("Alpine sys install failed", &redact(&combined, &secrets)));
    }
    if !combined.contains(SETUP_SYS_COMPLETE) {
        return Err(Error::new("Alpine sys install finished without the completion marker"));
    }
    report(p, "Alpine sys install finished. Rebooting into the persistent root...");
    let input = sudo_input_owned(c);
    let boot_id = read_boot_id(&mut s, cancel)?;
    reboot_host(&mut s, c, &input.0, "Post-sys-install reboot", cancel)?;
    drop(s);
    let pi = password_connection(c, "pi", &settings.pi_password, Some(&settings.pi_password));
    let root = password_connection(c, "root", &settings.root_password, None);
    drop(wait_for_reboot(&[&pi, &root], &boot_id, 8 * 60 * 1000, cancel, p)?);
    report(
        p,
        "Persistent sys mode is running. Connect as pi with the password you set, then Deploy. \
         The pi account is in wheel; the first deploy installs sudo.",
    );
    Ok(())
}

pub fn manage(
    c: &Connection,
    action: Action,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    sys::check_cancel(cancel)?;
    report(
        p,
        match action {
            Action::Status => "Fetching container status...",
            Action::Logs => "Fetching recent logs...",
            Action::Restart => "Restarting service...",
            Action::Reboot => "Scheduling operating-system reboot...",
        },
    );
    let mut s = connect_checked(c, cancel)?;
    let command = privileged_argv_command(c, action.argv());
    let input = sudo_input_owned(c);
    let r = s.run(&command, &input.0, cancel)?;
    require_success(&r, "Remote management action")?;
    let output = r.combined();
    if !output.trim().is_empty() {
        report(p, &output);
    }
    Ok(())
}

fn connection_secrets(c: &Connection) -> [&str; 3] {
    [c.password.text(), c.key_passphrase.text(), c.sudo_password.text()]
}

pub fn set_hostname(
    c: &Connection,
    hostname: &str,
    capsule: &Capsule,
    project_root: Option<&str>,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if let Some(problem) = core::validate_connection(c) {
        return Err(Error::new(problem));
    }
    if !core::valid_appliance_hostname(hostname) {
        return Err(Error::new(
            "Hostname must be 1-63 characters: letters, digits, and inner hyphens.",
        ));
    }
    sys::check_cancel(cancel)?;
    report(p, "Connecting and uploading the hostname script...");
    let mut s = connect_checked(c, cancel)?;
    let pl = capsule_member(capsule, project_root, SET_HOSTNAME_MEMBER)?;
    let token = random_token(8)?;
    let remote = format!("/tmp/omt-set-hostname-{token}.sh");
    let remote_q = shell_quote(&remote);
    upload_payload(&mut s, &pl, &remote, cancel)?;
    report(p, &format!("Renaming the appliance to {hostname}..."));
    // One privileged process, as with the Wi-Fi and Web-password channels:
    // sudo consumes the first stdin line and the script reads the name from
    // what remains, so the name never appears in a command line or a
    // process listing on the Pi.
    let quoted = format!("sh -c {}", shell_quote(&ssh_rename_command(&remote_q)));
    let command = privileged_stdin_command(c, &quoted);
    let mut input = sudo_input_owned(c);
    input.0.extend_from_slice(hostname.as_bytes());
    input.0.push(b'\n');
    let r = s.run(&command, &input.0, cancel)?;
    let secrets = connection_secrets(c);
    if !r.success() {
        return Err(fail_with_detail("Hostname change failed", &redact(&r.combined(), &secrets)));
    }
    // The script's summary is on stdout. Compose narrates the container's
    // recreation on stderr, which would bury the closing lines.
    let summary = redact(&r.out, &secrets);
    let Some(at) = summary.find(SET_HOSTNAME_COMPLETE) else {
        return Err(Error::new("Hostname change finished without the completion marker"));
    };
    let tail = &summary[at + SET_HOSTNAME_COMPLETE.len()..];
    let piece = tail.find(SET_HOSTNAME_COMPLETE).map_or(tail, |n| &tail[..n]).trim();
    if !piece.is_empty() {
        report(p, piece);
    }
    report(p, &format!("Appliance hostname is now {hostname}."));
    Ok(())
}

pub fn change_web_password(
    c: &Connection,
    password: &Secret,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if let Some(problem) =
        core::validate_connection(c).or_else(|| core::validate_web_password(password))
    {
        return Err(Error::new(problem));
    }
    sys::check_cancel(cancel)?;
    report(p, "Changing Web GUI password and restarting the service...");
    let mut s = connect_checked(c, cancel)?;
    let command = privileged_stdin_command(c, WEB_PASSWORD_COMMAND);
    let mut input = sudo_input_owned(c);
    input.0.extend_from_slice(password.text().as_bytes());
    input.0.push(b'\n');
    let r = s.run(&command, &input.0, cancel)?;
    if !r.success() {
        return Err(fail_with_detail(
            "Web GUI password change failed",
            &redact(&r.combined(), &[password.text()]),
        ));
    }
    report(p, "Web GUI password changed. Existing Web sessions were revoked.");
    Ok(())
}

pub fn apply_wifi(
    c: &Connection,
    settings: &WifiSettings,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if let Some(problem) = core::validate_connection(c).or_else(|| core::validate_wifi(settings)) {
        return Err(Error::new(problem));
    }
    sys::check_cancel(cancel)?;
    report(
        p,
        if settings.connect {
            "Applying Wi-Fi settings and requesting a connection..."
        } else {
            "Saving Wi-Fi profile without connecting..."
        },
    );
    report(
        p,
        if settings.connect {
            "SSH may disconnect if the Raspberry Pi switches networks."
        } else {
            "Immediate Wi-Fi association is paused; Wi-Fi SSH may disconnect."
        },
    );
    if !settings.preserve_existing_profiles {
        report(
            p,
            if settings.connect {
                "Existing Wi-Fi profiles will be removed after the new profile associates."
            } else {
                "Existing Wi-Fi profiles will be removed without testing the new profile; Wi-Fi \
                 SSH may disconnect."
            },
        );
    }
    let psk = wifi_psk(&settings.ssid, &settings.password)?;
    let token = random_token(12)?;
    let marker = format!("__OMT_WIFI_PASSWORD_FOLLOWS_{token}__");
    let ssid_hex = omt_common::buf::hex(settings.ssid.as_bytes());
    let mut input = sudo_input_owned(c);
    input.0.extend_from_slice(marker.as_bytes());
    input.0.push(b'\n');
    input.0.extend_from_slice(psk.text().as_bytes());
    input.0.push(b'\n');
    let yes_no = |b: bool| if b { "yes" } else { "no" };
    let wifi_command = format!(
        "sh -eu -c {} sh {} {} {} {} {}",
        shell_quote(WIFI_SCRIPT),
        shell_quote(&ssid_hex),
        shell_quote(yes_no(settings.connect)),
        shell_quote(yes_no(settings.preserve_existing_profiles)),
        shell_quote(&marker),
        shell_quote(&settings.ssid)
    );
    // Authenticate and execute in one sudo process. Alpine's default sudo
    // timestamp policy may not carry a non-interactive `sudo -v` ticket into
    // a second `sudo -n` invocation, even in the same SSH channel. The first
    // input line is consumed by sudo; the marker and PSK remain available to
    // the privileged shell on the same stdin stream.
    let command = privileged_stdin_command(c, &wifi_command);
    let mut s = connect_checked(c, cancel)?;
    let r = s.run(&command, &input.0, cancel)?;
    if !r.success() {
        let [a, b, d] = connection_secrets(c);
        let secrets = [a, b, d, psk.text(), settings.password.text()];
        return Err(Error::new(format!(
            "Wi-Fi update failed:\n{}",
            redact(&r.combined(), &secrets)
        )));
    }
    report(
        p,
        if settings.connect {
            "Wi-Fi settings applied and connection requested."
        } else {
            "Wi-Fi settings saved."
        },
    );
    Ok(())
}

// ------------------------------------------------------------------ deploy

/// Deliberately uncancellable: this is the failure path, and the operator
/// cancelling is one of the reasons it runs.
fn remove_stage(s: &mut Session<'_>, stage_q: &str) {
    let command = format!(
        "if [ -d {stage_q} ] && [ ! -L {stage_q} ]; then find -P {stage_q} -xdev -depth -delete; fi"
    );
    let _ = s.run(&command, b"", None);
}

fn run_required(
    s: &mut Session<'_>,
    command: &str,
    input: &[u8],
    what: &str,
    cancel: Option<&Cancel>,
) -> Result<()> {
    let r = s.run(command, input, cancel)?;
    require_success(&r, what)
}

struct Staging<'a> {
    stage: &'a str,
    token: &'a str,
    remote_q: &'a str,
}

/// Uploads every manifest member, verifies each one's remote digest, and
/// promotes the set. The caller removes the staging directory if this
/// fails.
fn stage_and_promote(
    s: &mut Session<'_>,
    arts: &[Artifact],
    st: &Staging<'_>,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    for a in arts {
        let name = a.name.as_str();
        sys::check_cancel(cancel)?;
        if let Some(slash) = name.rfind('/')
            && slash != 0
        {
            let parent = format!("{}/{}", st.stage, &name[..slash]);
            let command = format!("mkdir -p -- {}", shell_quote(&parent));
            run_required(s, &command, b"", "Remote staging preparation", cancel)?;
        }
        report(p, &format!("Uploading {name}..."));
        let remote_path = format!("{}/{name}", st.stage);
        upload_payload(s, &a.data, &remote_path, cancel)?;
        if !artifact_unchanged(a)? {
            return Err(Error::new(format!(
                "local deployment artifact changed during upload: {name}"
            )));
        }
        let r = s.run(&format!("sha256sum -- {}", shell_quote(&remote_path)), b"", cancel)?;
        require_success(&r, "Remote checksum")?;
        if parse_sha256_line(&r.out).as_deref() != Some(a.digest.as_str()) {
            return Err(Error::new(format!("SHA-256 mismatch after uploading {name}")));
        }
        report(p, &format!("Verified SHA-256 for {name}."));
    }
    let command = format!(
        "bash {} promote {} {} {}",
        shell_quote(&format!("{}/deploy/transaction.sh", st.stage)),
        st.remote_q,
        shell_quote(st.token),
        shell_quote(&format!("{}/deploy/manifest-v3.txt", st.stage))
    );
    run_required(s, &command, b"", "Deployment promotion", cancel)
}

const EXECUTABLE_PATHS: [&str; 9] = [
    "deploy/host/bootstrap.sh",
    "deploy/host/setup-sys.sh",
    "deploy/host/set-hostname.sh",
    "deploy/host/install.sh",
    "deploy/host/uninstall.sh",
    "deploy/host/host-diagnostics.sh",
    "deploy/host/host-event-watcher.sh",
    "deploy/host/host-reboot.sh",
    "deploy/transaction.sh",
];

pub fn deploy(
    c: &Connection,
    o: &DeployOptions,
    capsule: &Capsule,
    cancel: Option<&Cancel>,
    p: Option<Progress<'_>>,
) -> Result<()> {
    if let Some(problem) = core::validate_connection(c).or_else(|| core::validate_options(o)) {
        return Err(Error::new(problem));
    }
    sys::check_cancel(cancel)?;
    build_image(o, cancel, p)?;
    sys::check_cancel(cancel)?;
    let arts = capsule_artifacts(capsule, o)?;
    report(p, &capsule_summary(&arts, o));

    report(p, "Connecting and checking the Raspberry Pi...");
    let mut s = connect_checked(c, cancel)?;
    if let Some(board) = probe_platform(&mut s, cancel)? {
        report(p, &format!("Deploying to {board}."));
    }
    ensure_host_bootstrapped(&mut s, c, capsule, o.project_root.as_deref(), cancel, p)?;

    let remote_directory = o.remote_directory.trim_end_matches('/');
    let remote_q = shell_quote(remote_directory);
    let input = sudo_input_owned(c);
    let command = privileged_command(
        c,
        &format!("install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" {remote_q}"),
    );
    run_required(&mut s, &command, &input.0, "Remote directory preparation", cancel)?;
    let token = random_token(12)?;
    let staging_root = format!("{remote_directory}/.deploy-staging");
    let stage = format!("{staging_root}/{token}");
    let staging_q = shell_quote(&staging_root);
    let stage_q = shell_quote(&stage);

    let legacy = shell_quote(&format!("{remote_directory}/deploy-transaction.sh"));
    let legacy_manifest = shell_quote(&format!("{remote_directory}/deploy-artifacts.txt"));
    let current = shell_quote(&format!("{remote_directory}/deploy/transaction.sh"));
    let command = format!(
        "if [ -x {legacy} ] && [ -f {legacy_manifest} ]; then {legacy} recover {remote_q} \
         {legacy_manifest}; fi; if [ -x {current} ]; then {current} recover {remote_q}; fi"
    );
    run_required(&mut s, &command, b"", "Interrupted deployment recovery", cancel)?;
    let command = format!(
        "if [ -L {staging_q} ] || {{ [ -e {staging_q} ] && [ ! -d {staging_q} ]; }}; then exit \
         14; fi; install -d -m 700 -- {staging_q}; mkdir -- {stage_q}"
    );
    run_required(&mut s, &command, b"", "Remote staging root validation", cancel)?;
    // One owner for the staging directory: every failure between here and a
    // completed promotion removes it, a transport error included.
    let st = Staging { stage: &stage, token: &token, remote_q: &remote_q };
    if let Err(e) = stage_and_promote(&mut s, &arts, &st, cancel, p) {
        remove_stage(&mut s, &stage_q);
        return Err(e);
    }

    let chmod = std::iter::once("chmod +x".to_owned())
        .chain(
            EXECUTABLE_PATHS.iter().map(|path| shell_quote(&format!("{remote_directory}/{path}"))),
        )
        .collect::<Vec<_>>()
        .join(" ");
    let installer = shell_quote(&format!("{remote_directory}/deploy/host/install.sh"));
    let install_script = shell_quote(&format!("printf 'n\\n' | {installer}"));
    let inner = privileged_command(c, &format!("sh -c {install_script}"));
    let r = s.run(&format!("{chmod} && {inner}"), &input.0, cancel)?;
    require_success(&r, "Remote installer")?;
    // Whether the installer started the container itself rather than leaving
    // it for the reboot: only then can the first-start banner already
    // exist.
    let started_before_reboot = !contains_bytes(r.out.as_bytes(), "Container startup deferred");
    // The installer prints its operator summary on stdout. OpenRC and
    // Compose warnings arrive on stderr and are deliberately excluded.
    if let Some(summary) = installer_summary(&r.out) {
        let [a, b, d] = connection_secrets(c);
        report(p, &redact(&summary, &[a, b, d, c.bootstrap_root_password.text()]));
    }
    // A host whose KMS is already live -- any reinstall -- starts the
    // container during the install, so a newly generated password is
    // printed now, by a container the boot-time service start then
    // recreates. Its logs go with it, so the banner has to be read before
    // the reboot or never.
    let mut password =
        if started_before_reboot { fetch_initial_web_password(c, &mut s, cancel)? } else { None };
    report(p, "Rebooting to apply kernel, firmware, and KMS settings...");
    let boot_id = read_boot_id(&mut s, cancel)?;
    reboot_host(&mut s, c, &input.0, "Post-install reboot", cancel)?;
    drop(s);
    let mut s = wait_for_reboot(&[c], &boot_id, 6 * 60 * 1000, cancel, p)?;
    match wait_for_appliance(c, &mut s, cancel, p) {
        Ok(()) => {
            if password.is_none() {
                password = fetch_initial_web_password(c, &mut s, cancel)?;
            }
            if let Some(pw) = &password {
                report(p, &format!("Web UI password (save this now): {pw}"));
            }
            report(p, "Appliance is running.");
        }
        Err(wait_error) => report(
            p,
            &format!(
                "Install succeeded, but the appliance did not start within the wait: {wait_error}. \
                 Check `sudo rc-service omt-client status` on the Pi."
            ),
        ),
    }
    report(p, "Deployment complete.");
    Ok(())
}
