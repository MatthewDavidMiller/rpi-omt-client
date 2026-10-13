//! Port of the Web suite: password hashes and policy, video ceilings,
//! discovery-server normalisation and settings splicing, discovered-source
//! parsing, status records, forms and cookies, the rate limiter, templates,
//! the ZIP writer, HTTP heads, key=value records, and rate-limit settings.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::thread;
use std::time::Duration;

use omt_common::timefmt::{parse_rfc3339, rfc3339_millis, wall_clock};
use omt_common::{fsio, json};
use omt_web::app::{Form, RATE_KEYS, RateLimiter, cookie_value};
use omt_web::auth::{Password, encode_password, validate_new_password};
use omt_web::diagnostics::parse_record;
use omt_web::http::{MAX_HEADER_BYTES, Parse, Request, parse_head};
use omt_web::network::{normalize_server, parse_server, update_settings_xml};
use omt_web::playback::{parse_discovered, status_record_valid};
use omt_web::settings::{RateLimit, parse_rate_limit};
use omt_web::state::{describe_video_ceiling, parse_video_ceiling, pixel_rate};
use omt_web::templates_gen::render;
use omt_web::tv::Value;
use omt_web::zip::{Method, Zip, crc32};

fn verify(stored: &str, supplied: &str) -> bool {
    Password::parse(stored.as_bytes()).is_ok_and(|p| p.verify(supplied.as_bytes()))
}

#[test]
fn verifies_werkzeug_hashes() {
    let pbkdf2 =
        "pbkdf2:sha256:1$salt$120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b";
    assert!(verify(pbkdf2, "password"));
    assert!(!verify(pbkdf2, "no"));
    let scrypt = "scrypt:32768:8:1$07kZLpT9$d12f4706055d4d0812a754b965a9150e8c843b0ce3672d8db291df10e0b\
                  f144bc268bb049c9c3f209ea4614d5309a759eba4e123a4bd12e08daa002f95ccfe97";
    assert!(verify(scrypt, "password"));
    assert!(!verify(scrypt, "not-password"));
    assert!(verify("plain secret", "plain secret"));
    assert!(!verify("plain secret", "plain secre"));
}

#[test]
fn malformed_hashes_are_rejected() {
    for bad in [
        "pbkdf2:sha256:1$salt$",
        "pbkdf2:sha256:1$salt$00",
        "pbkdf2:sha256:1$salt$00000000000000000000000000000000000000000000000000000000000000",
        "pbkdf2:sha256:0$salt$120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b",
        "pbkdf2:sha1:1$salt$00",
        "argon2:whatever",
        "scrypt:3:8:1$s$00112233445566778899",
        "scrypt:4:8$s$00112233445566778899",
        "scrypt:4:8:1:1$s$00112233445566778899",
    ] {
        assert!(Password::parse(bad.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn new_password_policy_and_encoding() {
    assert!(validate_new_password("correct horse battery staple").is_ok());
    assert!(validate_new_password("too-short").is_err());
    assert!(validate_new_password(&"x".repeat(129)).is_err());
    assert!(validate_new_password("twelve bytes\n").is_err());
    let encoded = encode_password("correct horse battery staple").unwrap();
    assert!(encoded.ends_with('\n'));
    assert!(verify(encoded.trim(), "correct horse battery staple"));
    assert!(!verify(encoded.trim(), "wrong password"));
}

#[test]
fn ceilings_and_delays() {
    assert!(parse_video_ceiling("1920x1080@60").is_ok());
    assert_eq!(
        parse_video_ceiling("1921x1080@60").unwrap_err().to_string(),
        "Width 1921 is outside 16-1920."
    );
    assert!(parse_video_ceiling("1920x1080@0").is_err());
    assert_eq!(parse_video_ceiling("1920x1080@999").unwrap_err().to_string(), "Invalid frame rate");
    assert!(parse_video_ceiling("a,b,c,d,e").is_err());
    assert_eq!(
        describe_video_ceiling("1920x1080@30,1280x720@060"),
        "1920x1080 at 30 fps, or 1280x720 at 60 fps"
    );
    assert_eq!(pixel_rate("1920x1080@30,1280x720@60"), 1920 * 1080 * 30);
}

#[test]
fn discovery_servers_normalize() {
    let n = |s: &str| normalize_server(s).ok();
    assert_eq!(n("Example.COM").as_deref(), Some("omt://example.com:6399"));
    assert_eq!(n("[2001:db8::1]").as_deref(), Some("omt://[2001:db8::1]:6399"));
    assert_eq!(
        n("omt://[2001:0db8:0:0:0:0:0:1]:7000").as_deref(),
        Some("omt://[2001:db8::1]:7000")
    );
    assert_eq!(n("omt://[::ffff:192.0.2.1]").as_deref(), Some("omt://[::ffff:192.0.2.1]:6399"));
    assert_eq!(n("192.0.2.1:9").as_deref(), Some("omt://192.0.2.1:9"));
    assert_eq!(n("  ").as_deref(), Some(""));
    assert_eq!(n("host/path"), None);
    assert_eq!(n("2001:db8::1"), None);
    assert_eq!(n("host:0"), None);
    for bad in [
        "<Settings /><Settings />",
        "<!DOCTYPE Settings><Settings />",
        "<Settings><DiscoveryServer>a&amp;b</DiscoveryServer></Settings>",
        "<Settings><DiscoveryServer>a</DiscoveryServer><DiscoveryServer/></Settings>",
    ] {
        assert!(parse_server(bad.as_bytes()).is_err(), "{bad}");
    }

    // Splicing keeps every other byte and reads back the new value.
    let docs = [
        "<Settings />",
        "<?xml version=\"1.0\"?>\n<Settings>\n  <Other a=\"1\">x</Other>\n</Settings>\n",
        "<Settings><DiscoveryServer>old.example</DiscoveryServer><Keep/></Settings>",
        "<Settings><DiscoveryServer/></Settings>",
    ];
    for (i, doc) in docs.iter().enumerate() {
        let updated =
            update_settings_xml(doc.as_bytes(), "omt://example.com:6399").unwrap().unwrap();
        assert_eq!(parse_server(&updated).unwrap(), "omt://example.com:6399");
        let text = String::from_utf8(updated.clone()).unwrap();
        if i == 1 {
            assert!(text.contains("<Other a=\"1\">x</Other>"));
        }
        if i == 2 {
            assert!(text.contains("<Keep/>"));
        }
        assert!(update_settings_xml(&updated, "omt://example.com:6399").unwrap().is_none());
    }
}

#[test]
fn discovery_is_deduplicated_and_sorted() {
    let out = "[{\"name\":\"B\",\"target\":\"B\"},{\"name\":\"A\",\"target\":\"A\"},\
               {\"name\":\"A\",\"target\":\"A\"},{\"name\":\"C\",\"target\":\"X\"}]";
    assert_eq!(parse_discovered(out), ["A", "B"]);
    assert!(parse_discovered("[{\"name\":1}]").is_empty());
}

fn status_doc(state: &str, video: &str, audio: &str, stamp: &str) -> String {
    format!(
        "{{\"schema\":1,\"state\":\"{state}\",\"video_state\":\"{video}\",\"audio_state\":\
         \"{audio}\",\"target\":\"Camera\",\"detail\":\"ok\",\"connector\":\"none\",\
         \"drm_device\":\"none\",\"alsa_device\":\"none\",\"updated_at\":\"{stamp}\"}}"
    )
}

#[test]
fn status_records_must_be_fresh_and_consistent() {
    let (now, _) = wall_clock();
    let stamp = rfc3339_millis(now, 0);
    let doc = format!(
        "{{\"schema\":1,\"state\":\"running\",\"video_state\":\"running\",\"audio_state\":\
         \"running\",\"target\":\"Camera\",\"detail\":\"ok\",\"connector\":\"HDMI-A-1\",\
         \"drm_device\":\"/dev/dri/card0\",\"alsa_device\":\"x\",\"updated_at\":\"{stamp}\"}}"
    );
    let (state, _) = status_record_valid(&doc, "Camera", 5).unwrap();
    assert_eq!(state, "running");
    assert!(status_record_valid(&doc, "Other", 5).is_none());
    let old = status_doc("running", "running", "running", &rfc3339_millis(now - 60, 0));
    assert!(status_record_valid(&old, "Camera", 5).is_none());
    assert_eq!(parse_rfc3339("2026-01-01T00:00:00.250Z"), Some((1_767_225_600, 250_000_000)));
    assert_eq!(parse_rfc3339("2026-01-01T01:00:00+01:00").map(|t| t.0), Some(1_767_225_600));
    assert!(parse_rfc3339("2026-02-30T00:00:00Z").is_none());
    assert!(parse_rfc3339("2026-01-01 00:00:00Z").is_none());

    // The consumer's accept-list is exactly the shared contract's.
    let path =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/schema/playback-status-vectors.json");
    let raw = fsio::read_bounded(path, 1 << 20).unwrap().unwrap();
    let root = json::parse(&raw).unwrap();
    let states = root.get("receiver_states").and_then(json::Value::as_array).unwrap();
    assert_eq!(states.len(), 8);
    for st in states {
        let st = st.as_str().unwrap();
        let video = if st == "running" || st == "degraded" { "running" } else { st };
        let audio = if st == "degraded" { "failed" } else { "stopped" };
        let doc = status_doc(st, video, audio, &rfc3339_millis(wall_clock().0, 0));
        assert!(status_record_valid(&doc, "Camera", 5).is_some(), "{st}");
    }
}

#[test]
fn forms_and_cookies() {
    let f = Form::parse(b"csrf_token=abc&password=a+b%20c%2&empty=&noeq&password=last");
    assert_eq!(f.get("csrf_token"), Some("abc"));
    assert_eq!(f.get("password"), Some("last"));
    assert_eq!(f.get("empty"), Some(""));
    assert_eq!(f.get("noeq"), Some(""));
    let f = Form::parse(b"source=discovered%7CCaf%C3%A9&x=%2");
    assert_eq!(f.get("source"), Some("discovered|Caf\u{e9}"));
    assert_eq!(f.get("x"), Some("%2"));
    assert!(Form::parse(b"a=1&b=%FF").is_empty());
    let name = "__Host-omt_session";
    assert_eq!(cookie_value("a=1; __Host-omt_session=abc; b=2", name, 63).as_deref(), Some("abc"));
    assert_eq!(cookie_value("__Host-omt_session=", name, 63), None);
    assert_eq!(cookie_value("x__Host-omt_session=abc", name, 63), None);
    assert_eq!(cookie_value("__Host-omt_session=abcd", name, 3), None);
}

#[test]
fn the_rate_limiter_is_a_sliding_window() {
    let mut rates = RateLimiter::default();
    let two = RateLimit { count: 2, window_ms: 200 };
    assert!(rates.allow("login", "10.0.0.1", two));
    assert!(rates.allow("login", "10.0.0.1", two));
    assert!(!rates.allow("login", "10.0.0.1", two));
    assert!(rates.allow("login", "10.0.0.2", two));
    assert!(rates.allow("reboot", "10.0.0.1", two));
    thread::sleep(Duration::from_millis(250));
    assert!(rates.allow("login", "10.0.0.1", two));

    // A full table fails closed for new keys.
    let mut rates = RateLimiter::default();
    let hour = RateLimit { count: 1, window_ms: 3_600_000 };
    for i in 0..RATE_KEYS {
        assert!(rates.allow("login", &format!("10.{}.{}.1", i / 256, i % 256), hour));
    }
    assert!(!rates.allow("login", "192.0.2.1", hour));
    assert_eq!(rates.keys(), RATE_KEYS);
}

fn base_context() -> Value {
    let mut ctx = Value::map();
    ctx.set_bool("authenticated", false);
    ctx.set_str("csrf_token", "t");
    ctx.set_str("endpoint", "auth.login");
    ctx.set("flashes", Value::List(Vec::new()));
    ctx
}

#[test]
fn html_templates_escape_untrusted_values() {
    let mut ctx = base_context();
    ctx.set_str("hostname", "<script>alert(1)</script>");
    ctx.set_str("error", "it's \"quoted\" & <b>");
    let mut out = String::new();
    assert!(render("login.html", &ctx, &mut out));
    assert!(out.contains("&lt;script&gt;alert(1)&lt;&#x2f;script&gt; OMT Client"));
    assert!(out.contains("it&#x27;s &quot;quoted&quot; &amp; &lt;b&gt;"));
    assert!(!out.contains("<script>"));
    assert!(!render("missing.html", &ctx, &mut out));
}

#[test]
fn templates_render_every_page() {
    let mut ctx = Value::map();
    let flash = Value::List(vec![Value::str("success"), Value::str("Saved.")]);
    ctx.set("flashes", Value::List(vec![flash]));
    ctx.set_bool("authenticated", true);
    ctx.set_str("endpoint", "system.reboot");
    let mut limit = Value::map();
    limit.set_bool("overridden", true);
    limit.set_str("effective", "1280x720@60");
    ctx.set("video_limit", limit);
    let mut command = Value::map();
    command.set("returncode", Value::None);
    command.set("duration_seconds", Value::Float(0.25));
    command.set_bool("timed_out", false);
    let mut result = Value::map();
    result.set("command", command);
    ctx.set("result", result);
    for page in [
        "about.html",
        "dashboard.html",
        "diagnostics.html",
        "error.html",
        "login.html",
        "network.html",
        "reboot_confirm.html",
        "reboot_scheduled.html",
        "system.html",
    ] {
        let mut out = String::new();
        assert!(render(page, &ctx, &mut out), "{page}");
        assert!(out.contains("</html>"));
        if page == "system.html" {
            assert!(!out.contains("playout") && !out.contains("Playout"));
            assert!(out.contains("href=\"/system\" aria-current=\"page\""));
            assert!(out.contains("<li class=\"flash flash-success\">Saved.</li>"));
        }
        if page == "diagnostics.html" {
            assert!(out.contains("<dd>None</dd>"));
            assert!(out.contains("<dd>false</dd>"));
            assert!(out.contains("<dd>0.25s</dd>"));
            assert!(out.contains("&lt;empty&gt;"));
        }
    }
}

#[test]
fn template_floats_render_as_minijinja_does() {
    for (value, text) in [(1.0, "1.0"), (0.25, "0.25"), (1e21, "1000000000000000000000.0")] {
        let mut ctx = base_context();
        let mut command = Value::map();
        command.set("duration_seconds", Value::Float(value));
        let mut result = Value::map();
        result.set("command", command);
        ctx.set("result", result);
        let mut out = String::new();
        assert!(render("diagnostics.html", &ctx, &mut out));
        assert!(out.contains(&format!("<dd>{text}s</dd>")), "{value}");
    }
}

#[test]
fn zip_members_round_trip() {
    // Deflate is checked bit-exactly by the Python reader in test_web.sh;
    // here the archive structure and CRCs are pinned.
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    let mut z = Zip::new(1 << 20);
    z.add("a.txt", b"hello hello hello hello hello world\n", Method::Deflated);
    z.add("b.bin", b"\x00\x01\x02", Method::Stored);
    assert!(z.members[0].compressed < z.members[0].size);
    let out = z.finish().unwrap();
    assert!(out.len() > 22);
    assert_eq!(u32::from_le_bytes(out[..4].try_into().unwrap()), 0x0403_4b50);
    let end = out.len() - 22;
    assert_eq!(u32::from_le_bytes(out[end..end + 4].try_into().unwrap()), 0x0605_4b50);
}

#[test]
fn http_heads_are_strict() {
    let ok =
        b"POST /login?x=1 HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\nCookie: a=b\r\n\r\nhello";
    let Parse::Complete(head) = parse_head(ok) else { panic!("complete head") };
    assert_eq!(head.method, "POST");
    assert_eq!(head.path, "/login");
    assert_eq!(head.content_length, 5);
    assert!(head.keep_alive);
    let req = Request::new(&head, ok, "127.0.0.1");
    assert_eq!(req.header("COOKIE"), Some(&b"a=b"[..]));
    assert_eq!(req.body, b"hello");
    assert!(matches!(parse_head(b"GET / HTTP/1.1\r\nHost: a\r\n"), Parse::Incomplete));
    for (text, want) in [
        (&b"GET / HTTP/2.0\r\n\r\n"[..], 505),
        (b"GET http://x/ HTTP/1.1\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n", 411),
        (b"GET / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\n folded: x\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nBad Name: x\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nContent-Length: -1\r\n\r\n", 400),
        (b"G\x01T / HTTP/1.1\r\n\r\n", 400),
    ] {
        assert!(
            matches!(parse_head(text), Parse::Refuse(s) if s == want),
            "{}",
            String::from_utf8_lossy(text)
        );
    }
    let close = b"GET / HTTP/1.1\r\nConnection: keep-alive, close\r\n\r\n";
    assert!(matches!(parse_head(close), Parse::Complete(h) if !h.keep_alive));
    assert!(matches!(parse_head(b"GET / HTTP/1.0\r\n\r\n"), Parse::Complete(h) if !h.keep_alive));
    let mut big = vec![b'a'; MAX_HEADER_BYTES + 64];
    big[..19].copy_from_slice(b"GET / HTTP/1.1\r\nX: ");
    assert!(matches!(parse_head(&big), Parse::Refuse(431)));
}

#[test]
fn records_are_strict() {
    let fields = ["version", "request_id", "status"];
    let ok = "version=1\nrequest_id=abc\nstatus=complete\n\nbody text=with equals\n";
    assert_eq!(parse_record(ok, &fields, true).unwrap()[1], "abc");
    let dup = "version=1\nversion=1\nrequest_id=abc\nstatus=complete\n";
    assert!(parse_record(dup, &fields, true).is_none());
    let extra = "version=1\nrequest_id=abc\nstatus=complete\nmore=1\n";
    assert!(parse_record(extra, &fields, true).is_none());
    assert!(parse_record("version=1\nrequest_id=abc\n", &fields, true).is_none());
    assert!(parse_record(ok, &fields, false).is_none());
}

#[test]
fn settings_parse_rate_limits() {
    let r = |s: &str| parse_rate_limit(s);
    assert_eq!(r("5 per minute"), Some(RateLimit { count: 5, window_ms: 60_000 }));
    assert_eq!(r("10   per   hours").map(|l| l.window_ms), Some(3_600_000));
    assert_eq!(r("1 per seconds").map(|l| l.window_ms), Some(1000));
    for bad in ["0 per minute", "5 each minute", "5 per fortnight", "5 per minute extra"] {
        assert!(r(bad).is_none(), "{bad}");
    }
}
