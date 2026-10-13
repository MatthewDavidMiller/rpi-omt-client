//! The terminal deployer: state and input, the views, and the VT decoder,
//! ported from tests/c/test_deploy_tui.c.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use omt_common::VERSION;
use omt_deploy::capsule::Capsule;
use omt_deploy::core::Action;
use omt_deploy::jobs::JobKind;
use omt_tui::app::{App, Field, LOG_CAPACITY, Slot, View, Worker};
use omt_tui::screen::Screen;
use omt_tui::term::{Code, Key, decode};
use omt_tui::ui;

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

/// What a terminal would show, as text.
fn render(a: &App, w: i32, h: i32) -> String {
    let mut s = Screen::new();
    s.begin(w, h);
    ui::draw(&mut s, a);
    (0..h).map(|y| s.row_text(y) + "\n").collect()
}

// ------------------------------------------------------------------ log

#[test]
fn paging_up_leaves_the_tail_rather_than_the_beginning() {
    let mut a = with_log(500);
    assert!(a.follow_log);
    a.scroll_log(-1);
    assert!(!a.follow_log);
    assert_eq!(a.log_scroll, 489);
}

#[test]
fn paging_back_down_resumes_following() {
    let mut a = with_log(500);
    a.scroll_log(-1);
    a.scroll_log(1);
    assert_eq!(a.log_scroll, 499);
    a.scroll_log(1);
    assert!(a.follow_log);
}

#[test]
fn paging_up_stops_at_the_start_of_the_log() {
    let mut a = with_log(30);
    for _ in 0..10 {
        a.scroll_log(-1);
    }
    assert_eq!(a.log_scroll, 0);
    assert!(!a.follow_log);
}

#[test]
fn paging_an_empty_log_is_harmless() {
    let mut a = with_log(0);
    a.scroll_log(-1);
    assert_eq!(a.log_scroll, 0);
}

#[test]
fn the_log_keeps_its_capacity() {
    let a = with_log(LOG_CAPACITY + 5);
    assert_eq!(a.log.len(), LOG_CAPACITY);
    assert_eq!(a.log[0], "5");
}

// ------------------------------------------------------------------ app

#[test]
fn a_prefilled_first_field_is_edited_from_its_end() {
    let mut a = app();
    for _ in 0.."raspberrypi.local".len() {
        a.backspace();
    }
    assert_eq!(a.field(Field::Host), "");
    a.set_field(Field::Host, "pi.local");
    a.select_view(View::Deploy);
    a.select_view(View::Connection);
    a.backspace();
    assert_eq!(a.field(Field::Host), "pi.loca");
}

#[test]
fn editing_counts_characters_not_bytes() {
    let mut a = app();
    a.select_view(View::Wifi);
    a.handle_key(&Key::char(0xE9));
    a.handle_key(&Key::char(u32::from('x')));
    assert_eq!(a.field(Field::WifiSsid), "\u{e9}x");
    a.move_cursor(-1);
    a.backspace();
    assert_eq!(a.field(Field::WifiSsid), "x");
    a.cursor_home();
    a.delete();
    assert_eq!(a.field(Field::WifiSsid), "");
    // Control characters never reach a value.
    a.handle_key(&Key::char(0x07));
    assert_eq!(a.field(Field::WifiSsid), "");
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("omt-tui-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

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
    let r = a.request(JobKind::PrepareSd, Action::Status).unwrap();
    assert!(r.connection.is_none());
    a.set_field(Field::WifiCountry, "us");
    assert!(a.precheck(JobKind::PrepareSd).is_some());
    let _ = fs::remove_dir_all(&root);
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
    // While a confirmation is open, other keys do nothing.
    let focus = a.focus;
    a.handle_key(&Key::of(Code::Tab));
    assert_eq!(a.focus, focus);
    a.handle_key(&Key::of(Code::Esc));
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

#[test]
fn keys_switch_views_and_reveal_secrets() {
    let mut a = app();
    a.handle_key(&Key::f(5));
    assert_eq!(a.current, View::Manage);
    let next = Key { ctrl: true, ..Key::of(Code::Right) };
    let prev = Key { ctrl: true, ..Key::of(Code::Left) };
    a.handle_key(&next);
    assert_eq!(a.current, View::Wifi);
    a.handle_key(&prev);
    a.handle_key(&prev);
    assert_eq!(a.current, View::Deploy);
    a.select_view(View::Connection);
    a.handle_key(&prev);
    assert_eq!(a.current, View::About);
    a.handle_key(&Key::ctrl('r'));
    assert!(a.reveal);
    a.handle_key(&Key::ctrl('c'));
    assert!(a.should_quit);
}

// ------------------------------------------------------------------- ui

#[test]
fn about_reproduces_the_licence_and_the_third_party_notices() {
    let mut lines = Vec::new();
    ui::about_text(&CAPSULE, 80, &mut lines);
    let joined = lines.join("\n");
    assert!(joined.contains(VERSION));
    assert!(joined.contains("LICENSE\n-------"));
    assert!(joined.contains("THIRD-PARTY NOTICES"));
    // Every line of both texts is reproduced; the real notices have lines
    // wider than the view, so compare with the wrapping's breaks undone.
    let words = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = words(&joined);
    for text in [CAPSULE.license, CAPSULE.notices] {
        for line in text.lines() {
            assert!(flat.contains(&words(line)), "missing: {line}");
        }
    }
}

#[test]
fn wrapping_keeps_blank_lines_and_indentation() {
    let mut lines = Vec::new();
    ui::wrap("head\n\n  a bb ccc", 6, &mut lines);
    assert_eq!(lines, ["head", "", "  a bb", "  ccc"]);
}

#[test]
fn a_word_wider_than_the_view_is_broken_rather_than_clipped() {
    let mut lines = Vec::new();
    ui::wrap("abcdefgh", 3, &mut lines);
    assert_eq!(lines, ["abc", "def", "gh"]);
}

#[test]
fn the_document_fits_the_width_it_wrapped_to() {
    for width in [38, 60, 120] {
        let mut lines = Vec::new();
        ui::about_text(&CAPSULE, width, &mut lines);
        for line in &lines {
            assert!(line.chars().count() <= width, "{width}: {line}");
        }
    }
}

#[test]
fn the_end_of_the_notices_can_be_scrolled_to() {
    let mut a = app();
    a.current = View::About;
    a.about_scroll = usize::MAX;
    let out = render(&a, 80, 30);
    let notices = CAPSULE.notices.trim_end_matches(['\n', ' ']);
    let last = notices.rsplit('\n').next().unwrap();
    assert!(out.contains(last), "{out}");
    a.about_scroll = 0;
    assert!(render(&a, 80, 30).contains("Raspberry Pi OMT client deployer"));
}

#[test]
fn a_short_terminal_scrolls_the_form_to_the_focused_row() {
    let mut a = app();
    a.select_view(View::Alpine);
    let slots = View::Alpine.slots().len();
    assert!(slots > 5);
    // 14 rows: 3 header, 3 status, 2 borders, 6 for nine fields.
    let out = render(&a, 80, 14);
    assert!(out.contains("Appliance hostname"));
    assert!(!out.contains("Run Alpine setup"));
    a.focus = slots - 1;
    let out = render(&a, 80, 14);
    assert!(out.contains("Run Alpine setup"));
    assert!(out.contains("fields"));
}

#[test]
fn a_tall_terminal_shows_every_row_at_once() {
    let mut a = app();
    a.select_view(View::Alpine);
    let out = render(&a, 80, 30);
    assert!(out.contains("Appliance hostname"));
    assert!(out.contains("Run Alpine setup"));
    assert!(!out.contains("fields 1-"));
}

#[test]
fn a_narrow_terminal_shows_the_end_of_the_value_being_typed() {
    let mut a = app();
    a.select_view(View::SdCard);
    a.focus = 0;
    let path = "/run/media/operator/ALPINE-BOOT-PARTITION";
    a.set_field(Field::BootDirectory, path);
    a.cursor = path.len();
    assert!(render(&a, 40, 20).contains("PARTITION"));
}

#[test]
fn the_label_gutter_leaves_room_for_a_value() {
    for width in [38, 40, 60, 80, 200] {
        let gutter = ui::label_width(width);
        assert!((12..=32).contains(&gutter));
        assert!(gutter < usize::try_from(width).unwrap());
    }
    assert_eq!(ui::truncate("short", 10), "short");
    assert_eq!(ui::truncate("far too long to fit", 10), "far too l~");
}

#[test]
fn the_visible_window_always_contains_the_focus() {
    for count in 1..12 {
        for visible in 1..=count {
            for focus in 0..count {
                let first = ui::first_visible_slot(focus, count, visible);
                assert!(first + visible <= count);
                assert!(focus >= first && focus < first + visible);
            }
        }
    }
}

#[test]
fn secrets_are_masked_until_revealed() {
    let mut a = app();
    a.set_field(Field::Password, "hunter2");
    let out = render(&a, 80, 24);
    assert!(!out.contains("hunter2"));
    assert!(out.contains("*******"));
    a.reveal = true;
    assert!(render(&a, 80, 24).contains("hunter2"));
}

#[test]
fn too_small_a_terminal_says_so() {
    assert!(render(&app(), 30, 8).contains("Terminal is 30x8"));
}

// ---------------------------------------------------------------- input

fn keys(bytes: &[u8], final_: bool) -> Vec<Key> {
    let mut out = Vec::new();
    decode(bytes, final_, 16, &mut out);
    out
}

#[test]
fn vt_input_decodes_to_the_keys_the_app_handles() {
    let k = keys(b"\x1b[A\x1b[B\x1b[C\x1b[D", false);
    assert_eq!(k.len(), 4);
    assert!(k[0].code == Code::Up && k[3].code == Code::Left);
    let k = keys(b"\x1b[1;5C\x1b[1;5D", false);
    assert_eq!(k.len(), 2);
    assert!(k[0].code == Code::Right && k[0].ctrl && k[1].code == Code::Left && k[1].ctrl);
    let k = keys(b"\x1bOP\x1b[15~\x1b[19~", false);
    assert_eq!(k.len(), 3);
    assert!(k[0].code == Code::F && k[0].f == 1 && k[1].f == 5 && k[2].f == 8);
    let k = keys(b"\x1b[H\x1b[F\x1b[3~\x1b[5~\x1b[6~\x1b[Z", false);
    let codes: Vec<Code> = k.iter().map(|k| k.code).collect();
    assert_eq!(
        codes,
        [Code::Home, Code::End, Code::Delete, Code::PageUp, Code::PageDown, Code::BackTab]
    );
    let k = keys(b"\r\t\x7f\x11\x12\x03", false);
    assert_eq!(k.len(), 6);
    assert!(k[0].code == Code::Enter && k[1].code == Code::Tab && k[2].code == Code::Backspace);
    assert!(k[3].code == Code::Char && k[3].ctrl && k[3].ch == u32::from('q'));
    assert!(k[4].ch == u32::from('r') && k[5].ch == u32::from('c'));
    let k = keys("a\u{e9}\u{20ac}".as_bytes(), false);
    assert_eq!(k.len(), 3);
    assert!(k[1].ch == 0xE9 && k[2].ch == 0x20AC);
    // A lone Esc only once nothing more is coming.
    assert!(keys(b"\x1b", false).is_empty());
    let k = keys(b"\x1b", true);
    assert!(k.len() == 1 && k[0].code == Code::Esc);
    // A split UTF-8 character waits for the rest.
    assert!(keys(b"\xc3", false).is_empty());
}
