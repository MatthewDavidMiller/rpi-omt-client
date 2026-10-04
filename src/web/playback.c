/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/playback.h"

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/timefmt.h"

static uint64_t ms_of(double seconds) { return seconds <= 0 ? 0 : (uint64_t)(seconds * 1000.0); }

void omt_playback_init(omt_playback *p, const omt_web_settings *s) {
    memset(p, 0, sizeof(*p));
    p->settings = s;
    p->cache_expires_ms = omt_now_ms();
}

void omt_control(const omt_web_settings *s, const char *action, double timeout_s,
                 omt_proc_result *out) {
    const char *argv[] = {s->control_command, action, NULL};
    omt_proc_options o = {argv, ms_of(timeout_s), 0, NULL, 0, NULL};
    omt_proc_run(&o, out);
}

static bool control_ok(omt_proc_result *r) { return r->has_returncode && r->returncode == 0; }

OMT_PRINTF(2, 3) static void success(omt_action_result *out, const char *fmt, ...) {
    out->ok = true;
    va_list args;
    va_start(args, fmt);
    vsnprintf(out->message, sizeof(out->message), fmt, args);
    va_end(args);
    out->error[0] = 0;
}

static void failure_text(omt_action_result *out, const char *text) {
    out->ok = false;
    out->message[0] = 0;
    snprintf(out->error, sizeof(out->error), "%s", text);
}

void omt_playback_configuration(const omt_playback *p, omt_source_configuration *out) {
    memset(out, 0, sizeof(*out));
    omt_source_target t;
    omt_err err;
    int r = omt_read_source(p->settings->source_target_file, &t, &err);
    if (r < 0) {
        omt_strlcpy(out->error, err.msg, sizeof(out->error));
    } else if (r > 0) {
        omt_strlcpy(out->source, t.value, sizeof(out->source));
        if (t.direct) omt_strlcpy(out->direct_address, t.value, sizeof(out->direct_address));
    }
}

static int compare_names(const void *a, const void *b) { return strcmp(a, b); }

void omt_parse_discovered(const char *output, size_t len, omt_source_choices *out) {
    out->count = 0;
    if (len > 256 * 1024) return;
    omt_json_doc doc;
    omt_json *root = omt_json_parse(&doc, output, len, 0);
    if (!root) return;
    bool ok = root->type == OMT_JSON_ARRAY;
    /* Every entry must have string name and target, or the whole answer is
     * refused, as serde refused it. */
    for (size_t i = 0; ok && i < root->count; i++) {
        const omt_json *e = root->items[i];
        ok = e->type == OMT_JSON_OBJECT && omt_json_get(e, "name") &&
             omt_json_get(e, "name")->type == OMT_JSON_STRING && omt_json_get(e, "target") &&
             omt_json_get(e, "target")->type == OMT_JSON_STRING;
    }
    for (size_t i = 0; ok && i < root->count && out->count < OMT_MAX_CHOICES; i++) {
        const omt_json *name = omt_json_get(root->items[i], "name");
        const omt_json *target = omt_json_get(root->items[i], "target");
        if (name->string_len != target->string_len ||
            memcmp(name->string, target->string, name->string_len) != 0 ||
            !omt_is_valid_source_name(name->string, name->string_len))
            continue;
        bool seen = false;
        for (size_t k = 0; k < out->count && !seen; k++)
            seen = strcmp(out->names[k], name->string) == 0;
        if (!seen) omt_strlcpy(out->names[out->count++], name->string, sizeof(out->names[0]));
    }
    qsort(out->names, out->count, sizeof(out->names[0]), compare_names);
    omt_json_doc_free(&doc);
}

void omt_playback_sources(omt_playback *p, omt_source_choices *out) {
    if (omt_now_ms() < p->cache_expires_ms) {
        *out = p->cache;
        return;
    }
    const char *argv[] = {
        p->settings->receiver_command, "discover", "--wait-ms", "1500", "--json", NULL};
    double timeout = p->settings->control_timeout_s > 3.0 ? p->settings->control_timeout_s : 3.0;
    omt_proc_options o = {argv, ms_of(timeout), 0, NULL, 0, NULL};
    omt_proc_result r;
    omt_proc_run(&o, &r);
    out->count = 0;
    if (r.has_returncode && r.returncode == 0)
        omt_parse_discovered(omt_buf_cstr(&r.out), r.out.len, out);
    omt_proc_result_free(&r);
    p->cache = *out;
    p->cache_expires_ms = omt_now_ms() + ms_of(p->settings->source_cache_ttl_s);
}

void omt_playback_refresh(omt_playback *p) {
    p->cache_expires_ms = omt_now_ms();
    p->cache.count = 0;
}

static void save_and_restart(omt_playback *p, const omt_source_target *t, const char *label,
                             omt_action_result *out) {
    omt_err err;
    if (!omt_save_source(p->settings->source_target_file, t, &err)) {
        failure_text(out, err.msg);
        return;
    }
    omt_playback_refresh(p);
    omt_proc_result r;
    omt_control(p->settings, "restart", p->settings->control_timeout_s, &r);
    if (control_ok(&r)) {
        success(out, "%s saved and running.", label);
    } else {
        omt_buf scratch;
        omt_buf_init(&scratch, 512 * 1024);
        out->ok = false;
        out->message[0] = 0;
        snprintf(out->error, sizeof(out->error),
                 "%s was saved, but playback could not be restarted. %s", label,
                 omt_proc_failure_detail(&r, &scratch));
        omt_buf_free(&scratch);
    }
    omt_proc_result_free(&r);
}

void omt_playback_select(omt_playback *p, const char *selection, omt_action_result *out) {
    omt_span t = omt_utf8_trim(selection, strlen(selection));
    char trimmed[OMT_TARGET_MAX_BYTES * 2];
    if (t.len >= sizeof(trimmed)) {
        failure_text(out, "Invalid OMT source selection.");
        return;
    }
    memcpy(trimmed, t.p, t.len);
    trimmed[t.len] = 0;
    omt_source_target target;
    memset(&target, 0, sizeof(target));
    if (omt_has_prefix(trimmed, "discovered|") &&
        omt_is_valid_source_name(trimmed + 11, strlen(trimmed + 11))) {
        omt_strlcpy(target.value, trimmed + 11, sizeof(target.value));
        save_and_restart(p, &target, "OMT discovery", out);
        return;
    }
    omt_direct_target d;
    if (omt_has_prefix(trimmed, "direct|") &&
        omt_parse_direct_target(trimmed + 7, strlen(trimmed + 7), &d, NULL)) {
        target.direct = true;
        omt_strlcpy(target.value, trimmed + 7, sizeof(target.value));
        save_and_restart(p, &target, "OMT direct target", out);
        return;
    }
    if (omt_is_valid_source_name(trimmed, strlen(trimmed))) {
        omt_strlcpy(target.value, trimmed, sizeof(target.value));
        save_and_restart(p, &target, "OMT discovery", out);
        return;
    }
    failure_text(out, "Invalid OMT source selection.");
}

void omt_playback_save_direct(omt_playback *p, const char *address, omt_action_result *out) {
    omt_direct_target d;
    if (!omt_parse_direct_target(address, strlen(address), &d, NULL)) {
        failure_text(out, "Direct target must use omt://host:port with no path or credentials.");
        return;
    }
    omt_source_target target;
    memset(&target, 0, sizeof(target));
    target.direct = true;
    omt_strlcpy(target.value, address, sizeof(target.value));
    save_and_restart(p, &target, "OMT direct target", out);
}

void omt_playback_restart(omt_playback *p, omt_action_result *out) {
    omt_source_target t;
    omt_err err;
    int r = omt_read_source(p->settings->source_target_file, &t, &err);
    if (r < 0) {
        out->ok = false;
        out->message[0] = 0;
        snprintf(out->error, sizeof(out->error), "Saved OMT target is invalid: %s", err.msg);
        return;
    }
    if (r == 0) {
        failure_text(out, "No OMT source is configured.");
        return;
    }
    omt_proc_result c;
    omt_control(p->settings, "restart", p->settings->control_timeout_s, &c);
    if (control_ok(&c)) {
        success(out, "%s", "OMT playback restarted.");
    } else {
        omt_buf scratch;
        omt_buf_init(&scratch, 512 * 1024);
        out->ok = false;
        out->message[0] = 0;
        snprintf(out->error, sizeof(out->error), "Unable to restart OMT playback. %s",
                 omt_proc_failure_detail(&c, &scratch));
        omt_buf_free(&scratch);
    }
    omt_proc_result_free(&c);
}

void omt_playback_clear(omt_playback *p, omt_action_result *out) {
    omt_proc_result c;
    omt_control(p->settings, "stop", p->settings->control_timeout_s, &c);
    bool stopped = c.has_returncode && (c.returncode == 0 || c.returncode == 3);
    if (!stopped) {
        omt_buf scratch;
        omt_buf_init(&scratch, 512 * 1024);
        out->ok = false;
        out->message[0] = 0;
        snprintf(out->error, sizeof(out->error),
                 "Playback could not be stopped, so the saved target was retained. %s",
                 omt_proc_failure_detail(&c, &scratch));
        omt_buf_free(&scratch);
        omt_proc_result_free(&c);
        return;
    }
    omt_proc_result_free(&c);
    omt_err err;
    if (!omt_save_source(p->settings->source_target_file, NULL, &err)) {
        out->ok = false;
        out->message[0] = 0;
        snprintf(out->error, sizeof(out->error),
                 "Playback stopped, but the saved target could not be cleared. %s", err.msg);
        return;
    }
    omt_playback_refresh(p);
    success(out, "%s", "Playback stopped and the saved target was cleared.");
}

void omt_playback_video_limit(const omt_playback *p, omt_video_limit *out) {
    memset(out, 0, sizeof(*out));
    const char *board_default = p->settings->board_video_ceiling;
    omt_strlcpy(out->board_label, p->settings->board_label, sizeof(out->board_label));
    omt_strlcpy(out->board_default, board_default, sizeof(out->board_default));
    omt_buf effective, d;
    omt_buf_init(&effective, 256);
    omt_buf_init(&d, 512);
    omt_err err;
    if (omt_effective_video_ceiling(p->settings->video_ceiling_file, board_default, &effective,
                                    &err)) {
        omt_strlcpy(out->effective, omt_buf_cstr(&effective), sizeof(out->effective));
        out->overridden = strcmp(out->effective, board_default) != 0;
        out->above_board_default =
            out->overridden && omt_pixel_rate(out->effective) > omt_pixel_rate(board_default);
        omt_describe_video_ceiling(out->effective, &d);
    } else {
        omt_strlcpy(out->effective, board_default, sizeof(out->effective));
        omt_strlcpy(out->error, err.msg, sizeof(out->error));
        omt_describe_video_ceiling(board_default, &d);
    }
    omt_strlcpy(out->effective_description, omt_buf_cstr(&d), sizeof(out->effective_description));
    omt_buf_clear(&d);
    omt_describe_video_ceiling(board_default, &d);
    omt_strlcpy(out->board_default_description, omt_buf_cstr(&d),
                sizeof(out->board_default_description));
    omt_buf_free(&effective);
    omt_buf_free(&d);
}

static void restart_after(omt_playback *p, const char *label, omt_action_result *out) {
    omt_proc_result c;
    omt_control(p->settings, "restart", p->settings->control_timeout_s, &c);
    if (control_ok(&c)) {
        success(out, "%s and playback restarted.", label);
    } else {
        omt_buf scratch;
        omt_buf_init(&scratch, 512 * 1024);
        out->ok = false;
        out->message[0] = 0;
        snprintf(out->error, sizeof(out->error), "%s, but playback could not be restarted. %s",
                 label, omt_proc_failure_detail(&c, &scratch));
        omt_buf_free(&scratch);
    }
    omt_proc_result_free(&c);
}

void omt_playback_save_video_limit(omt_playback *p, const char *value, omt_action_result *out) {
    omt_span t = omt_utf8_trim(value, strlen(value));
    char requested[256];
    if (t.len >= sizeof(requested)) {
        failure_text(out, "A video limit must list between 1 and 4 resolutions.");
        return;
    }
    memcpy(requested, t.p, t.len);
    requested[t.len] = 0;
    omt_err err;
    if (!omt_save_video_ceiling(p->settings->video_ceiling_file, t.len ? requested : NULL, &err)) {
        failure_text(out, err.msg);
        return;
    }
    char label[700];
    if (t.len == 0) {
        omt_strlcpy(label, "Video limit cleared", sizeof(label));
    } else {
        omt_buf d;
        omt_buf_init(&d, 512);
        omt_describe_video_ceiling(requested, &d);
        snprintf(label, sizeof(label), "Video limit set to %s", omt_buf_cstr(&d));
        omt_buf_free(&d);
    }
    restart_after(p, label, out);
}

static bool member(const char *value, const char *const *set, size_t n) {
    for (size_t i = 0; i < n; i++)
        if (strcmp(value, set[i]) == 0) return true;
    return false;
}

bool omt_status_record_valid(const char *json, size_t len, const char *expected,
                             uint64_t stale_seconds, char *state, size_t state_size, char *detail,
                             size_t detail_size) {
    static const char *const receiver_states[] = {
        "running",  "waiting-for-discovery", "waiting-for-hdmi", "retrying",
        "degraded", "unsupported-format",    "starting",         "stopped"};
    static const char *const video_states[] = {
        "running",  "waiting-for-discovery", "waiting-for-hdmi",
        "retrying", "unsupported-format",    "starting",
        "stopped"};
    static const char *const audio_states[] = {"stopped", "running", "failed"};
    static const char *const connectors[] = {"none", "HDMI-A-1", "HDMI-A-2"};
    static const char *const fields[] = {"schema",      "state",     "video_state", "audio_state",
                                         "target",      "detail",    "connector",   "drm_device",
                                         "alsa_device", "updated_at"};
    omt_json_doc doc;
    omt_json *root = omt_json_parse(&doc, json, len, 0);
    if (!root) return false;
    uint64_t schema;
    const char *s = omt_json_as_str(omt_json_get(root, "state"));
    const char *v = omt_json_as_str(omt_json_get(root, "video_state"));
    const char *a = omt_json_as_str(omt_json_get(root, "audio_state"));
    const char *target = omt_json_as_str(omt_json_get(root, "target"));
    const char *d = omt_json_as_str(omt_json_get(root, "detail"));
    const char *connector = omt_json_as_str(omt_json_get(root, "connector"));
    const char *drm = omt_json_as_str(omt_json_get(root, "drm_device"));
    const char *alsa = omt_json_as_str(omt_json_get(root, "alsa_device"));
    const char *updated = omt_json_as_str(omt_json_get(root, "updated_at"));
    bool ok =
        root->type == OMT_JSON_OBJECT && omt_json_only_keys(root, fields, 10) &&
        root->count == 10 && omt_json_as_u64(omt_json_get(root, "schema"), &schema) &&
        schema == 1 && s && v && a && target && d && connector && drm && alsa && updated &&
        member(s, receiver_states, 8) && member(v, video_states, 7) && member(a, audio_states, 3) &&
        strcmp(target, expected) == 0 && strlen(d) <= 2048 && member(connector, connectors, 3) &&
        drm[0] && strlen(drm) <= 256 && alsa[0] && strlen(alsa) <= 256 &&
        !(strcmp(s, "degraded") == 0 && (strcmp(v, "running") != 0 || strcmp(a, "failed") != 0)) &&
        !(strcmp(s, "running") == 0 && (strcmp(v, "running") != 0 || strcmp(a, "failed") == 0)) &&
        (strcmp(s, "running") == 0 || strcmp(s, "degraded") == 0 || strcmp(s, v) == 0);
    if (ok) {
        int64_t at;
        uint32_t nanos;
        int64_t now_s;
        uint32_t now_n;
        ok = omt_parse_rfc3339(updated, &at, &nanos);
        if (ok) {
            omt_wall_clock(&now_s, &now_n);
            /* Age in nanoseconds; a record up to five seconds in the future is
             * accepted to tolerate clock skew between the two processes. */
            __int128 age = ((__int128)now_s - at) * 1000000000 + ((__int128)now_n - nanos);
            ok = age >= -(__int128)5000000000 && age <= (__int128)stale_seconds * 1000000000;
        }
    }
    if (ok) {
        omt_strlcpy(state, s, state_size);
        omt_strlcpy(detail, d, detail_size);
    }
    omt_json_doc_free(&doc);
    return ok;
}

static void summary(omt_playback_summary *out, const char *state, const char *label,
                    const char *detail, const char *tone, const char *source, const char *direct) {
    omt_strlcpy(out->state, state, sizeof(out->state));
    omt_strlcpy(out->label, label, sizeof(out->label));
    omt_strlcpy(out->detail, detail, sizeof(out->detail));
    omt_strlcpy(out->tone, tone, sizeof(out->tone));
    omt_strlcpy(out->source, source, sizeof(out->source));
    omt_strlcpy(out->direct_address, direct, sizeof(out->direct_address));
}

void omt_playback_summary_read(omt_playback *p, omt_playback_summary *out) {
    memset(out, 0, sizeof(*out));
    omt_source_target t;
    omt_err err;
    int r = omt_read_source(p->settings->source_target_file, &t, &err);
    if (r < 0) {
        summary(out, "configuration-error", "Source configuration invalid", err.msg, "danger", "",
                "");
        return;
    }
    if (r == 0) {
        summary(out, "unconfigured", "No source configured",
                "Select a discovered source or configure a direct OMT target.", "neutral", "", "");
        return;
    }
    const char *direct = t.direct ? t.value : "";
    omt_buf data;
    if (omt_read_bounded(p->settings->playback_status_file, 4096, &data, &err) != OMT_READ_OK) {
        omt_proc_result c;
        omt_control(p->settings, "status", p->settings->control_timeout_s, &c);
        if (control_ok(&c))
            summary(out, "starting", "Starting playback",
                    "The receiver is running and has not published fresh status yet.", "warning",
                    t.value, direct);
        else
            summary(out, "stopped", "Playback stopped",
                    "A target is saved but the receiver is not running.", "neutral", t.value,
                    direct);
        omt_proc_result_free(&c);
        return;
    }
    char state[32], detail[2100];
    bool valid = omt_status_record_valid(omt_buf_cstr(&data), data.len, t.value,
                                         p->settings->playback_status_stale_s, state, sizeof(state),
                                         detail, sizeof(detail));
    omt_buf_free(&data);
    if (!valid) {
        summary(out, "stale", "Playback status stale",
                "The receiver status record is unavailable or stale.", "warning", t.value, direct);
        return;
    }
    static const struct {
        const char *state, *public_state, *label, *tone;
    } map[] = {
        {"running", "playing", "Playing", "success"},
        {"waiting-for-discovery", "waiting-for-discovery", "Waiting for discovery", "warning"},
        {"waiting-for-hdmi", "waiting-for-hdmi", "Waiting for HDMI", "warning"},
        {"retrying", "retrying", "Retrying playback", "warning"},
        {"degraded", "degraded", "Playback degraded", "warning"},
        {"unsupported-format", "unsupported-format", "Unsupported video format", "danger"},
        {"starting", "starting", "Starting playback", "warning"},
    };
    for (size_t i = 0; i < OMT_ARRAY_LEN(map); i++)
        if (strcmp(state, map[i].state) == 0) {
            summary(out, map[i].public_state, map[i].label, detail, map[i].tone, t.value, direct);
            return;
        }
    summary(out, "stopped", "Playback stopped", detail, "neutral", t.value, direct);
}
