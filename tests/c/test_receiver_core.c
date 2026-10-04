/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Port of the omt-receiver-core tests, including the shared status vectors.
 */
#include <stdlib.h>
#include <unistd.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/timefmt.h"
#include "receiver_core/core.h"
#include "test.h"

static omt_video_ceiling ceiling(const char *text) {
    omt_video_ceiling c;
    omt_err err;
    CHECK_MSG(omt_ceiling_parse(text, &c, &err), "%s: %s", text, err.msg);
    return c;
}

static bool admits(const omt_video_ceiling *c, int w, int h, double rate) {
    omt_err err;
    return omt_ceiling_admits(c, w, h, rate, &err);
}

static void status_on_tmpfs_skips_fsync(void) {
    CHECK(!omt_durable_status_parent("/run/omt/state"));
    CHECK(!omt_durable_status_parent("/run"));
    CHECK(omt_durable_status_parent("/etc/omt/run"));
    CHECK(omt_durable_status_parent("/tmp/omt"));
    CHECK(omt_durable_status_parent("/running"));
}

static void board_tiers_admit_their_intended_formats(void) {
    omt_video_ceiling pi5 = ceiling("1920x1080@60"), pi4 = ceiling("1920x1080@30,1280x720@60"),
                      pi3 = ceiling("1280x720@60");
    CHECK(admits(&pi5, 1920, 1080, 60.0));
    CHECK(admits(&pi5, 1280, 720, 60.0));
    CHECK(admits(&pi4, 1920, 1080, 30.0));
    CHECK(admits(&pi4, 1280, 720, 60.0));
    CHECK(!admits(&pi4, 1920, 1080, 60.0));
    CHECK(admits(&pi3, 1280, 720, 60.0));
    CHECK(admits(&pi3, 1280, 720, 30.0));
    CHECK(!admits(&pi3, 1920, 1080, 30.0));
    CHECK(!admits(&pi3, 1920, 1080, 60.0));
    omt_video_ceiling tier = ceiling("1280x720@60");
    CHECK(admits(&tier, 1280, 720, 60000.0 / 1001.0));
    CHECK(!admits(&tier, 1280, 720, 60.5));
    omt_video_ceiling wide = ceiling("1920x1080@30");
    CHECK(admits(&wide, 640, 480, 25.0));
    CHECK(admits(&wide, 1920, 240, 30.0));
    CHECK(admits(&wide, 16, 16, 1.0));
}

static void a_refusal_names_the_stream_and_the_ceiling(void) {
    omt_video_ceiling c = ceiling("1280x720@60");
    omt_err err;
    CHECK(!omt_ceiling_admits(&c, 1920, 1080, 59.94, &err));
    CHECK_STR(err.msg,
              "1920x1080 at 59.94 fps exceeds this appliance's limit of 1280x720 at 60 fps.");
    omt_buf d;
    omt_buf_init(&d, 256);
    omt_video_ceiling two = ceiling("1920x1080@30,1280x720@60");
    omt_ceiling_describe(&two, &d);
    CHECK_STR(omt_buf_cstr(&d), "1920x1080 at 30 fps, or 1280x720 at 60 fps");
    omt_buf_free(&d);
}

static void rejects_malformed_and_out_of_range_ceilings(void) {
    const char *bad[] = {"",
                         "1920x1080",
                         "1920X1080@60",
                         "1920x1080@60Hz",
                         "1921x1080@60",
                         "1920x1081@60",
                         "1920x1080@61",
                         "3840x2160@30",
                         "15x15@60",
                         "1920x1080@0",
                         "0x1080@60",
                         "0640x480@30",
                         "+640x480@30",
                         " 640x480@30",
                         "640x480@30 ",
                         "1920x1080@60,",
                         ",1920x1080@60",
                         "1920x1080@60,,1280x720@30",
                         "1920x1080@60 1280x720@30",
                         "12345x1080@60",
                         "640x480@25,800x600@30,1280x720@50,1920x1080@30,640x360@24"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(bad); i++) {
        omt_video_ceiling c;
        omt_err err;
        CHECK_MSG(!omt_ceiling_parse(bad[i], &c, &err), "accepted [%s]", bad[i]);
    }
    omt_err err;
    omt_video_ceiling c;
    CHECK(!omt_ceiling_parse("1920x1080@61", &c, &err));
    CHECK_STR(err.msg,
              "Video ceiling 1920x1080@61 is outside the supported 1920x1080 at 60 fps maximum.");
    CHECK(!omt_ceiling_parse("abc", &c, &err));
    CHECK_STR(err.msg, "Invalid video ceiling: abc. Expected WIDTHxHEIGHT@FPS.");
    const char *good[] = {"1920x1080@60", "1920x1080@30,1280x720@60", "1280x720@60",
                          "640x480@25,800x600@30,1280x720@50,1920x1080@30"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(good); i++) (void)ceiling(good[i]);
}

static void detail_contract(void) {
    char out[OMT_DETAIL_LIMIT + 1];
    omt_sanitize_detail("  a\nb  ", out);
    CHECK_STR(out, "ab");
    char big[4097];
    memset(big, 'x', 4096);
    big[4096] = 0;
    omt_sanitize_detail(big, out);
    CHECK_INT(strlen(out), OMT_DETAIL_LIMIT);
}

static void format_timestamp_contract(void) {
    struct {
        int64_t s;
        uint32_t ms;
        const char *expected;
    } cases[] = {{0, 0, "1970-01-01T00:00:00.000Z"},
                 {1, 7, "1970-01-01T00:00:01.007Z"},
                 {86399, 999, "1970-01-01T23:59:59.999Z"},
                 {86400, 0, "1970-01-02T00:00:00.000Z"},
                 {951782400, 0, "2000-02-29T00:00:00.000Z"},
                 {4107456000, 0, "2100-02-28T00:00:00.000Z"},
                 {4107456000 + 86400, 0, "2100-03-01T00:00:00.000Z"},
                 {1767225600, 250, "2026-01-01T00:00:00.250Z"},
                 {2147483648, 0, "2038-01-19T03:14:08.000Z"}};
    for (size_t i = 0; i < OMT_ARRAY_LEN(cases); i++) {
        char out[32];
        omt_format_rfc3339_millis(cases[i].s, cases[i].ms, out);
        CHECK_STR(out, cases[i].expected);
        int64_t y;
        unsigned m, d;
        omt_civil_from_days(cases[i].s / 86400, &y, &m, &d);
        CHECK_INT(omt_days_from_civil(y, m, d), cases[i].s / 86400);
    }
}

static char tmpdir[256];

static omt_json *read_doc(const char *path, omt_json_doc *doc, omt_buf *raw) {
    omt_err err;
    if (omt_read_bounded(path, 1 << 20, raw, &err) != OMT_READ_OK) return NULL;
    return omt_json_parse(doc, omt_buf_cstr(raw), raw->len, 0);
}

static void shared_status_vectors(void) {
    omt_json_doc vdoc;
    omt_buf vraw;
    omt_json *vectors = read_doc("tests/schema/playback-status-vectors.json", &vdoc, &vraw);
    CHECK(vectors != NULL);
    if (!vectors) return;
    const omt_json *fields = omt_json_get(vectors, "fields");
    const omt_json *projections = omt_json_get(vectors, "projections");
    for (size_t i = 0; i < projections->count; i++) {
        const omt_json *v = projections->items[i];
        char path[512];
        snprintf(path, sizeof(path), "%s/v%zu/status.json", tmpdir, i);
        omt_playback_status status;
        CHECK(omt_status_init(&status, path, "Camera"));
        omt_connector none;
        omt_connector_none(&none);
        omt_err err;
        const omt_json *events = omt_json_get(v, "events");
        for (size_t e = 0; e < events->count; e++) {
            const char *ev = events->items[e]->string;
            if (!strcmp(ev, "AudioRunning"))
                (void)omt_status_audio(&status, OMT_AUDIO_RUNNING, "", &none, &err);
            else if (!strcmp(ev, "AudioFailed"))
                (void)omt_status_audio(&status, OMT_AUDIO_FAILED, "Audio unavailable", &none, &err);
            else if (!strcmp(ev, "AudioStopped"))
                (void)omt_status_audio(&status, OMT_AUDIO_STOPPED, "", &none, &err);
            else if (!strcmp(ev, "VideoStarting"))
                (void)omt_status_video(&status, OMT_VIDEO_STARTING, "", &none, &err);
            else if (!strcmp(ev, "WaitingForDiscovery"))
                (void)omt_status_video(&status, OMT_VIDEO_WAITING_FOR_DISCOVERY, "", &none, &err);
            else if (!strcmp(ev, "WaitingForHdmi"))
                (void)omt_status_video(&status, OMT_VIDEO_WAITING_FOR_HDMI, "", &none, &err);
            else if (!strcmp(ev, "VideoRetrying"))
                (void)omt_status_video(&status, OMT_VIDEO_RETRYING, "", &none, &err);
            else if (!strcmp(ev, "UnsupportedFormat"))
                (void)omt_status_video(&status, OMT_VIDEO_UNSUPPORTED_FORMAT, "", &none, &err);
            else if (!strcmp(ev, "VideoRunning"))
                (void)omt_status_video(&status, OMT_VIDEO_RUNNING, "", &none, &err);
            else if (!strcmp(ev, "Stopped"))
                (void)omt_status_stopped(&status, "", &err);
            else
                CHECK_MSG(false, "unknown event %s", ev);
        }
        if (access(path, F_OK) != 0) (void)omt_status_stopped(&status, "Playback stopped.", &err);
        omt_json_doc doc;
        omt_buf raw;
        omt_json *d = read_doc(path, &doc, &raw);
        CHECK(d != NULL);
        if (d) {
            const char *name = omt_json_get(v, "name")->string;
            CHECK_MSG(!strcmp(omt_json_get(d, "state")->string, omt_json_get(v, "state")->string),
                      "%s", name);
            CHECK_MSG(!strcmp(omt_json_get(d, "video_state")->string,
                              omt_json_get(v, "video_state")->string),
                      "%s", name);
            CHECK_MSG(!strcmp(omt_json_get(d, "audio_state")->string,
                              omt_json_get(v, "audio_state")->string),
                      "%s", name);
            CHECK_INT(d->count, fields->count);
            for (size_t f = 0; f < fields->count; f++)
                CHECK_MSG(omt_json_get(d, fields->items[f]->string) != NULL, "%s missing %s", name,
                          fields->items[f]->string);
            CHECK_INT(strlen(omt_json_get(d, "updated_at")->string), 24);
            omt_json_doc_free(&doc);
            omt_buf_free(&raw);
        }
        omt_status_destroy(&status);
        unlink(path);
        char dir[512];
        snprintf(dir, sizeof(dir), "%s/v%zu", tmpdir, i);
        rmdir(dir);
    }
    /* The producer's state names must be exactly the consumer's accept-list. */
    const omt_json *states = omt_json_get(vectors, "video_states");
    CHECK_INT(states->count, OMT_VIDEO_STATE_COUNT);
    for (int s = 0; s < OMT_VIDEO_STATE_COUNT; s++) {
        bool found = false;
        for (size_t k = 0; k < states->count; k++)
            found |= !strcmp(states->items[k]->string, omt_video_state_name((omt_video_state)s));
        CHECK_MSG(found, "%s", omt_video_state_name((omt_video_state)s));
    }
    omt_json_doc_free(&vdoc);
    omt_buf_free(&vraw);
}

static void heartbeat_republishes_after_the_interval(void) {
    char path[512];
    snprintf(path, sizeof(path), "%s/hb/status.json", tmpdir);
    omt_playback_status s;
    CHECK(omt_status_init(&s, path, "Camera"));
    omt_connector none;
    omt_connector_none(&none);
    omt_err err;
    CHECK_INT(omt_status_video(&s, OMT_VIDEO_RUNNING, "Playing OMT video.", &none, &err), 1);
    CHECK_INT(omt_status_video(&s, OMT_VIDEO_RUNNING, "Playing OMT video.", &none, &err), 0);
    CHECK_INT(omt_status_heartbeat(&s, &none, &err), 0);
    usleep((OMT_HEARTBEAT_MS + 20) * 1000);
    CHECK_INT(omt_status_heartbeat(&s, &none, &err), 1);
    omt_json_doc doc;
    omt_buf raw;
    omt_json *d = read_doc(path, &doc, &raw);
    CHECK(d && !strcmp(omt_json_get(d, "state")->string, "running"));
    if (d) {
        omt_json_doc_free(&doc);
        omt_buf_free(&raw);
    }
    omt_status_destroy(&s);
    unlink(path);
    snprintf(path, sizeof(path), "%s/hb", tmpdir);
    rmdir(path);
}

int main(void) {
    const char *t = getenv("TMPDIR");
    snprintf(tmpdir, sizeof(tmpdir), "%s/omt-core-XXXXXX", t ? t : "/tmp");
    if (!mkdtemp(tmpdir)) return 1;
    RUN(status_on_tmpfs_skips_fsync);
    RUN(board_tiers_admit_their_intended_formats);
    RUN(a_refusal_names_the_stream_and_the_ceiling);
    RUN(rejects_malformed_and_out_of_range_ceilings);
    RUN(detail_contract);
    RUN(format_timestamp_contract);
    RUN(shared_status_vectors);
    RUN(heartbeat_republishes_after_the_interval);
    rmdir(tmpdir);
    return TEST_EXIT();
}
