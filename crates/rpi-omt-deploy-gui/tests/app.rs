//! The deployer's state model: prechecks, confirmations, the worker, and
//! the log, as the browser page drives them.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use omt_common::VERSION;
use omt_deploy::capsule::Capsule;
use omt_deploy::core::Action;
use omt_deploy::jobs::JobKind;
use omt_gui::about;
use omt_gui::app::{App, FIELD_LIMIT, FIELDS, Field, LOG_CAPACITY, Slot, VIEWS, Worker};

/// The licence and notices the shipped capsule carries, without needing the
/// appliance image built.
static CAPSULE: Capsule = Capsule {
    members: &[],
    license: include_str!("../../../LICENSE"),
    notices: include_str!("../../../THIRD_PARTY_NOTICES.txt"),
};

fn app() -> App {
    App::new(&CAPSULE)
}

fn with_log(lines: usize) -> App {
    let mut a = app();
    for i in 0..lines {
        a.push_log(&i.to_string());
    }
    a
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("omt-gui-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------ log

#[test]
fn the_log_keeps_its_capacity() {
    let a = with_log(LOG_CAPACITY + 5);
    assert_eq!(a.log.len(), LOG_CAPACITY);
    assert_eq!(a.log[0], "5");
    assert_eq!(a.log_end, (LOG_CAPACITY + 5) as u64);
}

#[test]
fn a_reader_gets_only_the_lines_it_has_not_seen() {
    let a = with_log(10);
    let (first, lines) = a.log_since(7);
    assert_eq!(first, 7);
    assert_eq!(lines.cloned().collect::<Vec<_>>(), ["7", "8", "9"]);
    let (first, lines) = a.log_since(10);
    assert_eq!(first, 10);
    assert_eq!(lines.count(), 0);
    // Asking past the end is the same as being caught up.
    assert_eq!(a.log_since(99).1.count(), 0);
}

#[test]
fn a_reader_that_fell_behind_learns_lines_were_dropped() {
    let a = with_log(LOG_CAPACITY + 50);
    let (first, mut lines) = a.log_since(3);
    assert_eq!(first, 50);
    assert_eq!(lines.next().unwrap(), "50");
}

// ---------------------------------------------------------------- names

#[test]
fn every_field_and_slot_has_a_unique_name_that_round_trips() {
    let mut names: Vec<&str> = FIELDS.iter().map(|f| f.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), FIELDS.len());
    for f in FIELDS {
        assert_eq!(Field::from_name(f.name()), Some(f));
    }
    let mut slots: Vec<Slot> = VIEWS.iter().flat_map(|v| v.slots()).copied().collect();
    slots.sort_by_key(|s| s.name());
    slots.dedup();
    let mut names: Vec<&str> = slots.iter().map(|s| s.name()).collect();
    names.dedup();
    assert_eq!(names.len(), slots.len());
    for s in slots {
        assert_eq!(Slot::from_name(s.name()), Some(s));
    }
    assert_eq!(Field::from_name("nope"), None);
    assert_eq!(Slot::from_name("nope"), None);
}

#[test]
fn every_button_runs_a_job_and_nothing_else_does() {
    for s in VIEWS.iter().flat_map(|v| v.slots()) {
        assert_eq!(s.job().is_some(), s.kind() == omt_gui::app::Kind::Button, "{s:?}");
    }
}

// ---------------------------------------------------------------- values

#[test]
fn values_refuse_control_characters_and_oversized_text() {
    let mut a = app();
    assert!(a.set_field(Field::WifiSsid, "caf\u{e9}"));
    assert_eq!(a.field(Field::WifiSsid), "caf\u{e9}");
    assert!(!a.set_field(Field::WifiSsid, "bell\u{7}"));
    assert!(!a.set_field(Field::WifiSsid, &"x".repeat(FIELD_LIMIT + 1)));
    assert_eq!(a.field(Field::WifiSsid), "caf\u{e9}");
}

#[test]
fn toggles_are_set_only_through_toggle_slots() {
    let mut a = app();
    assert!(a.set_toggle(Slot::WifiConnect, false));
    assert!(!a.toggle(Slot::WifiConnect));
    assert!(!a.set_toggle(Slot::Host, true));
}

// ------------------------------------------------------------------ jobs

#[test]
fn sd_card_job_is_local_and_uses_the_shared_validation() {
    let root = scratch("sd");
    fs::write(root.join(".alpine-release"), "alpine-rpi-3.24.2\n").unwrap();
    fs::write(root.join("config.txt"), "[all]\n").unwrap();
    fs::create_dir(root.join("boot")).unwrap();
    let mut a = app();
    a.set_field(Field::BootDirectory, root.to_str().unwrap());
    a.set_field(Field::WifiSsid, "studio");
    a.set_field(Field::WifiPassword, "passphrase");
    assert_eq!(a.precheck(JobKind::PrepareSd), None);
    assert_eq!(a.blocked(Slot::PrepareSdButton), None);
    let r = a.request(JobKind::PrepareSd, Action::Status).unwrap();
    assert!(r.connection.is_none());
    a.set_field(Field::WifiCountry, "us");
    assert!(a.precheck(JobKind::PrepareSd).is_some());
    assert!(a.blocked(Slot::PrepareSdButton).is_some());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_connection_the_core_would_refuse_blocks_its_buttons() {
    let mut a = app();
    assert_eq!(a.blocked(Slot::TestButton), None);
    a.set_field(Field::Host, "");
    assert!(a.blocked(Slot::TestButton).is_some());
    assert!(a.blocked(Slot::StatusButton).is_some());
    // Toggles and fields are never blocked; only actions are.
    assert_eq!(a.blocked(Slot::Host), None);
}

#[test]
fn factory_alpine_keeps_an_explicit_empty_ssh_password() {
    let mut a = app();
    a.set_field(Field::Host, "10.1.20.223");
    a.set_field(Field::User, "root");
    a.set_field(Field::Password, "");
    let r = a.request(JobKind::Test, Action::Status).unwrap();
    assert_eq!(r.connection.unwrap().password.text(), "");
}

#[test]
fn the_alpine_install_is_confirmed_before_anything_starts() {
    let mut a = app();
    a.press(Slot::AlpineButton);
    let p = a.confirm.clone().unwrap();
    assert!(p.prompt.contains("Erase the boot disk"));
    assert!(!p.quit && p.job == JobKind::Alpine);
    assert!(!a.busy());
    a.confirm_pending(false);
    assert!(a.confirm.is_none());
    assert!(!a.busy());
    assert_eq!(a.status, "Cancelled.");
}

#[test]
fn reading_status_is_not_confirmed() {
    let mut a = app();
    // An unroutable documentation address: the job fails, but never asks.
    a.set_field(Field::Host, "192.0.2.1");
    a.press(Slot::StatusButton);
    assert!(a.confirm.is_none());
    if a.busy() {
        a.cancel_job();
        while a.busy() {
            a.poll_worker();
            thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn restart_and_reboot_ask_first() {
    let mut a = app();
    a.press(Slot::RestartButton);
    assert_eq!(a.confirm.as_ref().map(|p| p.action), Some(Action::Restart));
    a.confirm_pending(false);
    a.press(Slot::RebootButton);
    assert_eq!(a.confirm.as_ref().map(|p| p.action), Some(Action::Reboot));
    a.confirm_pending(false);
    assert!(a.confirm.is_none());
}

#[test]
fn quitting_asks_only_while_a_job_is_running() {
    let mut a = app();
    a.request_quit();
    assert!(a.should_quit);

    let mut busy = app();
    busy.set_worker(Worker::running());
    busy.request_quit();
    assert!(!busy.should_quit);
    assert!(busy.confirm.is_some());
    busy.confirm_pending(true);
    assert!(busy.should_quit);
}

#[test]
fn a_short_host_password_is_refused_by_the_precheck() {
    let mut a = app();
    a.set_field(Field::OsRootPassword, "rootpass1");
    a.set_field(Field::OsRootConfirm, "rootpass1");
    a.set_field(Field::OsPiPassword, "pipassword");
    a.set_field(Field::OsPiConfirm, "pipassword");
    assert_eq!(a.precheck(JobKind::Alpine), None);
    a.set_field(Field::OsPiPassword, "short");
    a.set_field(Field::OsPiConfirm, "short");
    assert!(a.precheck(JobKind::Alpine).is_some());
    a.set_field(Field::OsPiPassword, "pipassword");
    a.set_field(Field::OsPiConfirm, "pipassword2");
    assert_eq!(a.precheck(JobKind::Alpine), Some("pi password confirmation does not match"));
    a.set_field(Field::OsPiConfirm, "pipassword");
    a.set_field(Field::WifiSsid, "studio");
    assert_eq!(a.precheck(JobKind::Alpine), Some("A Wi-Fi SSID needs a Wi-Fi password"));
}

#[test]
fn deploy_refuses_a_remote_directory_the_core_would_reject() {
    let mut a = app();
    assert_eq!(a.precheck(JobKind::Deploy), None);
    for rejected in ["", "opt/omt-client", "/opt/omt-client/", "/opt/../etc"] {
        a.set_field(Field::RemoteDirectory, rejected);
        assert!(a.precheck(JobKind::Deploy).is_some(), "accepted {rejected}");
    }
}

#[test]
fn successful_alpine_setup_can_apply_the_new_pi_login() {
    let mut a = app();
    a.set_field(Field::User, "root");
    a.set_field(Field::OsPiPassword, "new-pi-password");
    a.switch_to_pi_after_job = true;
    a.set_worker(Worker::finished(Ok(())));
    a.poll_worker();
    assert_eq!(a.field(Field::User), "pi");
    assert_eq!(a.field(Field::Password), "new-pi-password");
    assert_eq!(a.field(Field::SudoPassword), "new-pi-password");
    assert!(!a.switch_to_pi_after_job);
    assert!(!a.busy());
}

#[test]
fn a_failed_job_is_reported_in_the_status_and_the_log() {
    let mut a = app();
    a.set_worker(Worker::finished(Err("it broke")));
    a.poll_worker();
    assert_eq!(a.status, "Failed: it broke");
    assert_eq!(a.log, ["-- failed: it broke --"]);
}

/// Remote errors carry line breaks. Found on a Pi 4: the Wi-Fi rollback
/// error rendered its breaks as replacement characters and cut the status
/// bar short.
#[test]
fn a_multi_line_failure_is_one_status_row_and_several_log_rows() {
    let mut a = app();
    a.set_worker(Worker::finished(Err(
        "Wi-Fi update failed:\nNew Wi-Fi profile did not associate; retained\n",
    )));
    a.poll_worker();
    assert_eq!(
        a.status,
        "Failed: Wi-Fi update failed: New Wi-Fi profile did not associate; retained"
    );
    assert_eq!(
        a.log,
        ["-- failed: Wi-Fi update failed:", "New Wi-Fi profile did not associate; retained --"]
    );
    a.push_log("one\r\ntwo\n\nfour");
    assert_eq!(a.log.len(), 6);
    assert_eq!(a.log.range(2..).collect::<Vec<_>>(), ["one", "two", "", "four"]);
}

#[test]
fn web_password_rotation_uses_its_own_fields() {
    let mut a = app();
    a.set_field(Field::Password, "pw");
    a.set_field(Field::WebPassword, "deploy-view-password");
    a.set_field(Field::ManageWebPassword, "manage-view-password");
    let r = a.request(JobKind::WebPassword, Action::Status).unwrap();
    assert_eq!(r.web_password.text(), "manage-view-password");
    let r = a.request(JobKind::Deploy, Action::Status).unwrap();
    assert_eq!(r.web_password.text(), "deploy-view-password");
}

// ----------------------------------------------------------------- about

#[test]
fn about_reproduces_the_licence_and_the_third_party_notices() {
    let summary = about::summary(&CAPSULE);
    assert!(summary.contains(VERSION));
    assert!(summary.contains("built without"));
    for (text, full) in
        [(about::license(&CAPSULE), CAPSULE.license), (about::notices(&CAPSULE), CAPSULE.notices)]
    {
        assert_eq!(text, full.trim_end());
    }
}
