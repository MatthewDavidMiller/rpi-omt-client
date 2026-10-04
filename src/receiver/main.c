/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * omt-receiver: discover, probe, and play OMT sources.
 *
 * The CLI is a trust boundary: control-omt.sh and the Web diagnostics both
 * build argument vectors for it, so every rejection is a named usage failure
 * (exit 2) that leaves the command unrun. Playback behaviour is derived from
 * the MIT-licensed Open Media Transport projects; see
 * third_party/omt/PROVENANCE.md.
 */
#include <errno.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common/json.h"
#include "common/proc.h"
#include "common/version.h"
#include "receiver/discovery.h"
#include "receiver/play.h"

/* More options than any subcommand accepts, which bounds the parse. */
#define MAX_OPTIONS 8

typedef struct {
    const char *key;
    const char *value; /* NULL for the --json flag */
} option;

typedef struct {
    option items[MAX_OPTIONS];
    size_t count;
} options;

static int usage(void) {
    fprintf(stderr,
            "Usage: omt-receiver --version | discover --wait-ms N --json | probe --target TARGET "
            "--timeout-ms N --json | play --target TARGET --connector auto|HDMI-A-1|HDMI-A-2 "
            "--status-file PATH --video-ceiling WIDTHxHEIGHT@FPS[,...]\n");
    return 2;
}

static const option *find(const options *o, const char *key) {
    for (size_t i = 0; i < o->count; i++)
        if (strcmp(o->items[i].key, key) == 0) return &o->items[i];
    return NULL;
}

/* Parses `--key value` pairs and the single `--json` flag. */
static bool parse(int argc, char **argv, options *out, omt_err *err) {
    out->count = 0;
    for (int i = 0; i < argc; i++) {
        const char *key = argv[i];
        if (!omt_has_prefix(key, "--")) {
            omt_err_set(err, "Unexpected argument: %s", key);
            return false;
        }
        const char *value = NULL;
        if (strcmp(key, "--json") != 0) {
            if (++i >= argc) {
                omt_err_set(err, "Missing value for %s", key);
                return false;
            }
            value = argv[i];
        }
        if (find(out, key)) {
            omt_err_set(err, "Duplicate option: %s", key);
            return false;
        }
        if (out->count == MAX_OPTIONS) {
            omt_err_set(err, "Too many options.");
            return false;
        }
        out->items[out->count++] = (option){key, value};
    }
    return true;
}

/* Rejects any option the subcommand does not define, reporting the first in
 * sorted order. */
static bool allowed(const options *o, const char *const *names, size_t count, omt_err *err) {
    const char *offending = NULL;
    for (size_t i = 0; i < o->count; i++) {
        bool known = false;
        for (size_t k = 0; k < count && !known; k++) known = strcmp(o->items[i].key, names[k]) == 0;
        if (!known && (!offending || strcmp(o->items[i].key, offending) < 0))
            offending = o->items[i].key;
    }
    if (offending) {
        omt_err_set(err, "Option %s is not valid for this command.", offending);
        return false;
    }
    return true;
}

static const char *required(const options *o, const char *name, omt_err *err) {
    const option *opt = find(o, name);
    if (!opt || !opt->value || !*opt->value) {
        omt_err_set(err, "%s is required.", name);
        return NULL;
    }
    return opt->value;
}

static bool flag(const options *o, const char *name, omt_err *err) {
    const option *opt = find(o, name);
    if (opt && !opt->value) return true;
    omt_err_set(err, "%s is required.", name);
    return false;
}

static bool number(const options *o, const char *name, uint64_t def, uint64_t min, uint64_t max,
                   uint64_t *out, omt_err *err) {
    const option *opt = find(o, name);
    if (!opt) {
        *out = def;
        return true;
    }
    if (!opt->value) {
        omt_err_set(err, "%s requires a value.", name);
        return false;
    }
    /* u64::from_str: ASCII digits with an optional leading '+'. */
    const char *text = opt->value[0] == '+' ? opt->value + 1 : opt->value;
    uint64_t v;
    if (!omt_parse_u64(text, strlen(text), UINT64_MAX, &v) || v < min || v > max) {
        omt_err_set(err, "%s must be between %llu and %llu.", name, (unsigned long long)min,
                    (unsigned long long)max);
        return false;
    }
    *out = v;
    return true;
}

static int discover(const options *o, omt_err *err) {
    const char *names[] = {"--wait-ms", "--json"};
    uint64_t wait;
    if (!allowed(o, names, 2, err) || !flag(o, "--json", err) ||
        !number(o, "--wait-ms", 1500, 0, 60000, &wait, err))
        return -1;
    omt_source_set *set = malloc(sizeof(*set));
    if (!set) {
        omt_err_set(err, "out of memory");
        return -1;
    }
    omt_discover_sources(wait, set);
    omt_buf out;
    omt_buf_init(&out, 1 << 20);
    omt_buf_putc(&out, '[');
    for (size_t i = 0; i < set->count; i++) {
        if (i) omt_buf_putc(&out, ',');
        omt_buf_puts(&out, "{\"name\":");
        omt_json_write_cstr(&out, set->items[i].name);
        omt_buf_puts(&out, ",\"target\":");
        omt_json_write_cstr(&out, set->items[i].name);
        omt_buf_puts(&out, ",\"kind\":\"discovered\"}");
    }
    omt_buf_putc(&out, ']');
    printf("%s\n", omt_buf_cstr(&out));
    omt_buf_free(&out);
    free(set);
    return 0;
}

typedef struct {
    bool video, audio;
    int32_t width, height, channels, sample_rate;
    double frame_rate;
} measurement;

/* Reads within the caller's bounded share looking for the wanted type. */
static bool sample(omt_channel *c, omt_frame_type wanted, uint64_t deadline, omt_err *error) {
    while (omt_now_ms() < deadline) {
        omt_err err;
        omt_recv_status st = omt_channel_receive(c, deadline, &err);
        if (st == OMT_RECV_OK) {
            if (c->frame.header.frame_type == wanted) return true;
        } else {
            *error = err;
            return false;
        }
    }
    return false;
}

/* A video frame can legitimately take longer than a fixed short slice on a
 * busy link, so video gets half of what is left while audio is still owed a
 * share, and all of it otherwise. */
static uint64_t fair_sample_deadline(uint64_t deadline, bool reserve_other) {
    return reserve_other ? omt_now_ms() + omt_remaining_ms(deadline) / 2 : deadline;
}

/* Subscribes to both media types and reports whatever arrives, so a
 * video-only or audio-only sender still probes as reachable. */
static void measure(const omt_endpoint *ep, uint64_t deadline, measurement *m, omt_err *error) {
    omt_channel video, audio;
    omt_channel_init(&video);
    omt_channel_init(&audio);
    omt_err err;
    bool video_connected = omt_channel_connect(&video, ep, OMT_FRAME_VIDEO, deadline, &err);
    if (!video_connected) *error = err;
    bool audio_connected = omt_channel_connect(&audio, ep, OMT_FRAME_AUDIO, deadline, &err);
    if (!audio_connected) *error = err;
    while (omt_now_ms() < deadline && !(m->video && m->audio)) {
        uint64_t video_deadline = fair_sample_deadline(deadline, !m->audio && audio_connected);
        if (!m->video && video_connected &&
            sample(&video, OMT_FRAME_VIDEO, video_deadline, error) && video.frame.has_video) {
            m->video = true;
            m->width = video.frame.video.width;
            m->height = video.frame.video.height;
            m->frame_rate =
                (double)video.frame.video.frame_rate_n / (double)video.frame.video.frame_rate_d;
        }
        if (!m->audio && audio_connected && sample(&audio, OMT_FRAME_AUDIO, deadline, error) &&
            audio.frame.has_audio) {
            m->audio = true;
            m->channels = audio.frame.audio.channels;
            m->sample_rate = audio.frame.audio.sample_rate;
        }
        if (!video_connected && !audio_connected) break;
    }
    omt_channel_free(&video);
    omt_channel_free(&audio);
}

static int probe(const options *o, omt_err *err) {
    const char *names[] = {"--target", "--timeout-ms", "--json"};
    uint64_t timeout;
    const char *target;
    if (!allowed(o, names, 3, err) || !flag(o, "--json", err) ||
        !(target = required(o, "--target", err)) ||
        !number(o, "--timeout-ms", 3000, 1, 60000, &timeout, err))
        return -1;
    if (!omt_is_valid_target(target, strlen(target))) {
        omt_err_set(err, "Invalid OMT direct target.");
        return -1;
    }
    uint64_t deadline = omt_now_ms() + timeout;
    measurement m;
    memset(&m, 0, sizeof(m));
    omt_err failure;
    failure.msg[0] = 0;
    /* Half the budget discovers; the rest is left for the measurement. */
    omt_endpoint ep;
    if (!omt_discover_resolve(target, timeout / 2, &ep)) {
        omt_err_set(&failure, "OMT target was not discovered.");
    } else {
        omt_err channel_error;
        channel_error.msg[0] = 0;
        measure(&ep, deadline, &m, &channel_error);
        if (!(m.video || m.audio)) {
            if (channel_error.msg[0])
                failure = channel_error;
            else
                omt_err_set(&failure, "No OMT media was received.");
        }
    }
    bool ok = m.video || m.audio;
    char sanitized[OMT_DETAIL_LIMIT + 1];
    omt_sanitize_detail(failure.msg, sanitized);
    omt_buf out;
    omt_buf_init(&out, 64 * 1024);
    omt_buf_printf(&out, "{\"ok\":%s,\"target\":", ok ? "true" : "false");
    omt_json_write_cstr(&out, target);
    omt_buf_printf(&out, ",\"video\":%s,\"audio\":%s,\"width\":%d,\"height\":%d,\"frame_rate\":",
                   m.video ? "true" : "false", m.audio ? "true" : "false", m.width, m.height);
    omt_json_write_f64(&out, m.frame_rate);
    omt_buf_printf(&out, ",\"channels\":%d,\"sample_rate\":%d,\"error\":", m.channels,
                   m.sample_rate);
    omt_json_write_cstr(&out, sanitized);
    omt_buf_putc(&out, '}');
    printf("%s\n", omt_buf_cstr(&out));
    omt_buf_free(&out);
    return ok ? 0 : 3;
}

static atomic_bool stop_requested;

static void on_signal(int signal_number) {
    (void)signal_number;
    atomic_store(&stop_requested, true);
}

static int play(const options *o, omt_err *err) {
    const char *names[] = {"--target", "--connector", "--status-file", "--retry-seconds",
                           "--video-ceiling"};
    const char *target, *path;
    uint64_t retry;
    if (!allowed(o, names, 5, err) || !(target = required(o, "--target", err)) ||
        !(path = required(o, "--status-file", err)) ||
        !number(o, "--retry-seconds", 2, 1, 30, &retry, err))
        return -1;
    const option *pref = find(o, "--connector");
    const char *preference = pref && pref->value ? pref->value : "auto";
    /* The default is the Pi 5 tier; the installer always passes the board's
     * own ceiling, so this only applies to a hand-run receiver. */
    const option *ceiling_opt = find(o, "--video-ceiling");
    omt_play_options po;
    memset(&po, 0, sizeof(po));
    if (!omt_ceiling_parse(ceiling_opt && ceiling_opt->value ? ceiling_opt->value : "1920x1080@60",
                           &po.ceiling, err))
        return -1;
    if (!omt_is_valid_target(target, strlen(target)) ||
        !(strcmp(preference, "auto") == 0 || strcmp(preference, "HDMI-A-1") == 0 ||
          strcmp(preference, "HDMI-A-2") == 0)) {
        omt_err_set(err, "Invalid play options.");
        return -1;
    }
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = on_signal;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGINT, &sa, NULL) != 0 || sigaction(SIGTERM, &sa, NULL) != 0) {
        omt_err e;
        omt_err_os(&e, errno);
        omt_err_set(err, "Unable to install the shutdown handler: %s", e.msg);
        return -1;
    }
    omt_playback_status status;
    if (!omt_status_init(&status, path, target)) {
        omt_err_set(err, "out of memory");
        return -1;
    }
    po.target = target;
    po.preference = preference;
    po.retry_ms = retry * 1000;
    omt_play_run(&po, &status, &stop_requested);
    omt_status_destroy(&status);
    return 0;
}

int main(int argc, char **argv) {
    /* As the Rust runtime does: a closed pipe is an error return, not a kill. */
    signal(SIGPIPE, SIG_IGN);
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        printf("%s\n", omt_version);
        return 0;
    }
    if (argc < 2) return usage();
    options o;
    omt_err err;
    if (!parse(argc - 2, argv + 2, &o, &err)) {
        fprintf(stderr, "%s\n", err.msg);
        return 2;
    }
    int code;
    if (strcmp(argv[1], "discover") == 0)
        code = discover(&o, &err);
    else if (strcmp(argv[1], "probe") == 0)
        code = probe(&o, &err);
    else if (strcmp(argv[1], "play") == 0)
        code = play(&o, &err);
    else
        return usage();
    if (code < 0) {
        fprintf(stderr, "%s\n", err.msg);
        return 2;
    }
    return code;
}
