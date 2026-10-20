//! The loopback server: what it refuses, what it never sends back, and the
//! folder picker's view of the file system.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use omt_common::json;
use omt_deploy::capsule::Capsule;
use omt_gui::app::Field;
use omt_gui::fs as gfs;
use omt_gui::http::{self, Request};
use omt_gui::server::{Shared, schema_json, serve};

static CAPSULE: Capsule = Capsule {
    members: &[],
    license: include_str!("../../../LICENSE"),
    notices: include_str!("../../../THIRD_PARTY_NOTICES.txt"),
};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const PORT: u16 = 8765;

fn shared() -> Shared {
    Shared::new(&CAPSULE, TOKEN.into(), PORT)
}

fn request(method: &str, target: &str, body: &str) -> Request {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut headers = vec![
        ("Host".to_owned(), format!("127.0.0.1:{PORT}")),
        ("X-Deploy-Token".to_owned(), TOKEN.to_owned()),
    ];
    if method == "POST" {
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    }
    Request {
        method: method.into(),
        path: path.into(),
        query: query.into(),
        headers,
        body: body.as_bytes().to_vec(),
    }
}

fn without(mut r: Request, header: &str) -> Request {
    r.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(header));
    r
}

fn with(mut r: Request, header: &str, value: &str) -> Request {
    r = without(r, header);
    r.headers.push((header.to_owned(), value.to_owned()));
    r
}

fn body(s: &Shared, r: &Request) -> (u16, json::Value) {
    let res = s.handle(r);
    (res.status, json::parse(&res.body).unwrap())
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("omt-gui-server-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_boot_partition(dir: &std::path::Path) {
    fs::create_dir_all(dir.join("boot")).unwrap();
    fs::write(dir.join(".alpine-release"), "alpine-rpi-3.24.2\n").unwrap();
    fs::write(dir.join("config.txt"), "[all]\n").unwrap();
}

// -------------------------------------------------------------- refusals

#[test]
fn the_api_needs_the_session_token() {
    let s = shared();
    assert_eq!(s.handle(&request("GET", "/api/state", "")).status, 200);
    assert_eq!(s.handle(&without(request("GET", "/api/state", ""), "x-deploy-token")).status, 403);
    let wrong = with(request("GET", "/api/state", ""), "X-Deploy-Token", &TOKEN.replace('0', "1"));
    assert_eq!(s.handle(&wrong).status, 403);
    let short = with(request("GET", "/api/state", ""), "X-Deploy-Token", &TOKEN[1..]);
    assert_eq!(s.handle(&short).status, 403);
    // The page itself is not a secret; the token arrives in its fragment.
    assert_eq!(s.handle(&without(request("GET", "/", ""), "x-deploy-token")).status, 200);
}

#[test]
fn only_this_servers_own_host_names_are_answered() {
    let s = shared();
    for host in ["127.0.0.1:8765", "localhost:8765"] {
        assert_eq!(s.handle(&with(request("GET", "/", ""), "Host", host)).status, 200, "{host}");
    }
    // A rebound DNS name, the wrong port, and no Host at all.
    for host in ["evil.example:8765", "127.0.0.1:80", "127.0.0.1", "localhost.evil:8765"] {
        assert_eq!(s.handle(&with(request("GET", "/", ""), "Host", host)).status, 403, "{host}");
    }
    assert_eq!(s.handle(&without(request("GET", "/", ""), "host")).status, 403);
}

#[test]
fn cross_origin_requests_are_refused() {
    let s = shared();
    let same = with(request("POST", "/api/cancel", "{}"), "Origin", "http://127.0.0.1:8765");
    assert_eq!(s.handle(&same).status, 200);
    let other = with(request("POST", "/api/cancel", "{}"), "Origin", "https://evil.example");
    assert_eq!(s.handle(&other).status, 403);
}

#[test]
fn actions_take_only_json() {
    let s = shared();
    let form = with(
        request("POST", "/api/press", "slot=reboot"),
        "Content-Type",
        "application/x-www-form-urlencoded",
    );
    assert_eq!(s.handle(&form).status, 415);
    assert_eq!(s.handle(&request("POST", "/api/press", "{not json")).status, 400);
    assert_eq!(s.handle(&request("PUT", "/api/press", "{}")).status, 405);
    assert!(s.app().confirm.is_none());
}

#[test]
fn unknown_names_change_nothing() {
    let s = shared();
    for b in [
        r#"{"fields":{"host":"pi.local","nope":"x"}}"#,
        r#"{"fields":{"host":"pi.local"},"extra":1}"#,
        r#"{"fields":{"host":"pi.local"},"toggles":{"host":true}}"#,
        r#"{"fields":{"host":"pi\u0007.local"}}"#,
        r#"{"fields":{"host":7}}"#,
    ] {
        assert_eq!(s.handle(&request("POST", "/api/fields", b)).status, 400, "{b}");
        assert_eq!(s.app().field(Field::Host), "raspberrypi.local", "{b}");
    }
    assert_eq!(s.handle(&request("POST", "/api/press", r#"{"slot":"host"}"#)).status, 400);
    assert_eq!(s.handle(&request("GET", "/api/nope", "")).status, 404);
    assert_eq!(s.handle(&request("GET", "/../etc/passwd", "")).status, 404);
}

// ----------------------------------------------------------------- state

#[test]
fn secrets_go_in_and_never_come_out() {
    let s = shared();
    let b = r#"{"fields":{"host":"pi.local","password":"hunter2-secret"},"toggles":{"wifi_connect":false}}"#;
    let res = s.handle(&request("POST", "/api/fields", b));
    assert_eq!(res.status, 200);
    assert!(!String::from_utf8_lossy(&res.body).contains("hunter2"));
    assert_eq!(s.app().field(Field::Password), "hunter2-secret");
    assert!(!s.app().wifi_connect);
    let (status, state) = body(&s, &request("GET", "/api/state", ""));
    assert_eq!(status, 200);
    let fields = state.get("fields").unwrap();
    assert_eq!(fields.get("host").unwrap().as_str(), Some("pi.local"));
    assert_eq!(fields.get("password").unwrap().get("set").unwrap().as_bool(), Some(true));
    assert_eq!(fields.get("sudo_password").unwrap().get("set").unwrap().as_bool(), Some(false));
    let raw = s.handle(&request("GET", "/api/state", ""));
    assert!(!String::from_utf8_lossy(&raw.body).contains("hunter2"));
}

#[test]
fn a_destructive_action_only_asks() {
    let s = shared();
    let (status, state) = body(&s, &request("POST", "/api/press", r#"{"slot":"reboot"}"#));
    assert_eq!(status, 200);
    assert!(state.get("confirm").unwrap().as_str().unwrap().contains("Reboot"));
    assert_eq!(state.get("busy").unwrap().as_bool(), Some(false));
    // Nothing else is pressed while the question is open.
    assert_eq!(s.handle(&request("POST", "/api/press", r#"{"slot":"restart"}"#)).status, 400);
    let (_, state) = body(&s, &request("POST", "/api/confirm", r#"{"accept":false}"#));
    assert!(state.get("confirm").unwrap().is_null());
    assert_eq!(state.get("status").unwrap().as_str(), Some("Cancelled."));
}

#[test]
fn state_reports_why_each_action_would_not_start() {
    let s = shared();
    s.handle(&request("POST", "/api/fields", r#"{"fields":{"remote_directory":"relative"}}"#));
    let (_, state) = body(&s, &request("GET", "/api/state", ""));
    let blocked = state.get("blocked").unwrap();
    assert!(blocked.get("deploy").unwrap().as_str().unwrap().contains("Remote directory"));
    assert!(blocked.get("test").unwrap().is_null());
    assert!(blocked.get("prepare_sd").unwrap().as_str().is_some());
}

#[test]
fn the_log_is_read_incrementally() {
    let s = shared();
    for i in 0..5 {
        s.app().push_log(&format!("line {i}"));
    }
    let (_, state) = body(&s, &request("GET", "/api/state?since=3", ""));
    assert_eq!(state.get("log_start").unwrap().as_u64(), Some(3));
    assert_eq!(state.get("log_end").unwrap().as_u64(), Some(5));
    let log: Vec<_> =
        state.get("log").unwrap().as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(log, ["line 3", "line 4"]);
}

/// Found in the browser: an action's answer that carried the log moved the
/// page's cursor past lines it had not fetched, and they were never shown.
#[test]
fn an_actions_answer_leaves_the_log_to_the_poll() {
    let s = shared();
    s.app().push_log("before");
    let (_, poll) = body(&s, &request("GET", "/api/state?since=0", ""));
    let end = poll.get("log_end").unwrap().as_u64().unwrap();
    s.app().push_log("after");
    let (_, answer) = body(&s, &request("POST", "/api/cancel", "{}"));
    assert!(answer.get("log").is_none() && answer.get("log_end").is_none());
    let (_, poll) = body(&s, &request("GET", &format!("/api/state?since={end}"), ""));
    let log: Vec<_> =
        poll.get("log").unwrap().as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(log, ["after"]);
}

#[test]
fn quitting_ends_the_session() {
    let s = shared();
    assert!(!s.quitting());
    let (_, state) = body(&s, &request("POST", "/api/quit", "{}"));
    assert_eq!(state.get("quit").unwrap().as_bool(), Some(true));
    assert!(s.quitting());
}

#[test]
fn the_schema_is_the_state_models_views() {
    let schema = json::parse(schema_json().as_bytes()).unwrap();
    let views = schema.get("views").unwrap().as_array().unwrap();
    assert_eq!(views.len(), omt_gui::app::VIEWS.len());
    let sd = views.iter().find(|v| v.get("title").unwrap().as_str() == Some("SD card")).unwrap();
    let slots = sd.get("slots").unwrap().as_array().unwrap();
    assert_eq!(slots[0].get("field").unwrap().as_str(), Some("boot_directory"));
    let s = shared();
    let (status, _) = body(&s, &request("GET", "/api/schema", ""));
    assert_eq!(status, 200);
}

#[test]
fn the_page_is_served_with_its_security_headers() {
    let s = shared();
    let page = s.handle(&request("GET", "/", ""));
    assert!(String::from_utf8_lossy(&page.body).contains("/style.css"));
    // The appliance's own stylesheet, byte for byte.
    let css = s.handle(&request("GET", "/style.css", ""));
    assert_eq!(css.body, include_bytes!("../../omt-web/static/style.css"));
    assert!(http::SECURITY_HEADERS.contains("frame-ancestors 'none'"));
    assert!(http::SECURITY_HEADERS.contains("script-src 'self'"));
    assert!(!http::SECURITY_HEADERS.contains("unsafe-inline"));
}

// ------------------------------------------------------------------ http

#[test]
fn the_request_reader_holds_to_its_ceilings() {
    let ok = b"POST /api/x?since=4 HTTP/1.1\r\nHost: a\r\nContent-Length: 2\r\n\r\n{}";
    let r = http::read_request(&mut &ok[..]).unwrap();
    assert_eq!(
        (r.method.as_str(), r.path.as_str(), r.query.as_str()),
        ("POST", "/api/x", "since=4")
    );
    assert_eq!(r.body, b"{}");
    assert_eq!(r.param("since").as_deref(), Some("4"));

    let mut huge = b"GET / HTTP/1.1\r\nX: ".to_vec();
    huge.resize(http::MAX_HEADER_BYTES + 10, b'a');
    assert_eq!(http::read_request(&mut &huge[..]).unwrap_err(), 431);
    let big = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", http::MAX_BODY_BYTES + 1);
    assert_eq!(http::read_request(&mut big.as_bytes()).unwrap_err(), 413);
    let chunked = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
    assert_eq!(http::read_request(&mut &chunked[..]).unwrap_err(), 411);
    let twice = b"POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n";
    assert_eq!(http::read_request(&mut &twice[..]).unwrap_err(), 400);
    let short = b"POST / HTTP/1.1\r\nContent-Length: 9\r\n\r\n{}";
    assert_eq!(http::read_request(&mut &short[..]).unwrap_err(), 400);
    assert_eq!(http::read_request(&mut &b"GET / HTTP/2\r\n\r\n"[..]).unwrap_err(), 505);
    assert_eq!(http::read_request(&mut &b"GET nope HTTP/1.1\r\n\r\n"[..]).unwrap_err(), 400);
    let folded = b"GET / HTTP/1.1\r\nA: b\r\n c\r\n\r\n";
    assert_eq!(http::read_request(&mut &folded[..]).unwrap_err(), 400);
}

#[test]
fn query_values_are_percent_decoded() {
    assert_eq!(http::percent_decode("%2Frun%2Fmedia+x").as_deref(), Some("/run/media x"));
    assert_eq!(http::percent_decode("%C3%A9").as_deref(), Some("\u{e9}"));
    assert_eq!(http::percent_decode("%zz"), None);
    assert_eq!(http::percent_decode("%4"), None);
    assert_eq!(http::percent_decode("%FF"), None);
}

#[test]
fn a_real_socket_round_trip() {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    let port = listener.local_addr().unwrap().port();
    let s = Arc::new(Shared::new(&CAPSULE, TOKEN.into(), port));
    let server = Arc::clone(&s);
    let handle = thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || serve(&listener, &server).unwrap())
        .unwrap();
    let ask = |req: String| {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        c.write_all(req.as_bytes()).unwrap();
        let mut out = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            let n = c.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
        }
        String::from_utf8(out).unwrap()
    };
    let got = ask(format!("GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    assert!(got.starts_with("HTTP/1.1 403"), "{got}");
    assert!(got.contains("X-Frame-Options: DENY"));
    let got = ask(format!(
        "GET /api/state HTTP/1.1\r\nHost: localhost:{port}\r\nX-Deploy-Token: {TOKEN}\r\n\r\n"
    ));
    assert!(got.starts_with("HTTP/1.1 200"), "{got}");
    assert!(got.contains("\"busy\":false"));
    let got = ask(format!(
        "POST /api/quit HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Deploy-Token: {TOKEN}\r\n\
         Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
    ));
    assert!(got.contains("\"quit\":true"), "{got}");
    handle.join().unwrap();
    assert!(s.quitting());
}

// -------------------------------------------------------------------- fs

#[test]
fn the_picker_lists_folders_first_and_flags_boot_partitions() {
    let root = scratch("list");
    make_boot_partition(&root.join("ALPINE"));
    fs::create_dir(root.join("b-folder")).unwrap();
    fs::create_dir(root.join(".hidden")).unwrap();
    fs::write(root.join("a-file.txt"), "x").unwrap();
    let path = root.to_str().unwrap();

    let l = gfs::list(path, false, false).unwrap();
    let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["ALPINE", "b-folder"]);
    assert!(l.entries[0].alpine_boot && !l.entries[1].alpine_boot);
    assert_eq!(l.parent.as_deref(), root.parent().and_then(|p| p.to_str()));

    let l = gfs::list(path, true, true).unwrap();
    let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, [".hidden", "ALPINE", "b-folder", "a-file.txt"]);
    assert!(gfs::is_alpine_boot(root.join("ALPINE").to_str().unwrap()));
    assert!(gfs::list(root.join("missing").to_str().unwrap(), false, false).is_err());

    let s = shared();
    let target = format!("/api/fs/list?path={}", path.replace('/', "%2F"));
    let (status, listing) = body(&s, &request("GET", &target, ""));
    assert_eq!(status, 200);
    assert_eq!(listing.get("entries").unwrap().as_array().unwrap().len(), 2);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_huge_folder_is_listed_truncated() {
    let root = scratch("huge");
    for i in 0..=gfs::MAX_ENTRIES {
        fs::create_dir(root.join(format!("d{i:05}"))).unwrap();
    }
    let l = gfs::list(root.to_str().unwrap(), false, false).unwrap();
    assert_eq!(l.entries.len(), gfs::MAX_ENTRIES);
    assert!(l.truncated);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn removable_media_are_found_in_the_mount_table() {
    let table = "/dev/nvme0n1p2 / ext4 rw 0 0\n\
                 /dev/nvme0n1p1 /boot vfat rw 0 0\n\
                 /dev/sdb1 /run/media/op/ALPINE\\040BOOT vfat rw 0 0\n\
                 /dev/sdc1 /media/usb ext4 rw 0 0\n\
                 /dev/sdd1 /srv/card exfat rw 0 0\n\
                 proc /proc proc rw 0 0\n";
    assert_eq!(
        gfs::removable_mounts(table),
        ["/run/media/op/ALPINE BOOT", "/media/usb", "/srv/card"]
    );
    assert_eq!(gfs::unescape_mount_path("a\\134b\\011c"), "a\\b\tc");
    assert_eq!(gfs::unescape_mount_path("trailing\\04"), "trailing\\04");
}

#[test]
fn the_roots_put_a_boot_partition_first() {
    let roots = gfs::roots();
    // Nothing about the build host is assumed but that ordering holds.
    let first_plain = roots.iter().position(|r| !r.alpine_boot).unwrap_or(roots.len());
    assert!(roots[first_plain..].iter().all(|r| !r.alpine_boot));
}
