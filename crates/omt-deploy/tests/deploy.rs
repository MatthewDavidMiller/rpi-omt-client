//! The deployer core, ported from tests/c/test_deploy.c: validation,
//! secrets, manifests, processes, the remote operations' fixed commands,
//! the SSH helpers, workstation tooling, jobs, the SD card, and the capsule.

#![allow(clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use omt_common::buf::hex;
use omt_deploy::capsule::{Capsule, Member};
use omt_deploy::core::{self, Action, Connection, SdSettings, Secret};
use omt_deploy::jobs::{ConnectionFields, connection_from_fields};
use omt_deploy::ops::{self, HostTooling};
use omt_deploy::sd_card;
use omt_deploy::ssh::{self, SshResult, channel, wire};
use omt_deploy::sys::{self, OUTPUT_LIMIT, ProcessResult, run_process};
use omt_deploy::tools::{
    self, DOCKER_DESKTOP, GIT_FOR_WINDOWS, Prerequisite, WINGET_ALREADY_INSTALLED,
};

fn secret(text: &str) -> Secret {
    Secret::new(text).unwrap()
}

fn connection(user: &str) -> Connection {
    let mut c = Connection::new();
    c.host = "pi.local".into();
    c.username = user.into();
    c
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("omt-deploy-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------- core

#[test]
fn validation_contract() {
    assert!(core::valid_host("pi.local"));
    assert!(!core::valid_host("-pi.local"));
    assert!(core::valid_username("pi_admin-1"));
    assert!(core::valid_appliance_hostname("omt-client"));
    assert!(core::valid_appliance_hostname("rpi5"));
    assert!(!core::valid_appliance_hostname("-pi"));
    assert!(!core::valid_appliance_hostname("omt_client"));
    assert!(!core::valid_appliance_hostname("omt.client"));
    assert!(core::valid_remote_directory("/opt/omt-client"));

    let mut empty_ssh = Connection::new();
    empty_ssh.host = "10.1.20.210".into();
    empty_ssh.username = "root".into();
    empty_ssh.password = secret("");
    assert_eq!(core::validate_connection(&empty_ssh), None);

    assert_eq!(core::validate_os_password(&secret("alpinepw")), None);
    assert!(core::validate_os_password(&secret("short")).is_some());
    assert_eq!(core::validate_web_password(&secret("correct horse battery staple")), None);
    assert!(core::validate_web_password(&secret("too-short")).is_some());

    assert_eq!(core::shell_quote("a'b"), "'a'\\''b'");
}

#[test]
fn secrets_refuse_controls_and_oversize() {
    assert!(Secret::new("a\x07b").is_err());
    assert!(Secret::new("tab\tinside").is_err());
    assert!(Secret::new("\u{85}").is_err()); // a C1 control
    let big = "a".repeat(4097);
    assert!(Secret::new(&big[..4096]).is_ok());
    assert!(Secret::new(&big).is_err());
    assert!(Secret::new("caf\u{e9}").is_ok());
    // Never set is different from set to "".
    assert!(!Secret::absent().is_set());
    assert!(secret("").is_set());
}

#[test]
fn restart_manages_the_service_even_before_first_container_creation() {
    assert_eq!(Action::Restart.argv(), ["rc-service", "omt-client", "restart"]);
}

#[test]
fn reboot_is_a_fixed_deferred_host_action() {
    assert_eq!(
        Action::Reboot.argv(),
        ["sh", "-c", "nohup sh -c 'sleep 1; /sbin/reboot' </dev/null >/dev/null 2>&1 &"]
    );
}

#[test]
fn hex_is_lower_case_and_fixed_width() {
    assert_eq!(hex(b""), "");
    assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    assert_eq!(hex(b"studio"), "73747564696f");
}

#[test]
fn psk_vector() {
    let psk = core::derive_wpa_psk("IEEE", &secret("password")).unwrap();
    assert_eq!(psk.text(), "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
}

#[test]
fn manifest_requires_transaction_members() {
    let dir = scratch("manifest");
    let path = dir.join("manifest-v3.txt");
    let path_text = path.to_str().unwrap();
    fs::write(&path, "version=3\nLICENSE\n").unwrap();
    assert!(core::load_manifest(path_text).is_err());
    fs::write(&path, "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n").unwrap();
    assert_eq!(core::load_manifest(path_text).unwrap().len(), 2);

    let dup = "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\nLICENSE\nLICENSE\n";
    assert_eq!(
        core::parse_manifest(dup).unwrap_err().message(),
        "unsafe or duplicate manifest member"
    );
    let unsafe_member = "version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n../etc\n";
    assert!(core::parse_manifest(unsafe_member).is_err());
    let crlf = "version=3\r\ndeploy/transaction.sh\r\ndeploy/manifest-v3.txt\r\n";
    assert!(core::parse_manifest(crlf).is_ok());
    assert_eq!(core::parse_manifest("version=2\n").unwrap_err().message(), "unsupported manifest");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn manifest_names_are_safe() {
    assert!(core::valid_manifest_name("deploy/host/install.sh"));
    for bad in ["/etc/passwd", "a//b", "a/./b", "a/../b", "trailing/", "sp ace", ""] {
        assert!(!core::valid_manifest_name(bad), "accepted {bad}");
    }
    for bad in ["opt/omt-client", "/opt/omt-client/", "/opt/../etc", "/"] {
        assert!(!core::valid_remote_directory(bad), "accepted {bad}");
    }
}

#[test]
fn cancelled_process_is_not_spawned() {
    let cancel = AtomicBool::new(true);
    let e = run_process("/nonexistent/omt-command-test", &[], Some("."), &[], Some(&cancel))
        .unwrap_err();
    assert_eq!(e.message(), "operation cancelled");
}

#[test]
fn process_captures_both_streams_and_nonzero_exit() {
    let r = run_process(
        "/bin/sh",
        &["-c", "printf out; printf err >&2; exit 7"],
        Some("/tmp"),
        &[],
        None,
    )
    .unwrap();
    assert_eq!(r.exit_code, 7);
    assert_eq!(r.output, "outerr");

    // The environment additions reach the child, and the directory is used.
    let r = run_process(
        "/bin/sh",
        &["-c", "printf '%s:' \"$OMT_PLAN\"; pwd"],
        Some("/"),
        &[("OMT_PLAN", "yes")],
        None,
    )
    .unwrap();
    assert_eq!(r.output, "yes:/\n");

    let e = run_process("/nonexistent/omt-tool", &[], Some("."), &[], None).unwrap_err();
    assert!(e.message().contains("cannot run /nonexistent/omt-tool"), "{}", e.message());
}

#[test]
fn cancellation_remains_live_after_the_direct_child_exits() {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let setter = thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            thread::sleep(Duration::from_millis(100));
            flag.store(true, Ordering::Relaxed);
        })
        .unwrap();
    let started = Instant::now();
    let e = run_process("/bin/sh", &["-c", "sleep 2 & exit 0"], Some("/tmp"), &[], Some(&cancel))
        .unwrap_err();
    assert_eq!(e.message(), "operation cancelled");
    assert!(started.elapsed() < Duration::from_secs(1));
    setter.join().unwrap();
}

#[test]
fn output_limit_keeps_the_first_four_mib() {
    let r = run_process(
        "/bin/sh",
        &["-c", "head -c 4202496 /dev/zero | tr '\\0' '*'"],
        Some("/tmp"),
        &[],
        None,
    )
    .unwrap();
    assert_eq!(r.exit_code, 0);
    assert_eq!(r.output.len(), OUTPUT_LIMIT);
}

// -------------------------------------------------------------------- ops

fn probe(model: &str) -> String {
    format!("aarch64\nalpine\n3.24.2\n{model}\n")
}

/// The same matrix as tests/unit/test_board_profile.sh: the two gates run
/// on different machines, so a board either passes both or fails halfway.
#[test]
fn accepts_every_supported_board() {
    for model in ["Raspberry Pi 5 Model B Rev 1.0", "Raspberry Pi 4 Model B Rev 1.4"] {
        assert!(ops::require_supported_appliance(&probe(model)).is_ok(), "rejected {model}");
    }
}

#[test]
fn refuses_unsupported_boards_and_near_misses() {
    for model in [
        "Raspberry Pi 500 Rev 1.0",
        "Raspberry Pi 400 Rev 1.0",
        "Raspberry Pi 2 Model B Rev 1.1",
        "Raspberry Pi 3 Model B Rev 1.2",
        "Raspberry Pi 3 Model B Plus Rev 1.3",
        "Raspberry Pi 3 Model A Plus Rev 1.0",
        "Raspberry Pi Zero 2 W Rev 1.0",
        "Raspberry Pi Zero 2 Rev 1.0",
        "Raspberry Pi Zero W Rev 1.1",
        "Raspberry Pi Model B Plus Rev 1.2",
        "Raspberry Pi Compute Module 4 Rev 1.0",
        "Raspberry Pi Compute Module 5 Rev 1.0",
        "Orange Pi 5",
        "",
    ] {
        assert!(ops::require_supported_appliance(&probe(model)).is_err(), "accepted {model}");
    }
}

#[test]
fn still_refuses_the_wrong_architecture_or_distribution() {
    for output in [
        "armv7l\nalpine\n3.24.2\nRaspberry Pi 5 Model B Rev 1.0\n",
        "aarch64\ndebian\n3.24.2\nRaspberry Pi 5 Model B Rev 1.0\n",
        "aarch64\nalpine\n3.22.1\nRaspberry Pi 5 Model B Rev 1.0\n",
        // 3.23 was the previously pinned series; package names moved in 3.24.
        "aarch64\nalpine\n3.23.5\nRaspberry Pi 5 Model B Rev 1.0\n",
        "aarch64\nalpine\n3.24.2\n",
    ] {
        let e = ops::require_supported_appliance(output).unwrap_err();
        assert_eq!(
            e.message(),
            "remote host must run Alpine Linux 3.24 aarch64 on a Raspberry Pi 5 or Raspberry Pi 4 \
             Model B",
            "{output}"
        );
    }
}

fn tooling(uid: &str, bash: bool, sudo: bool, doas: bool) -> HostTooling {
    HostTooling { uid: uid.into(), has_bash: bash, has_sudo: sudo, has_doas: doas }
}

#[test]
fn picks_an_escalation_for_each_stock_alpine_shape() {
    assert_eq!(ops::bootstrap_escalation(&tooling("0", false, false, false)).unwrap(), "");
    assert_eq!(
        ops::bootstrap_escalation(&tooling("1000", false, true, false)).unwrap(),
        "sudo -S -p ''"
    );
    assert_eq!(ops::bootstrap_escalation(&tooling("1000", false, false, true)).unwrap(), "doas");
    let e = ops::bootstrap_escalation(&tooling("1000", false, false, false)).unwrap_err();
    assert!(e.message().contains("bootstrap_root_password"));
}

#[test]
fn parses_the_tooling_probe() {
    assert_eq!(ops::parse_host_tooling("1000\nno\nno\nyes\n"), tooling("1000", false, false, true));
    assert_eq!(ops::parse_host_tooling(" 0 \r\nyes\r\nyes\r\nno"), tooling("0", true, true, false));
}

#[test]
fn reports_the_detected_board_for_progress() {
    assert_eq!(
        ops::probed_board(&probe("Raspberry Pi 4 Model B Rev 1.4")).as_deref(),
        Some("Raspberry Pi 4 Model B Rev 1.4")
    );
    assert_eq!(ops::probed_board("aarch64\nalpine\n3.24.2\n"), None);
}

#[test]
fn digest_parser_accepts_sha256sum_output() {
    let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert_eq!(ops::parse_sha256_line(&format!("{digest}  file\n")).as_deref(), Some(digest));
    assert_eq!(
        ops::parse_sha256_line(
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef  f"
        )
        .as_deref(),
        Some(digest)
    );
    assert_eq!(ops::parse_sha256_line("not-a-digest"), None);
}

#[test]
fn redaction_avoids_infinite_loops_on_marker_substrings() {
    let out = ops::redact("secret [redacted] secret", &["secret", "eda"]);
    assert!(!out.contains("secret"));
    assert!(out.contains("[redacted]"));
    assert_eq!(ops::redact("unchanged", &[""]), "unchanged");
}

#[test]
fn successful_installs_surface_only_the_final_summary() {
    assert_eq!(
        ops::installer_summary(
            "apk noise\n=== Installation Complete ===\nWeb UI: https://pi:5000\n"
        )
        .as_deref(),
        Some("=== Installation Complete ===\nWeb UI: https://pi:5000")
    );
    assert_eq!(ops::installer_summary("apk noise only"), None);
}

#[test]
fn sudo_input_is_newline_terminated_and_prefixes_follow_the_account() {
    let mut c = connection("pi");
    c.sudo_password = secret("hunter2");
    assert_eq!(ops::sudo_input(&c), b"hunter2\n");
    assert_eq!(ops::sudo_prefix(&c), "sudo -S -p ''");
    assert_eq!(ops::privileged_command(&c, "docker ps"), "sudo -S -p '' docker ps");
    assert_eq!(ops::privileged_stdin_command(&c, "sh wifi-update"), "sudo -S -p '' sh wifi-update");

    // No password means passwordless sudo: nothing on stdin, and the
    // non-interactive prefix. An empty one is the same as none.
    c.sudo_password = Secret::absent();
    assert!(ops::sudo_input(&c).is_empty());
    assert_eq!(ops::sudo_prefix(&c), "sudo -n");
    c.sudo_password = secret("");
    assert!(ops::sudo_input(&c).is_empty());
    assert_eq!(ops::sudo_prefix(&c), "sudo -n");

    // A root session runs the fixed command directly.
    c.username = "root".into();
    c.sudo_password = secret("unused");
    assert!(ops::sudo_input(&c).is_empty());
    assert_eq!(ops::sudo_prefix(&c), "");
    assert_eq!(ops::privileged_command(&c, "docker ps"), "docker ps");
}

#[test]
fn web_password_action_uses_only_a_fixed_stdin_command() {
    assert_eq!(
        ops::WEB_PASSWORD_COMMAND,
        "sh -eu -c 'docker exec -i omt-client /usr/local/bin/omt-web set-password && rc-service \
         omt-client restart'"
    );
    assert!(!ops::WEB_PASSWORD_COMMAND.contains("$1"));
    assert!(!ops::WEB_PASSWORD_COMMAND.contains("printf"));
}

#[test]
fn the_rename_command_names_only_the_uploaded_script() {
    let text = ops::ssh_rename_command(&core::shell_quote("/tmp/omt-set-hostname-0123456789ab.sh"));
    assert!(text.contains("/tmp/omt-set-hostname-0123456789ab.sh"));
    assert!(!text.contains("studio-pi-2"));
    assert!(text.contains("rm -f --"));
    assert!(text.ends_with("exit $rc"));
}

#[test]
fn untouched_alpine_can_bootstrap_through_su_only_with_a_root_secret() {
    let mut t = tooling("1000", false, false, false);
    let mut c = connection("pi");
    c.password = secret("ssh-password");
    c.sudo_password = secret("user-password");
    assert!(!ops::needs_su_bootstrap(&t, &c));
    c.bootstrap_root_password = secret("root-password");
    assert!(ops::needs_su_bootstrap(&t, &c));
    // Stock Alpine's doas can look usable to the probe and then refuse for
    // want of a TTY; a supplied root secret makes su authoritative.
    t.has_doas = true;
    assert!(ops::needs_su_bootstrap(&t, &c));
    t.uid = "0".into();
    assert!(!ops::needs_su_bootstrap(&t, &c));
}

fn find(haystack: &str, needle: &str) -> usize {
    haystack.find(needle).unwrap_or_else(|| panic!("missing: {needle}"))
}

#[test]
fn wifi_discovers_the_wireless_interface_instead_of_hardcoding_wlan0() {
    let s = ops::WIFI_SCRIPT;
    find(s, "iface=${path#/sys/class/net/}");
    find(s, "wpa_cli -i \"$iface\" ping");
    find(s, "iw dev \"$iface\" set power_save off");
    find(s, "set_network \"$network_id\" freq_list");
    assert!(!s.contains("wpa_cli -i wlan0"));
}

#[test]
fn wifi_profile_replacement_is_verified_or_explicitly_deferred() {
    let s = ops::WIFI_SCRIPT;
    let disconnect = find(s, "wpa_cli -i \"$iface\" disconnect");
    let enable = find(s, "wpa_cli -i \"$iface\" enable_network");
    let completed = find(s, "status_state\" = COMPLETED");
    let removal = find(s, "wpa_cli -i \"$iface\" remove_network");
    assert!(disconnect < enable);
    assert!(completed < removal);
    find(s, "if [ \"$preserve\" = no ] && [ \"$activate\" = yes ]; then");
    find(s, "New Wi-Fi profile did not associate; existing profiles were retained");
    find(s, "wpa_cli -i \"$iface\" reconfigure");
    assert_eq!(s.matches("secure_saved_config\n").count(), 3);
    find(s, "chmod 600 \"$config_path\"");
    find(s, "chown root:root \"$config_path\"");
}

#[test]
fn wifi_defaults_the_regulatory_country_to_us_before_scanning() {
    let s = ops::WIFI_SCRIPT;
    let country = find(s, "set country US");
    let scan = find(s, "wpa_cli -i \"$iface\" scan");
    assert!(country < scan);
    find(s, "get country");
    find(s, "[A-Z][A-Z]) ;;");
}

#[test]
fn wifi_update_script_is_valid_posix_shell() {
    let dir = scratch("wifi");
    let path = dir.join("wifi.sh");
    fs::write(&path, ops::WIFI_SCRIPT).unwrap();
    let r =
        run_process("/bin/sh", &["-n", path.to_str().unwrap()], Some("/tmp"), &[], None).unwrap();
    assert_eq!(r.exit_code, 0, "{}", r.output);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn first_web_password_reads_the_entrypoint_banner() {
    let logs = "============================================\n \
                Web UI password (save this now):\n \
                hunter2-web\n\
                ============================================\n\
                omt-web listening on https://0.0.0.0:5000\n";
    assert_eq!(ops::first_web_password(logs).as_deref(), Some("hunter2-web"));
    assert_eq!(ops::first_web_password("no password here"), None);
    assert_eq!(ops::first_web_password(" Web UI password (save this now):\n=====\n"), None);
}

#[test]
fn post_install_reboot_is_the_same_fixed_action_as_manage() {
    let mut c = connection("pi");
    c.sudo_password = secret("hunter2");
    let argv = Action::Reboot.argv();
    let joined = argv.iter().map(|a| core::shell_quote(a)).collect::<Vec<_>>().join(" ");
    assert_eq!(ops::privileged_argv_command(&c, argv), ops::privileged_command(&c, &joined));
}

#[test]
fn alpine_sys_setup_uses_the_fixed_script_and_marker() {
    assert_eq!(ops::SETUP_SYS_MEMBER, "deploy/host/setup-sys.sh");
    assert!(ops::SETUP_SYS_COMPLETE.contains("Alpine sys install complete"));
}

#[test]
fn require_success_quotes_the_remote_output() {
    let mut r = SshResult::default();
    assert!(ops::require_success(&r, "Probe").is_ok());
    r.exit_code = 3;
    assert_eq!(ops::require_success(&r, "Probe").unwrap_err().message(), "Probe failed");
    r.out = "partial".into();
    r.err = "denied\n".into();
    assert_eq!(
        ops::require_success(&r, "Probe").unwrap_err().message(),
        "Probe failed:\npartial\ndenied\n"
    );
}

// -------------------------------------------------------------------- ssh

#[test]
fn an_explicit_known_hosts_path_overrides_the_home_default() {
    let mut c = connection("pi");
    c.known_hosts_path = Some("/trusted/known_hosts".into());
    assert_eq!(ssh::known_hosts_path(&c).unwrap(), "/trusted/known_hosts");
    // The environment is not changed from a test (it is process-wide); the
    // default is checked against whatever HOME this run has.
    c.known_hosts_path = None;
    if let Some(home) = sys::getenv("HOME").filter(|h| !h.is_empty()) {
        assert_eq!(ssh::known_hosts_path(&c).unwrap(), format!("{home}/.ssh/known_hosts"));
    }
}

#[test]
fn shell_upload_quotes_the_remote_path() {
    assert_eq!(
        channel::shell_upload_command("/tmp/omt-setup-sys-abcd.sh"),
        "umask 077 && cat > '/tmp/omt-setup-sys-abcd.sh'"
    );
    assert_eq!(
        channel::shell_upload_command("/tmp/o'mt.sh"),
        "umask 077 && cat > '/tmp/o'\\''mt.sh'"
    );
}

#[test]
fn readiness_markers_are_found_only_as_complete_byte_sequences() {
    assert!(wire::contains_bytes(b"before\nready\r\nafter", "ready"));
    assert!(!wire::contains_bytes(b"rea", "ready"));
    assert!(!wire::contains_bytes(b"anything", ""));
}

#[test]
fn combined_output_separates_the_streams_with_one_newline() {
    let mut r = SshResult { out: "out".into(), err: "err".into(), ..SshResult::default() };
    assert_eq!(r.combined(), "out\nerr");
    r.out = "line\n".into();
    assert_eq!(r.combined(), "line\nerr");
}

// ------------------------------------------------------------------ tools

#[test]
fn windows_builds_through_bash_and_never_needs_make() {
    let plan = tools::plan_from(None, Some("C:\\Program Files\\Git\\bin\\bash.exe"), true).unwrap();
    assert_eq!(plan.program, "C:\\Program Files\\Git\\bin\\bash.exe");
    assert_eq!(plan.args, ["scripts/build-arm64.sh"]);
    assert_eq!(plan.env, [("ARM64_TARBALL".to_owned(), "omt-client-arm64.tar.gz".to_owned())]);
    // GNU Make on Windows hands the script to cmd.exe, so it is not a
    // substitute for the shell.
    let e = tools::plan_from(Some("C:\\make.exe"), None, true).unwrap_err();
    assert!(e.message().contains("Git for Windows"));
    assert!(e.message().contains("embedded in this deployer"));
    let plan =
        tools::plan_from(Some("C:\\make.exe"), Some("C:\\Git\\bin\\bash.exe"), true).unwrap();
    assert_eq!(plan.program, "C:\\Git\\bin\\bash.exe");
}

#[test]
fn unix_prefers_make_and_falls_back_to_the_script() {
    let plan = tools::plan_from(Some("/usr/bin/make"), Some("/bin/bash"), false).unwrap();
    assert_eq!(plan.program, "/usr/bin/make");
    assert_eq!(plan.args, ["build-arm64", "ARM64_TARBALL=omt-client-arm64.tar.gz"]);
    assert!(plan.env.is_empty());
    assert_eq!(plan.summary(), "/usr/bin/make build-arm64 ARM64_TARBALL=omt-client-arm64.tar.gz");
    let plan = tools::plan_from(None, Some("/bin/bash"), false).unwrap();
    assert_eq!(plan.args, ["scripts/build-arm64.sh"]);
    assert_eq!(plan.env[0].0, "ARM64_TARBALL");
    assert!(tools::plan_from(None, None, false).is_err());
}

#[test]
fn windows_executable_names_follow_pathext() {
    let s = tools::executable_suffixes(true, Some(".COM;.EXE;.BAT;.CMD;.VBS"));
    assert_eq!(s[0], "");
    assert!(s.iter().any(|x| x == ".EXE"));
    assert!(s.iter().any(|x| x == ".CMD"));
    assert_eq!(tools::executable_suffixes(false, Some(".EXE")).len(), 1);
    for broken in [None, Some(""), Some(";;"), Some("exe"), Some(". exe")] {
        assert!(tools::executable_suffixes(true, broken).iter().any(|x| x == ".EXE"), "{broken:?}");
    }
    assert_eq!(tools::executable_suffixes(true, Some(".EXE;.exe;.EXE")).len(), 2);
}

#[test]
fn the_wsl_launcher_is_not_a_build_shell() {
    assert!(tools::is_wsl_launcher("C:\\Windows\\System32\\bash.exe"));
    assert!(tools::is_wsl_launcher("c:\\windows\\sysnative\\bash.exe"));
    assert!(!tools::is_wsl_launcher("C:\\Program Files\\Git\\bin\\bash.exe"));
    assert!(!tools::is_wsl_launcher("/bin/bash"));
}

fn fake_env(mode: u8, name: &str) -> Option<String> {
    match name {
        "ProgramFiles" if mode == 0 => Some("C:\\Program Files".into()),
        "LOCALAPPDATA" if mode == 0 => Some("C:\\Users\\op\\AppData\\Local".into()),
        "SystemDrive" if mode != 1 => Some("C:".into()),
        _ => None,
    }
}

#[test]
fn git_for_windows_is_found_where_it_installs() {
    let c = tools::windows_bash_candidates(&|n| fake_env(0, n));
    // Joined with this host's separator.
    for expected in [
        "C:\\Program Files/Git/bin/bash.exe",
        "C:\\Users\\op\\AppData\\Local/Programs/Git/bin/bash.exe",
        "C:\\/msys64/usr/bin/bash.exe",
    ] {
        assert!(c.iter().any(|x| x == expected), "missing {expected}: {c:?}");
    }
    assert!(c.len() > 4);
    let none = tools::windows_bash_candidates(&|n| fake_env(1, n));
    let drive_only = tools::windows_bash_candidates(&|n| fake_env(2, n));
    assert_eq!(none, drive_only);
}

fn row(required: bool, satisfied: bool, package: Option<&'static tools::Package>) -> Prerequisite {
    Prerequisite {
        name: "row",
        purpose: "purpose",
        required,
        satisfied,
        detail: String::new(),
        remedy: String::new(),
        package,
    }
}

#[test]
fn winget_packages_come_from_unsatisfied_rows_only() {
    let rows = [
        row(true, true, Some(&DOCKER_DESKTOP)),
        row(true, false, Some(&GIT_FOR_WINDOWS)),
        row(true, false, Some(&GIT_FOR_WINDOWS)),
        row(true, false, None),
    ];
    let out = tools::missing_packages(&rows);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, GIT_FOR_WINDOWS.id);
    assert!(tools::missing_packages(&[]).is_empty());
}

#[test]
fn a_missing_required_row_blocks_and_an_optional_one_does_not() {
    assert!(row(true, false, None).blocking());
    assert!(!row(true, true, None).blocking());
    assert!(!row(false, false, None).blocking());
}

#[test]
fn an_already_installed_package_is_not_an_installation_failure() {
    let lines = std::sync::atomic::AtomicUsize::new(0);
    let progress = |_: &str| {
        lines.fetch_add(1, Ordering::Relaxed);
    };
    let mut r = ProcessResult::default();
    assert!(tools::report_install(&GIT_FOR_WINDOWS, &r, Some(&progress)).is_ok());
    r.exit_code = WINGET_ALREADY_INSTALLED;
    assert!(tools::report_install(&GIT_FOR_WINDOWS, &r, Some(&progress)).is_ok());
    r.exit_code = 1;
    assert!(tools::report_install(&GIT_FOR_WINDOWS, &r, Some(&progress)).is_err());
    assert_eq!(lines.load(Ordering::Relaxed), 2);
    assert_eq!(WINGET_ALREADY_INSTALLED, -1_978_335_189);
}

fn row_named<'a>(rows: &'a [Prerequisite], name: &str) -> Option<&'a Prerequisite> {
    rows.iter().find(|r| r.name == name)
}

#[test]
fn probing_this_workstation_reports_the_project_it_was_given() {
    let cancel = AtomicBool::new(false);
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let rows = tools::probe_prerequisites(Some(root), &Capsule::EMPTY, Some(&cancel));
    assert!(row_named(&rows, "Project source tree").unwrap().satisfied);
    assert!(row_named(&rows, "Container engine").is_some());
    assert!(row_named(&rows, "POSIX shell").is_some());
    let rows = tools::probe_prerequisites(
        Some("/nonexistent-omt-project"),
        &Capsule::EMPTY,
        Some(&cancel),
    );
    assert!(row_named(&rows, "Project source tree").unwrap().blocking());
}

/// The fixture capsule the 2.x suites linked: four members, the image a two-byte gzip
/// magic and a line of text.
static FIXTURE: Capsule = Capsule {
    members: &[
        Member { name: "omt-client-arm64.tar.gz", bytes: b"\x1f\x8bfixture image\n" },
        Member {
            name: "deploy/host/set-hostname.sh",
            bytes: b"#!/bin/sh\necho \"=== Appliance hostname set ===\"\n",
        },
        Member { name: "deploy/transaction.sh", bytes: b"#!/bin/bash\n" },
        Member {
            name: "deploy/manifest-v3.txt",
            bytes: b"version=3\nomt-client-arm64.tar.gz\ndeploy/host/set-hostname.sh\n\
                     deploy/transaction.sh\ndeploy/manifest-v3.txt\n",
        },
    ],
    license: "MIT License\n",
    notices: "THIRD-PARTY NOTICES\n",
};

#[test]
fn an_embedded_deployment_needs_nothing_from_this_workstation() {
    let cancel = AtomicBool::new(false);
    let rows = tools::probe_prerequisites(None, &FIXTURE, Some(&cancel));
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r.satisfied));
    assert!(row_named(&rows, "Container engine").is_none());
    assert!(row_named(&rows, "Project source tree").is_none());
    assert!(tools::missing_packages(&rows).is_empty());
}

#[test]
fn installation_is_refused_where_there_is_no_winget() {
    let cancel = AtomicBool::new(false);
    let e = tools::install_packages(&[&GIT_FOR_WINDOWS], Some(&cancel), None).unwrap_err();
    assert!(e.message().contains("make install"));
}

#[test]
fn the_windows_remedy_is_never_a_linux_only_installer() {
    let out = tools::emulation_failure("docker", true, true, "exec /bin/sh: exec format error");
    assert!(!out.contains("make setup-arm64-emulation"));
    assert!(out.contains("Windows containers"));
    assert!(out.contains("exec format error"));
    let out = tools::emulation_failure("podman", false, false, "");
    assert!(out.contains("make setup-arm64-emulation"));
    assert!(!out.ends_with('\n'));
}

#[test]
fn the_probe_verdict_is_the_machine_name_the_container_printed() {
    for (code, text, expect) in [
        (0, "aarch64\n", true),
        (
            0,
            "Unable to find image locally\nbookworm-slim: Pulling from library/debian\naarch64\r\n",
            true,
        ),
        (0, "x86_64\n", false),
        (1, "exec /bin/sh: exec format error", false),
        (125, "aarch64\n", false),
    ] {
        let r = ProcessResult { exit_code: code, output: text.into() };
        assert_eq!(tools::reports_aarch64(&r), expect, "{code} {text:?}");
    }
}

#[test]
fn a_failed_repair_is_reported_differently_from_an_unattempted_one() {
    let a = tools::emulation_failure("docker", true, true, "");
    let b = tools::emulation_failure("docker", true, false, "");
    assert_ne!(a, b);
    assert!(a.contains("still"));
}

// ------------------------------------------------------------------- jobs

fn fields() -> ConnectionFields<'static> {
    ConnectionFields {
        host: "pi.local",
        username: "pi",
        password: "ssh-password",
        sudo_password: "",
        known_hosts: "",
        bootstrap_root_password: "",
    }
}

#[test]
fn the_three_credentials_stay_separate() {
    let f = ConnectionFields {
        sudo_password: "sudo-password",
        bootstrap_root_password: "root-password",
        ..fields()
    };
    let c = connection_from_fields(&f).unwrap();
    assert_eq!(c.password.text(), "ssh-password");
    assert_eq!(c.sudo_password.text(), "sudo-password");
    assert_eq!(c.bootstrap_root_password.text(), "root-password");
}

#[test]
fn an_empty_ssh_password_survives_where_the_optional_ones_do_not() {
    let f = ConnectionFields { username: "root", password: "", ..fields() };
    let c = connection_from_fields(&f).unwrap();
    assert!(c.password.is_set());
    assert_eq!(c.password.text(), "");
    assert!(!c.sudo_password.is_set());
    assert!(!c.bootstrap_root_password.is_set());
    assert!(c.known_hosts_path.is_none());
}

#[test]
fn surrounding_whitespace_is_trimmed_from_the_typed_fields() {
    let f = ConnectionFields { host: "  pi.local\t", username: " pi ", ..fields() };
    let c = connection_from_fields(&f).unwrap();
    assert_eq!(c.host, "pi.local");
    assert_eq!(c.username, "pi");
}

#[test]
fn an_invalid_host_is_refused_before_anything_connects() {
    assert!(connection_from_fields(&ConnectionFields { host: " -pi.local ", ..fields() }).is_err());
    assert!(connection_from_fields(&ConnectionFields { username: "ro ot", ..fields() }).is_err());
    assert!(
        connection_from_fields(&ConnectionFields {
            known_hosts: "/nonexistent/known_hosts",
            ..fields()
        })
        .is_err()
    );
}

// ---------------------------------------------------------------- sd card

fn sd(path: &str) -> SdSettings {
    SdSettings {
        boot_directory: path.into(),
        country: "US".into(),
        wifi_ssid: "Studio Wi-Fi".into(),
        wifi_password: secret("correct horse battery staple"),
    }
}

#[test]
fn wifi_file_uses_linux_lines_and_derived_credentials() {
    let text = sd_card::wpa_supplicant_config(&sd("unused")).unwrap();
    assert!(text.starts_with("country=US\nnetwork={\n"));
    assert!(!text.contains('\r'));
    assert!(!text.contains("Studio Wi-Fi"));
    assert!(!text.contains("correct horse battery staple"));
    assert!(text.contains("    ssid=53747564696f2057692d4669\n"));
    let psk = &text[find(&text, "    psk=") + 8..];
    let psk = psk.split('\n').next().unwrap();
    assert_eq!(psk.len(), 64);
    assert!(psk.bytes().all(|b| b"0123456789abcdef".contains(&b)));
}

#[test]
fn validation_requires_an_alpine_boot_partition_and_country() {
    let root = scratch("sd-card");
    let mut s = sd(root.to_str().unwrap());
    let problem = sd_card::validate_sd_settings(&s).unwrap();
    assert!(problem.contains("not an Alpine boot partition"), "{problem}");
    fs::write(root.join(".alpine-release"), "3.24.2\n").unwrap();
    fs::write(root.join("config.txt"), "[all]\n").unwrap();
    fs::create_dir(root.join("boot")).unwrap();
    assert_eq!(sd_card::validate_sd_settings(&s), None);
    s.country = "us".into();
    assert!(sd_card::validate_sd_settings(&s).is_some());
    let _ = fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------- capsule

#[test]
fn the_fixture_capsule_is_described() {
    assert_eq!(FIXTURE.members.len(), 4);
    assert_eq!(FIXTURE.members[0].name, "omt-client-arm64.tar.gz");
    assert!(FIXTURE.member_named("deploy/host/set-hostname.sh").is_some());
    assert!(FIXTURE.member_named("deploy/host/nothing.sh").is_none());
    let image = FIXTURE.image().unwrap();
    assert!(image.bytes.len() > 2 && image.bytes.starts_with(&[0x1f, 0x8b]));
    assert!(
        FIXTURE
            .report()
            .unwrap()
            .starts_with("Embedded capsule: 4 members, omt-client-arm64.tar.gz 0 MiB, sha256 ")
    );
    assert!(Capsule::EMPTY.report().is_err());
}
