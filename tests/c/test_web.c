/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Port of the omt-web crate's unit tests, plus the C-only pieces: forms,
 * cookies, the rate limiter, templates, the ZIP writer, and HTTP parsing.
 */
#include <stdlib.h>
#include <unistd.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/timefmt.h"
#include "test.h"
#include "web/app.h"
#include "web/auth.h"
#include "web/diagnostics.h"
#include "web/http.h"
#include "web/network.h"
#include "web/playback.h"
#include "web/settings.h"
#include "web/state.h"
#include "web/templates.h"
#include "web/tv.h"
#include "web/zip.h"

static bool verify(const char *stored, const char *supplied) {
    omt_password p;
    omt_err err;
    if (!omt_password_parse(stored, strlen(stored), &p, &err)) return false;
    bool ok = omt_password_verify(&p, supplied, strlen(supplied));
    omt_password_free(&p);
    return ok;
}

static void verifies_werkzeug_hashes(void) {
    const char *pbkdf2 =
        "pbkdf2:sha256:1$salt$120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b";
    CHECK(verify(pbkdf2, "password"));
    CHECK(!verify(pbkdf2, "no"));
    const char *scrypt =
        "scrypt:32768:8:1$07kZLpT9$d12f4706055d4d0812a754b965a9150e8c843b0ce3672d8db291df10e0b"
        "f144bc268bb049c9c3f209ea4614d5309a759eba4e123a4bd12e08daa002f95ccfe97";
    CHECK(verify(scrypt, "password"));
    CHECK(!verify(scrypt, "not-password"));
    CHECK(verify("plain secret", "plain secret"));
    CHECK(!verify("plain secret", "plain secre"));
}

static void malformed_hashes_are_rejected(void) {
    const char *bad[] = {
        "pbkdf2:sha256:1$salt$",
        "pbkdf2:sha256:1$salt$00",
        "pbkdf2:sha256:1$salt$00000000000000000000000000000000000000000000000000000000000000",
        "pbkdf2:sha256:0$salt$120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b",
        "pbkdf2:sha1:1$salt$00",
        "argon2:whatever",
        "scrypt:3:8:1$s$00112233445566778899",
        "scrypt:4:8$s$00112233445566778899",
        "scrypt:4:8:1:1$s$00112233445566778899"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(bad); i++) {
        omt_password p;
        omt_err err;
        CHECK_MSG(!omt_password_parse(bad[i], strlen(bad[i]), &p, &err), "%s", bad[i]);
    }
}

static void new_password_policy_and_encoding(void) {
    omt_err err;
    CHECK(omt_validate_new_password("correct horse battery staple", 28, &err));
    CHECK(!omt_validate_new_password("too-short", 9, &err));
    char long_pw[200];
    memset(long_pw, 'x', sizeof(long_pw));
    CHECK(!omt_validate_new_password(long_pw, 129, &err));
    CHECK(!omt_validate_new_password("twelve bytes\n", 13, &err));
    omt_buf encoded;
    omt_buf_init(&encoded, 512);
    CHECK(omt_encode_password("correct horse battery staple", 28, &encoded, &err));
    omt_span t = omt_utf8_trim(omt_buf_cstr(&encoded), encoded.len);
    char text[512];
    memcpy(text, t.p, t.len);
    text[t.len] = 0;
    CHECK(verify(text, "correct horse battery staple"));
    CHECK(!verify(text, "wrong password"));
    omt_buf_free(&encoded);
}

static void ceilings_and_delays(void) {
    omt_err err;
    CHECK(omt_parse_video_ceiling("1920x1080@60", &err));
    CHECK(!omt_parse_video_ceiling("1921x1080@60", &err));
    CHECK_STR(err.msg, "Width 1921 is outside 16-1920.");
    CHECK(!omt_parse_video_ceiling("1920x1080@0", &err));
    CHECK(!omt_parse_video_ceiling("1920x1080@999", &err));
    CHECK_STR(err.msg, "Invalid frame rate");
    CHECK(!omt_parse_video_ceiling("a,b,c,d,e", &err));
    omt_buf d;
    omt_buf_init(&d, 256);
    omt_describe_video_ceiling("1920x1080@30,1280x720@060", &d);
    CHECK_STR(omt_buf_cstr(&d), "1920x1080 at 30 fps, or 1280x720 at 60 fps");
    omt_buf_free(&d);
    CHECK(omt_pixel_rate("1920x1080@30,1280x720@60") == 1920ull * 1080 * 30);
}

static void discovery_servers_normalize(void) {
    char out[600];
    omt_err err;
    CHECK(omt_normalize_server("Example.COM", out, &err));
    CHECK_STR(out, "omt://example.com:6399");
    CHECK(omt_normalize_server("[2001:db8::1]", out, &err));
    CHECK_STR(out, "omt://[2001:db8::1]:6399");
    CHECK(omt_normalize_server("omt://[2001:0db8:0:0:0:0:0:1]:7000", out, &err));
    CHECK_STR(out, "omt://[2001:db8::1]:7000");
    CHECK(omt_normalize_server("omt://[::ffff:192.0.2.1]", out, &err));
    CHECK_STR(out, "omt://[::ffff:192.0.2.1]:6399");
    CHECK(omt_normalize_server("192.0.2.1:9", out, &err));
    CHECK_STR(out, "omt://192.0.2.1:9");
    CHECK(omt_normalize_server("  ", out, &err) && out[0] == 0);
    CHECK(!omt_normalize_server("host/path", out, &err));
    CHECK(!omt_normalize_server("2001:db8::1", out, &err));
    CHECK(!omt_normalize_server("host:0", out, &err));
    const uint8_t two_roots[] = "<Settings /><Settings />";
    const uint8_t doctype[] = "<!DOCTYPE Settings><Settings />";
    const uint8_t entity[] = "<Settings><DiscoveryServer>a&amp;b</DiscoveryServer></Settings>";
    const uint8_t dup[] =
        "<Settings><DiscoveryServer>a</DiscoveryServer><DiscoveryServer/></Settings>";
    CHECK(!omt_parse_server(two_roots, sizeof(two_roots) - 1, out, &err));
    CHECK(!omt_parse_server(doctype, sizeof(doctype) - 1, out, &err));
    CHECK(!omt_parse_server(entity, sizeof(entity) - 1, out, &err));
    CHECK(!omt_parse_server(dup, sizeof(dup) - 1, out, &err));

    /* Splicing keeps every other byte and reads back the new value. */
    const char *docs[] = {
        "<Settings />",
        "<?xml version=\"1.0\"?>\n<Settings>\n  <Other a=\"1\">x</Other>\n</Settings>\n",
        "<Settings><DiscoveryServer>old.example</DiscoveryServer><Keep/></Settings>",
        "<Settings><DiscoveryServer/></Settings>"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(docs); i++) {
        omt_buf updated;
        omt_buf_init(&updated, 65536);
        bool changed;
        CHECK(omt_update_settings_xml((const uint8_t *)docs[i], strlen(docs[i]),
                                      "omt://example.com:6399", &updated, &changed, &err));
        CHECK(changed);
        CHECK(omt_parse_server(updated.data, updated.len, out, &err));
        CHECK_STR(out, "omt://example.com:6399");
        if (i == 1) CHECK(strstr(omt_buf_cstr(&updated), "<Other a=\"1\">x</Other>") != NULL);
        if (i == 2) CHECK(strstr(omt_buf_cstr(&updated), "<Keep/>") != NULL);
        bool again;
        omt_buf second;
        omt_buf_init(&second, 65536);
        CHECK(omt_update_settings_xml(updated.data, updated.len, "omt://example.com:6399", &second,
                                      &again, &err));
        CHECK(!again);
        omt_buf_free(&second);
        omt_buf_free(&updated);
    }
}

static void discovery_is_deduplicated_and_sorted(void) {
    omt_source_choices c;
    const char *out = "[{\"name\":\"B\",\"target\":\"B\"},{\"name\":\"A\",\"target\":\"A\"},"
                      "{\"name\":\"A\",\"target\":\"A\"},{\"name\":\"C\",\"target\":\"X\"}]";
    omt_parse_discovered(out, strlen(out), &c);
    CHECK_INT(c.count, 2);
    CHECK_STR(c.names[0], "A");
    CHECK_STR(c.names[1], "B");
    omt_parse_discovered("[{\"name\":1}]", 12, &c);
    CHECK_INT(c.count, 0);
}

static void status_records_must_be_fresh_and_consistent(void) {
    int64_t now;
    uint32_t nanos;
    omt_wall_clock(&now, &nanos);
    char stamp[32];
    omt_format_rfc3339_millis(now, 0, stamp);
    char state[32], detail[2100];
    char doc[1024];
    snprintf(doc, sizeof(doc),
             "{\"schema\":1,\"state\":\"running\",\"video_state\":\"running\",\"audio_state\":"
             "\"running\","
             "\"target\":\"Camera\",\"detail\":\"ok\",\"connector\":\"HDMI-A-1\",\"drm_device\":\"/"
             "dev/dri/"
             "card0\",\"alsa_device\":\"x\",\"updated_at\":\"%s\"}",
             stamp);
    CHECK(omt_status_record_valid(doc, strlen(doc), "Camera", 5, state, sizeof(state), detail,
                                  sizeof(detail)));
    CHECK_STR(state, "running");
    CHECK(!omt_status_record_valid(doc, strlen(doc), "Other", 5, state, sizeof(state), detail,
                                   sizeof(detail)));
    omt_format_rfc3339_millis(now - 60, 0, stamp);
    snprintf(
        doc, sizeof(doc),
        "{\"schema\":1,\"state\":\"running\",\"video_state\":\"running\",\"audio_state\":"
        "\"running\","
        "\"target\":\"Camera\",\"detail\":\"ok\",\"connector\":\"HDMI-A-1\",\"drm_device\":\"d\","
        "\"alsa_device\":\"x\",\"updated_at\":\"%s\"}",
        stamp);
    CHECK(!omt_status_record_valid(doc, strlen(doc), "Camera", 5, state, sizeof(state), detail,
                                   sizeof(detail)));
    int64_t s;
    uint32_t n;
    CHECK(omt_parse_rfc3339("2026-01-01T00:00:00.250Z", &s, &n) && s == 1767225600 &&
          n == 250000000);
    CHECK(omt_parse_rfc3339("2026-01-01T01:00:00+01:00", &s, &n) && s == 1767225600);
    CHECK(!omt_parse_rfc3339("2026-02-30T00:00:00Z", &s, &n));
    CHECK(!omt_parse_rfc3339("2026-01-01 00:00:00Z", &s, &n));

    /* The consumer's accept-list is exactly the shared contract's. */
    omt_buf raw;
    omt_err err;
    CHECK_INT(omt_read_bounded("tests/schema/playback-status-vectors.json", 1 << 20, &raw, &err),
              OMT_READ_OK);
    omt_json_doc jd;
    omt_json *root = omt_json_parse(&jd, omt_buf_cstr(&raw), raw.len, 0);
    const omt_json *states = omt_json_get(root, "receiver_states");
    CHECK(states && states->count == 8);
    for (size_t i = 0; states && i < states->count; i++) {
        omt_format_rfc3339_millis(now, 0, stamp);
        const char *st = states->items[i]->string;
        const char *video = (!strcmp(st, "running") || !strcmp(st, "degraded")) ? "running" : st;
        const char *audio = !strcmp(st, "degraded") ? "failed" : "stopped";
        snprintf(doc, sizeof(doc),
                 "{\"schema\":1,\"state\":\"%s\",\"video_state\":\"%s\",\"audio_state\":\"%s\","
                 "\"target\":"
                 "\"Camera\",\"detail\":\"ok\",\"connector\":\"none\",\"drm_device\":\"none\","
                 "\"alsa_device\":"
                 "\"none\",\"updated_at\":\"%s\"}",
                 st, video, audio, stamp);
        CHECK_MSG(omt_status_record_valid(doc, strlen(doc), "Camera", 5, state, sizeof(state),
                                          detail, sizeof(detail)),
                  "%s", st);
    }
    omt_json_doc_free(&jd);
    omt_buf_free(&raw);
}

static void forms_and_cookies(void) {
    omt_form f;
    const char *body = "csrf_token=abc&password=a+b%20c%2&empty=&noeq&password=last";
    omt_form_parse((const uint8_t *)body, strlen(body), &f);
    CHECK_STR(omt_form_get(&f, "csrf_token"), "abc");
    CHECK_STR(omt_form_get(&f, "password"), "last");
    CHECK_STR(omt_form_get(&f, "empty"), "");
    CHECK_STR(omt_form_get(&f, "noeq"), "");
    omt_form_free(&f);
    const char *encoded = "source=discovered%7CCaf%C3%A9&x=%2";
    omt_form_parse((const uint8_t *)encoded, strlen(encoded), &f);
    CHECK_STR(omt_form_get(&f, "source"), "discovered|Caf\xc3\xa9");
    CHECK_STR(omt_form_get(&f, "x"), "%2");
    omt_form_free(&f);
    const char *invalid = "a=1&b=%FF";
    omt_form_parse((const uint8_t *)invalid, strlen(invalid), &f);
    CHECK_INT(f.count, 0);
    char v[64];
    CHECK(omt_cookie_value("a=1; __Host-omt_session=abc; b=2", "__Host-omt_session", v, sizeof(v)));
    CHECK_STR(v, "abc");
    CHECK(!omt_cookie_value("__Host-omt_session=", "__Host-omt_session", v, sizeof(v)));
    CHECK(!omt_cookie_value("x__Host-omt_session=abc", "__Host-omt_session", v, sizeof(v)));
}

static void the_rate_limiter_is_a_sliding_window(void) {
    omt_app app;
    memset(&app, 0, sizeof(app));
    app.rates = test_alloc(OMT_RATE_KEYS * sizeof(omt_rate_entry));
    omt_rate_limit two = {2, 200};
    CHECK(omt_app_allow(&app, "login", "10.0.0.1", two));
    CHECK(omt_app_allow(&app, "login", "10.0.0.1", two));
    CHECK(!omt_app_allow(&app, "login", "10.0.0.1", two));
    CHECK(omt_app_allow(&app, "login", "10.0.0.2", two));
    CHECK(omt_app_allow(&app, "reboot", "10.0.0.1", two));
    usleep(250 * 1000);
    CHECK(omt_app_allow(&app, "login", "10.0.0.1", two));
    for (size_t i = 0; i < app.rate_count; i++) free(app.rates[i].expiries);
    app.rate_count = 0;
    /* A full table fails closed for new keys. */
    omt_rate_limit hour = {1, 3600000};
    char peer[32];
    for (int i = 0; i < OMT_RATE_KEYS; i++) {
        snprintf(peer, sizeof(peer), "10.%d.%d.1", i / 256, i % 256);
        CHECK(omt_app_allow(&app, "login", peer, hour));
    }
    CHECK(!omt_app_allow(&app, "login", "192.0.2.1", hour));
    for (size_t i = 0; i < app.rate_count; i++) free(app.rates[i].expiries);
    free(app.rates);
}

static void html_templates_escape_untrusted_values(void) {
    tv_arena a;
    tv_arena_init(&a);
    tv *ctx = tv_map(&a);
    tv_set_str(&a, ctx, "hostname", "<script>alert(1)</script>");
    tv_set_bool(&a, ctx, "authenticated", false);
    tv_set_str(&a, ctx, "csrf_token", "t");
    tv_set_str(&a, ctx, "endpoint", "auth.login");
    tv_set(&a, ctx, "flashes", tv_list(&a));
    tv_set_str(&a, ctx, "error", "it's \"quoted\" & <b>");
    omt_buf out;
    omt_buf_init(&out, 1 << 20);
    CHECK(omt_template_render("login.html", ctx, &out));
    CHECK(strstr(omt_buf_cstr(&out), "&lt;script&gt;alert(1)&lt;&#x2f;script&gt; OMT Client") !=
          NULL);
    CHECK(strstr(omt_buf_cstr(&out), "it&#x27;s &quot;quoted&quot; &amp; &lt;b&gt;") != NULL);
    CHECK(strstr(omt_buf_cstr(&out), "<script>") == NULL);
    CHECK(!omt_template_render("missing.html", ctx, &out));
    omt_buf_free(&out);
    tv_arena_free(&a);
}

static void templates_render_every_page(void) {
    const char *pages[] = {
        "about.html",   "dashboard.html",      "diagnostics.html",      "error.html", "login.html",
        "network.html", "reboot_confirm.html", "reboot_scheduled.html", "system.html"};
    tv_arena a;
    tv_arena_init(&a);
    tv *ctx = tv_map(&a);
    tv *flashes = tv_list(&a);
    tv *flash = tv_list(&a);
    tv_push(&a, flash, tv_str(&a, "success"));
    tv_push(&a, flash, tv_str(&a, "Saved."));
    tv_push(&a, flashes, flash);
    tv_set(&a, ctx, "flashes", flashes);
    tv_set_bool(&a, ctx, "authenticated", true);
    tv_set_str(&a, ctx, "endpoint", "system.reboot");
    tv *limit = tv_map(&a);
    tv_set_bool(&a, limit, "overridden", true);
    tv_set_str(&a, limit, "effective", "1280x720@60");
    tv_set(&a, ctx, "video_limit", limit);
    tv *result = tv_map(&a);
    tv *command = tv_map(&a);
    tv_set(&a, command, "returncode", tv_none(&a));
    tv_set(&a, command, "duration_seconds", tv_float(&a, 0.25));
    tv_set(&a, command, "timed_out", tv_bool(&a, false));
    tv_set(&a, result, "command", command);
    tv_set(&a, ctx, "result", result);
    for (size_t i = 0; i < OMT_ARRAY_LEN(pages); i++) {
        omt_buf out;
        omt_buf_init(&out, 1 << 20);
        CHECK_MSG(omt_template_render(pages[i], ctx, &out), "%s", pages[i]);
        CHECK(strstr(omt_buf_cstr(&out), "</html>") != NULL);
        if (!strcmp(pages[i], "system.html")) {
            CHECK(!strstr(omt_buf_cstr(&out), "playout") && !strstr(omt_buf_cstr(&out), "Playout"));
            CHECK(strstr(omt_buf_cstr(&out), "href=\"/system\" aria-current=\"page\"") != NULL);
            CHECK(strstr(omt_buf_cstr(&out), "<li class=\"flash flash-success\">Saved.</li>") !=
                  NULL);
        }
        if (!strcmp(pages[i], "diagnostics.html")) {
            CHECK(strstr(omt_buf_cstr(&out), "<dd>None</dd>") != NULL);
            CHECK(strstr(omt_buf_cstr(&out), "<dd>false</dd>") != NULL);
            CHECK(strstr(omt_buf_cstr(&out), "<dd>0.25s</dd>") != NULL);
            CHECK(strstr(omt_buf_cstr(&out), "&lt;empty&gt;") != NULL);
        }
        omt_buf_free(&out);
    }
    tv_arena_free(&a);
}

static void zip_members_round_trip(void) {
    /* Deflate is checked bit-exactly by the Python reader in test_web.sh; here
     * the archive structure and CRCs are pinned. */
    CHECK(omt_crc32("123456789", 9) == 0xCBF43926u);
    omt_zip z;
    omt_zip_init(&z, 1 << 20);
    const char *text = "hello hello hello hello hello world\n";
    omt_zip_add(&z, "a.txt", text, strlen(text), OMT_ZIP_DEFLATED);
    omt_zip_add(&z, "b.bin", "\x00\x01\x02", 3, OMT_ZIP_STORED);
    CHECK(omt_zip_finish(&z));
    CHECK(z.out.len > 22);
    CHECK(omt_le32(z.out.data) == 0x04034b50u);
    CHECK(omt_le32(z.out.data + z.out.len - 22) == 0x06054b50u);
    CHECK(z.members[0].compressed < z.members[0].size);
    omt_zip_free(&z);
}

static void http_heads_are_strict(void) {
    omt_http_request req;
    size_t head, length;
    bool keep;
    const char *ok =
        "POST /login?x=1 HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\nCookie: a=b\r\n\r\nhello";
    CHECK_INT(omt_http_parse_head((const uint8_t *)ok, strlen(ok), &req, &head, &length, &keep), 1);
    CHECK_STR(req.method, "POST");
    CHECK_STR(req.path, "/login");
    CHECK_INT(length, 5);
    CHECK(keep);
    char v[16];
    CHECK(omt_http_header_value(&req, "COOKIE", v, sizeof(v)) && !strcmp(v, "a=b"));
    const char *partial = "GET / HTTP/1.1\r\nHost: a\r\n";
    CHECK_INT(
        omt_http_parse_head((const uint8_t *)partial, strlen(partial), &req, &head, &length, &keep),
        0);
    struct {
        const char *text;
        int want;
    } bad[] = {{"GET / HTTP/2.0\r\n\r\n", -505},
               {"GET http://x/ HTTP/1.1\r\n\r\n", -400},
               {"GET / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n", -411},
               {"GET / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n", -400},
               {"GET / HTTP/1.1\r\n folded: x\r\n\r\n", -400},
               {"GET / HTTP/1.1\r\nBad Name: x\r\n\r\n", -400},
               {"GET / HTTP/1.1\r\nContent-Length: -1\r\n\r\n", -400},
               {"G\x01T / HTTP/1.1\r\n\r\n", -400}};
    for (size_t i = 0; i < OMT_ARRAY_LEN(bad); i++)
        CHECK_MSG(omt_http_parse_head((const uint8_t *)bad[i].text, strlen(bad[i].text), &req,
                                      &head, &length, &keep) == bad[i].want,
                  "case %zu", i);
    const char *close = "GET / HTTP/1.1\r\nConnection: keep-alive, close\r\n\r\n";
    CHECK_INT(
        omt_http_parse_head((const uint8_t *)close, strlen(close), &req, &head, &length, &keep), 1);
    CHECK(!keep);
    const char *old = "GET / HTTP/1.0\r\n\r\n";
    CHECK_INT(omt_http_parse_head((const uint8_t *)old, strlen(old), &req, &head, &length, &keep),
              1);
    CHECK(!keep);
    char big[OMT_HTTP_MAX_HEADER_BYTES + 64];
    memset(big, 'a', sizeof(big));
    memcpy(big, "GET / HTTP/1.1\r\nX: ", 19);
    CHECK_INT(omt_http_parse_head((const uint8_t *)big, sizeof(big), &req, &head, &length, &keep),
              -431);
}

static void records_are_strict(void) {
    const char *const fields[] = {"version", "request_id", "status"};
    const char *v[3];
    size_t l[3];
    const char *ok = "version=1\nrequest_id=abc\nstatus=complete\n\nbody text=with equals\n";
    CHECK(omt_parse_record(ok, strlen(ok), fields, 3, true, v, l));
    CHECK(l[1] == 3 && memcmp(v[1], "abc", 3) == 0);
    const char *dup = "version=1\nversion=1\nrequest_id=abc\nstatus=complete\n";
    CHECK(!omt_parse_record(dup, strlen(dup), fields, 3, true, v, l));
    const char *extra = "version=1\nrequest_id=abc\nstatus=complete\nmore=1\n";
    CHECK(!omt_parse_record(extra, strlen(extra), fields, 3, true, v, l));
    const char *missing = "version=1\nrequest_id=abc\n";
    CHECK(!omt_parse_record(missing, strlen(missing), fields, 3, true, v, l));
    CHECK(!omt_parse_record(ok, strlen(ok), fields, 3, false, v, l));
}

static void settings_parse_rate_limits(void) {
    omt_rate_limit r;
    CHECK(omt_rate_limit_parse("5 per minute", &r) && r.count == 5 && r.window_ms == 60000);
    CHECK(omt_rate_limit_parse("10   per   hours", &r) && r.window_ms == 3600000);
    CHECK(omt_rate_limit_parse("1 per seconds", &r) && r.window_ms == 1000);
    CHECK(!omt_rate_limit_parse("0 per minute", &r));
    CHECK(!omt_rate_limit_parse("5 each minute", &r));
    CHECK(!omt_rate_limit_parse("5 per fortnight", &r));
    CHECK(!omt_rate_limit_parse("5 per minute extra", &r));
}

int main(void) {
    RUN(verifies_werkzeug_hashes);
    RUN(malformed_hashes_are_rejected);
    RUN(new_password_policy_and_encoding);
    RUN(ceilings_and_delays);
    RUN(discovery_servers_normalize);
    RUN(discovery_is_deduplicated_and_sorted);
    RUN(status_records_must_be_fresh_and_consistent);
    RUN(forms_and_cookies);
    RUN(the_rate_limiter_is_a_sliding_window);
    RUN(html_templates_escape_untrusted_values);
    RUN(templates_render_every_page);
    RUN(zip_members_round_trip);
    RUN(http_heads_are_strict);
    RUN(records_are_strict);
    RUN(settings_parse_rate_limits);
    return TEST_EXIT();
}
