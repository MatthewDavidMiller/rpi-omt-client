/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/settings.h"

#include <math.h>
#include <stdlib.h>
#include <string.h>

#include "common/json.h"

extern char **environ;

static const char *value(const char *name, const char *def) {
    const char *v = getenv(name);
    return v ? v : def;
}

static bool path(char *out, const char *name, const char *def) {
    return omt_strlcpy(out, value(name, def), OMT_PATH_MAX);
}

static bool join(char *out, const char *dir, const char *leaf) {
    return omt_snprintf(out, OMT_PATH_MAX, "%s/%s", dir, leaf);
}

/* u64::from_str: digits with an optional leading '+'. */
static bool parse_u64(const char *raw, uint64_t *out) {
    const char *t = raw[0] == '+' ? raw + 1 : raw;
    return omt_parse_u64(t, strlen(t), UINT64_MAX, out);
}

static bool integer(const char *name, uint64_t def, uint64_t minimum, uint64_t *out, omt_err *err) {
    char text[32];
    snprintf(text, sizeof(text), "%llu", (unsigned long long)def);
    const char *raw = value(name, text);
    if (!parse_u64(raw, out) || *out < minimum) {
        omt_err_set(err, "%s must be an integer of at least %llu; received \"%s\"", name,
                    (unsigned long long)minimum, raw);
        return false;
    }
    return true;
}

/* f64::from_str accepts decimal and exponent forms and "inf"/"nan", which
 * the finiteness check then refuses. */
static bool seconds(const char *name, double def, bool allow_zero, double *out, omt_err *err) {
    omt_buf text;
    omt_buf_init(&text, 64);
    omt_fmt_f64_display(&text, def);
    const char *raw = value(name, omt_buf_cstr(&text));
    char *end = NULL;
    /* strtod also takes leading space and hex floats, which Rust does not. */
    bool ok = raw[0] != 0 && !strchr(" \t\n\r\f\v", raw[0]) && !strpbrk(raw, "xX");
    double v = ok ? strtod(raw, &end) : 0;
    ok = ok && end && *end == 0 && isfinite(v) && (allow_zero ? v >= 0.0 : v > 0.0);
    if (!ok)
        omt_err_set(err, "%s must be a finite positive number; received \"%s\"", name, raw);
    else
        *out = v;
    omt_buf_free(&text);
    return ok;
}

static bool boolean(const char *name, bool def, bool *out, omt_err *err) {
    const char *raw = value(name, def ? "1" : "0");
    omt_span t = omt_utf8_trim(raw, strlen(raw));
    const char *yes[] = {"1", "true", "yes", "on"}, *no[] = {"0", "false", "no", "off"};
    for (size_t i = 0; i < 4; i++) {
        if (omt_ascii_ieq((const char *)t.p, t.len, yes[i])) {
            *out = true;
            return true;
        }
        if (omt_ascii_ieq((const char *)t.p, t.len, no[i])) {
            *out = false;
            return true;
        }
    }
    omt_err_set(err, "%s must be a boolean; received \"%s\"", name, raw);
    return false;
}

bool omt_rate_limit_parse(const char *raw, omt_rate_limit *out) {
    /* Exactly three whitespace-separated words: COUNT per UNIT[s]. */
    char words[3][64];
    size_t n = 0;
    const char *p = raw;
    while (*p) {
        while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r' || *p == '\f' || *p == '\v') p++;
        if (!*p) break;
        const char *start = p;
        while (*p &&
               !(*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r' || *p == '\f' || *p == '\v'))
            p++;
        if (n == 3 || (size_t)(p - start) >= sizeof(words[0])) return false;
        memcpy(words[n], start, (size_t)(p - start));
        words[n][p - start] = 0;
        n++;
    }
    if (n != 3 || strcmp(words[1], "per") != 0) return false;
    uint64_t count;
    if (!parse_u64(words[0], &count) || count == 0) return false;
    /* trim_end_matches('s') strips every trailing 's'. */
    size_t len = strlen(words[2]);
    while (len && words[2][len - 1] == 's') len--;
    words[2][len] = 0;
    uint64_t unit;
    if (!strcmp(words[2], "second"))
        unit = 1;
    else if (!strcmp(words[2], "minute"))
        unit = 60;
    else if (!strcmp(words[2], "hour"))
        unit = 3600;
    else if (!strcmp(words[2], "day"))
        unit = 86400;
    else
        return false;
    out->count = (size_t)count;
    out->window_ms = unit * 1000;
    return true;
}

static bool rate(const char *name, const char *def, omt_rate_limit *out, omt_err *err) {
    const char *raw = value(name, def);
    if (omt_rate_limit_parse(raw, out)) return true;
    omt_err_set(err, "%s must be a rate limit such as \"%s\"; received \"%s\"", name, def, raw);
    return false;
}

bool omt_web_settings_load(omt_web_settings *s, omt_err *err) {
    memset(s, 0, sizeof(*s));
    /* Retired diagnostics switches are refused by name, so a stale compose
     * file cannot silently run without what it asked for. */
    omt_buf obsolete;
    omt_buf_init(&obsolete, 8192);
    const char *names[64];
    size_t count = 0;
    for (char **e = environ; e && *e && count < 64; e++) {
        if (omt_has_prefix(*e, "PIPELINE_STATUS_STALE_SECONDS=") ||
            omt_has_prefix(*e, "OMT_DEBUG_") || omt_has_prefix(*e, "OMT_HOST_DEBUG_"))
            names[count++] = *e;
    }
    if (count) {
        /* Sorted by name, as the Rust frontend reported them. */
        for (size_t i = 1; i < count; i++)
            for (size_t j = i; j > 0 && strcmp(names[j - 1], names[j]) > 0; j--) {
                const char *t = names[j - 1];
                names[j - 1] = names[j];
                names[j] = t;
            }
        for (size_t i = 0; i < count; i++) {
            if (i) omt_buf_puts(&obsolete, ", ");
            const char *eq = strchr(names[i], '=');
            omt_buf_append(&obsolete, names[i], eq ? (size_t)(eq - names[i]) : strlen(names[i]));
        }
        omt_err_set(err, "Obsolete diagnostics settings are not supported: %s",
                    omt_buf_cstr(&obsolete));
        omt_buf_free(&obsolete);
        return false;
    }
    omt_buf_free(&obsolete);

    char def[OMT_PATH_MAX];
    bool ok = path(s->config_dir, "OMT_CONFIG_DIR", "/etc/omt");
    ok = ok && join(def, s->config_dir, "run") && path(s->runtime_dir, "OMT_RUNTIME_DIR", def);
    ok = ok && join(def, s->config_dir, "omt") && path(s->sdk_config_dir, "OMT_STORAGE_PATH", def);
    ok = ok && join(def, s->sdk_config_dir, "settings.xml") &&
         path(s->runtime_config_file, "OMT_RUNTIME_CONFIG_FILE", def);
    if (!ok) {
        omt_err_set(err, "a configured path is too long");
        return false;
    }
    if (!seconds("OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS", 30.0, false,
                 &s->diagnostics_host_timeout_s, err) ||
        !seconds("OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS", 60.0, false,
                 &s->diagnostics_bundle_budget_s, err))
        return false;
    if (s->diagnostics_bundle_budget_s > 85.0) {
        omt_err_set(err, "OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS must be at most 85");
        return false;
    }
    if (s->diagnostics_host_timeout_s > s->diagnostics_bundle_budget_s) {
        omt_err_set(err, "OMT_DIAGNOSTICS_HOST_TIMEOUT_SECONDS must not exceed "
                         "OMT_DIAGNOSTICS_BUNDLE_BUDGET_SECONDS");
        return false;
    }
    const char *port_raw = value("WEB_PORT", "5000");
    uint64_t port;
    if (!parse_u64(port_raw, &port) || port > 65535) {
        omt_err_set(err, "WEB_PORT is invalid: \"%s\"", port_raw);
        return false;
    }
    s->web_port = (uint16_t)port;
    uint64_t max_request;
    char ssl[OMT_PATH_MAX];
    ok =
        join(def, s->config_dir, "web_password") &&
        path(s->password_file, "OMT_PASSWORD_FILE", def) &&
        integer("OMT_SESSION_LIFETIME_SECONDS", 43200, 1, &s->session_lifetime_s, err) &&
        integer("OMT_MAX_REQUEST_BYTES", 16384, 1024, &max_request, err) &&
        rate("OMT_LOGIN_RATE_LIMIT", "5 per minute", &s->login_limit, err) &&
        rate("OMT_DIAGNOSTICS_DOWNLOAD_LIMIT", "10 per hour", &s->diagnostic_download_limit, err) &&
        rate("OMT_DIAGNOSTICS_ACTION_LIMIT", "30 per hour", &s->diagnostic_action_limit, err) &&
        rate("OMT_REBOOT_ACTION_LIMIT", "3 per hour", &s->reboot_limit, err) &&
        path(s->control_command, "OMT_CONTROL_COMMAND", "/usr/local/bin/control-omt.sh") &&
        path(s->receiver_command, "OMT_RECEIVER_COMMAND", "/usr/local/bin/omt-receiver") &&
        seconds("OMT_CONTROL_TIMEOUT_SECONDS", 15.0, false, &s->control_timeout_s, err) &&
        seconds("OMT_SOURCE_CACHE_TTL_SECONDS", 5.0, true, &s->source_cache_ttl_s, err) &&
        join(def, s->config_dir, "source_target.json") &&
        path(s->source_target_file, "OMT_SOURCE_TARGET_FILE", def) &&
        join(def, s->config_dir, "video_ceiling.json") &&
        path(s->video_ceiling_file, "OMT_VIDEO_CEILING_FILE", def) &&
        join(def, s->config_dir, "playout_delay.json") &&
        path(s->playout_delay_file, "OMT_PLAYOUT_DELAY_FILE", def) &&
        omt_strlcpy(s->board_label, value("OMT_BOARD_LABEL", "Raspberry Pi"),
                    sizeof(s->board_label)) &&
        omt_strlcpy(s->board_video_ceiling, value("OMT_VIDEO_CEILING", "1920x1080@60"),
                    sizeof(s->board_video_ceiling)) &&
        join(def, s->runtime_dir, "playback-status.json") &&
        path(s->playback_status_file, "OMT_PLAYBACK_STATUS_FILE", def) &&
        integer("OMT_PLAYBACK_STATUS_STALE_SECONDS", 5, 1, &s->playback_status_stale_s, err) &&
        path(s->diagnostics_host_report_file, "OMT_DIAGNOSTICS_HOST_REPORT_FILE",
             "/host-diagnostics/host-report.txt") &&
        path(s->diagnostics_host_request_file, "OMT_DIAGNOSTICS_HOST_REQUEST_FILE",
             "/host-diagnostics/request") &&
        path(s->diagnostics_host_pcap_file, "OMT_DIAGNOSTICS_HOST_PCAP_FILE",
             "/host-diagnostics/host-network.pcap") &&
        path(s->diagnostics_host_pcap_metadata_file, "OMT_DIAGNOSTICS_HOST_PCAP_METADATA_FILE",
             "/host-diagnostics/host-network-pcap.txt") &&
        integer("OMT_DIAGNOSTICS_HOST_BUDGET_SECONDS", 25, 1, &s->diagnostics_host_budget, err) &&
        boolean("OMT_DIAGNOSTICS_RECEIVE_PROBE", true, &s->diagnostics_receive_probe, err) &&
        path(s->version_file, "RPI_OMT_CLIENT_VERSION_FILE", "/app/RPI_OMT_CLIENT_VERSION") &&
        path(s->runtime_integrity_manifest, "OMT_RUNTIME_INTEGRITY_MANIFEST",
             "/app/runtime-sha256.manifest") &&
        path(s->project_license_file, "OMT_PROJECT_LICENSE_FILE", "/app/legal/LICENSE") &&
        path(s->third_party_notices_file, "OMT_THIRD_PARTY_NOTICES_FILE",
             "/app/legal/THIRD_PARTY_NOTICES.txt") &&
        path(s->reboot_request_file, "OMT_REBOOT_REQUEST_FILE", "/host-actions/reboot.request") &&
        path(s->reboot_result_file, "OMT_REBOOT_RESULT_FILE", "/host-actions/reboot.result") &&
        seconds("OMT_REBOOT_ACK_TIMEOUT_SECONDS", 3.0, false, &s->reboot_ack_timeout_s, err) &&
        join(ssl, s->config_dir, "ssl") && join(def, ssl, "cert.pem") &&
        path(s->tls_cert_file, "OMT_TLS_CERT_FILE", def) && join(def, ssl, "key.pem") &&
        path(s->tls_key_file, "OMT_TLS_KEY_FILE", def);
    if (!ok) {
        if (err && !err->msg[0]) omt_err_set(err, "a configured path is too long");
        return false;
    }
    s->max_request_bytes = (size_t)max_request;
    return true;
}

static void seconds_line(omt_buf *out, const char *name, double v) {
    omt_buf_printf(out, "%s=", name);
    omt_fmt_f64_display(out, v);
    omt_buf_putc(out, '\n');
}

void omt_web_settings_diagnostic_lines(const omt_web_settings *s, omt_buf *out) {
    omt_buf_printf(out, "session_lifetime_seconds=%llu\n",
                   (unsigned long long)s->session_lifetime_s);
    omt_buf_printf(out, "max_request_bytes=%zu\n", s->max_request_bytes);
    seconds_line(out, "control_timeout_seconds", s->control_timeout_s);
    seconds_line(out, "source_cache_ttl_seconds", s->source_cache_ttl_s);
    omt_buf_printf(out, "playback_status_stale_seconds=%llu\n",
                   (unsigned long long)s->playback_status_stale_s);
    seconds_line(out, "diagnostics_host_timeout_seconds", s->diagnostics_host_timeout_s);
    omt_buf_printf(out, "diagnostics_host_budget_seconds=%llu\n",
                   (unsigned long long)s->diagnostics_host_budget);
    seconds_line(out, "diagnostics_bundle_budget_seconds", s->diagnostics_bundle_budget_s);
    omt_buf_printf(out, "diagnostics_receive_probe_enabled=%s\n",
                   s->diagnostics_receive_probe ? "true" : "false");
    omt_buf_printf(out, "board_label=%s\n", s->board_label);
    omt_buf_printf(out, "board_video_ceiling=%s\n", s->board_video_ceiling);
    omt_buf_printf(out, "sdk_config_dir=%s\n", s->sdk_config_dir);
    omt_buf_printf(out, "runtime_config_file=%s\n", s->runtime_config_file);
    seconds_line(out, "reboot_ack_timeout_seconds", s->reboot_ack_timeout_s);
}
