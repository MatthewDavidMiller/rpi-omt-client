/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/diagnostics.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/rand.h"
#include "common/timefmt.h"
#include "common/version.h"
#include "crypto/crypto.h"
#include "web/zip.h"

static uint64_t ms_of(double s) { return s <= 0 ? 0 : (uint64_t)(s * 1000.0); }

static uint64_t now_epoch(void) {
    time_t t = time(NULL);
    return t < 0 ? 0 : (uint64_t)t;
}

static void sleep_ms(unsigned ms) {
    struct timespec ts = {0, (long)ms * 1000000L};
    nanosleep(&ts, NULL);
}

static void run(const char *const *argv, uint64_t timeout_ms, omt_proc_result *out) {
    omt_proc_options o = {argv, timeout_ms, 0, NULL, 0, NULL};
    omt_proc_run(&o, out);
}

void omt_diagnostic_result_free(omt_diagnostic_result *r) { omt_proc_result_free(&r->command); }

void omt_diagnostics_status(const omt_diagnostics *d, omt_buf *out) {
    omt_proc_result r;
    omt_control(d->settings, "status", d->settings->control_timeout_s, &r);
    omt_buf scratch;
    omt_buf_init(&scratch, 512 * 1024);
    omt_buf_puts(out, omt_proc_report_text(&r, &scratch));
    omt_buf_free(&scratch);
    omt_proc_result_free(&r);
}

void omt_diagnostics_discovery(const omt_diagnostics *d, omt_diagnostic_result *out) {
    memset(out, 0, sizeof(*out));
    omt_strlcpy(out->title, "OMT discovery check", sizeof(out->title));
    const char *argv[] = {
        d->settings->receiver_command, "discover", "--wait-ms", "3000", "--json", NULL};
    run(argv, 5000, &out->command);
    if (!(out->command.has_returncode && out->command.returncode == 0)) return;
    /* The names of a well-formed answer, unfiltered: this check reports what
     * the receiver said. */
    omt_json_doc doc;
    omt_json *root = omt_json_parse(&doc, omt_buf_cstr(&out->command.out), out->command.out.len, 0);
    if (!root) return;
    for (size_t i = 0;
         root->type == OMT_JSON_ARRAY && i < root->count && out->source_count < OMT_MAX_CHOICES;
         i++) {
        const char *name = omt_json_as_str(omt_json_get(root->items[i], "name"));
        if (name && strlen(name) <= OMT_SOURCE_NAME_MAX_BYTES)
            omt_strlcpy(out->sources[out->source_count++], name, sizeof(out->sources[0]));
    }
    omt_json_doc_free(&doc);
}

void omt_diagnostics_runtime(const omt_diagnostics *d, omt_diagnostic_result *out,
                             omt_buf *status_text) {
    memset(out, 0, sizeof(*out));
    omt_strlcpy(out->title, "Runtime check", sizeof(out->title));
    const char *version_argv[] = {d->settings->receiver_command, "--version", NULL};
    const char *status_argv[] = {d->settings->control_command, "status", NULL};
    omt_proc_result version, status;
    run(version_argv, 3000, &version);
    run(status_argv, 3000, &status);
    bool ok = version.has_returncode && version.returncode == 0 && status.has_returncode &&
              (status.returncode == 0 || status.returncode == 3);
    omt_proc_result *r = &out->command;
    memset(r, 0, sizeof(*r));
    r->command = strdup("OMT runtime checks");
    r->has_returncode = true;
    r->returncode = ok ? 0 : 1;
    omt_buf_init(&r->out, 2 * 1024 * 1024);
    omt_buf_init(&r->err, 16);
    omt_buf_printf(&r->out, "$ %s\n%s%s%s\n\n$ %s\n%s%s%s", version.command,
                   omt_buf_cstr(&version.out), omt_buf_cstr(&version.err), version.error,
                   status.command, omt_buf_cstr(&status.out), omt_buf_cstr(&status.err),
                   status.error);
    omt_buf_append(&r->err, "", 0);
    if (!ok) omt_strlcpy(r->error, "One or more runtime checks failed.", sizeof(r->error));
    r->duration_seconds = version.duration_seconds + status.duration_seconds;
    omt_buf scratch;
    omt_buf_init(&scratch, 512 * 1024);
    omt_buf_puts(status_text, omt_proc_report_text(&status, &scratch));
    omt_buf_free(&scratch);
    omt_proc_result_free(&version);
    omt_proc_result_free(&status);
}

void omt_diagnostics_direct(const omt_diagnostics *d, const char *address,
                            omt_diagnostic_result *out) {
    memset(out, 0, sizeof(*out));
    omt_strlcpy(out->title, "Direct-connect check", sizeof(out->title));
    omt_direct_target t;
    if (!omt_parse_direct_target(address, strlen(address), &t, NULL)) {
        omt_buf_init(&out->command.out, 16);
        omt_buf_init(&out->command.err, 16);
        omt_buf_append(&out->command.out, "", 0);
        omt_buf_append(&out->command.err, "", 0);
        out->command.command = strdup("");
        omt_strlcpy(out->command.error, "Invalid OMT direct target.", sizeof(out->command.error));
        out->skipped = true;
        return;
    }
    const char *argv[] = {d->settings->receiver_command,
                          "probe",
                          "--target",
                          address,
                          "--timeout-ms",
                          "3000",
                          "--json",
                          NULL};
    run(argv, 5000, &out->command);
}

bool omt_parse_record(const char *text, size_t len, const char *const *required, size_t count,
                      bool allow_body, const char **values, size_t *value_lens) {
    for (size_t i = 0; i < count; i++) values[i] = NULL;
    size_t found = 0, pos = 0;
    while (pos < len) {
        const char *line = text + pos;
        const char *nl = memchr(line, '\n', len - pos);
        size_t line_len = nl ? (size_t)(nl - line) : len - pos;
        pos += line_len + (nl ? 1 : 0);
        /* str::lines also strips a trailing carriage return. */
        if (line_len && line[line_len - 1] == '\r') line_len--;
        if (line_len == 0 && allow_body) break;
        const char *eq = memchr(line, '=', line_len);
        if (!eq || eq == line) return false;
        size_t key_len = (size_t)(eq - line);
        bool known = false;
        for (size_t i = 0; i < count; i++) {
            if (strlen(required[i]) != key_len || memcmp(required[i], line, key_len) != 0) continue;
            if (values[i]) return false;
            values[i] = eq + 1;
            value_lens[i] = line_len - key_len - 1;
            known = true;
            found++;
        }
        /* An unexpected key makes the field count wrong, which fails below. */
        if (!known) return false;
    }
    return found == count;
}

static bool field_is(const char *v, size_t len, const char *want) {
    return v && strlen(want) == len && memcmp(v, want, len) == 0;
}

static void unavailable(omt_buf *out, const char *detail) {
    omt_buf_printf(out, "unavailable: %s\n", detail);
}

static void json_error(omt_buf *out, const char *detail) {
    omt_span t = omt_utf8_trim(detail, strlen(detail));
    omt_buf_puts(out, "{\"error\":");
    if (t.len == 0)
        omt_json_write_cstr(out, "command failed");
    else
        omt_json_write_string(out, (const char *)t.p, t.len);
    omt_buf_puts(out, ",\"ok\":false}");
}

static void wait_for_host_report(const omt_diagnostics *d, const char *request_id,
                                 uint64_t deadline_ms, omt_buf *out) {
    uint64_t host_end = omt_now_ms() + ms_of(d->settings->diagnostics_host_timeout_s);
    uint64_t end = deadline_ms < host_end ? deadline_ms : host_end;
    char detail[300] = "host diagnostic report was not published";
    while (omt_now_ms() < end) {
        omt_buf report;
        omt_err err;
        omt_read_result r = omt_read_text(d->settings->diagnostics_host_report_file,
                                          16u * 1024 * 1024, &report, &err);
        if (r == OMT_READ_OK) {
            static const char *const fields[] = {"version", "request_id", "status"};
            const char *v[3];
            size_t l[3];
            if (omt_parse_record(omt_buf_cstr(&report), report.len, fields, 3, true, v, l) &&
                field_is(v[0], l[0], "1") && field_is(v[1], l[1], request_id) &&
                (field_is(v[2], l[2], "complete") || field_is(v[2], l[2], "partial"))) {
                omt_buf_append(out, report.data, report.len);
                omt_buf_free(&report);
                return;
            }
            omt_strlcpy(detail, "host diagnostic report did not match this request",
                        sizeof(detail));
            omt_buf_free(&report);
        } else if (r == OMT_READ_ERROR) {
            omt_strlcpy(detail, err.msg, sizeof(detail));
        }
        sleep_ms(50);
    }
    unavailable(out, detail);
}

/* Validates and loads the host's packet capture. Returns 1 with data, 0 when
 * the capture is unavailable (with `error`), -1 for a capture that breaks the
 * contract (which fails the whole bundle, as it did in Rust). */
static int capture(const omt_diagnostics *d, const char *request_id, omt_buf *metadata,
                   omt_buf *data, char *error, size_t error_size, omt_err *err) {
    omt_buf meta;
    omt_read_result r =
        omt_read_text(d->settings->diagnostics_host_pcap_metadata_file, 64 * 1024, &meta, err);
    if (r == OMT_READ_ERROR) return -1;
    if (r == OMT_READ_OK) omt_buf_append(metadata, meta.data, meta.len);
    static const char *const fields[] = {
        "version",        "request_id",      "capture_status",  "capture_interface",
        "capture_filter", "capture_snaplen", "capture_seconds", "max_bytes",
        "size_bytes",     "sha256",          "pcap_magic",      "tcpdump_exit_status"};
    const char *v[12];
    size_t l[12];
    bool parsed =
        r == OMT_READ_OK && omt_parse_record(omt_buf_cstr(&meta), meta.len, fields, 12, true, v, l);
    int result = 0;
    if (!parsed) {
        snprintf(error, error_size, "capture metadata schema is invalid");
    } else if (!field_is(v[0], l[0], "1") || !field_is(v[1], l[1], request_id)) {
        snprintf(error, error_size, "capture metadata does not match this request");
    } else if (!field_is(v[2], l[2], "complete") && !field_is(v[2], l[2], "time_limit") &&
               !field_is(v[2], l[2], "size_limit")) {
        snprintf(error, error_size, "%.*s", (int)l[2], v[2]);
    } else {
        char expected_max[32];
        snprintf(expected_max, sizeof(expected_max), "%d", PCAP_MAX_BYTES);
        uint64_t size;
        omt_buf pcap;
        if (!field_is(v[7], l[7], expected_max)) {
            omt_err_set(err, "capture metadata size exceeds the limit");
            result = -1;
        } else if (!omt_parse_u64(v[8], l[8], PCAP_MAX_BYTES, &size)) {
            omt_err_set(err, "capture metadata size is invalid");
            result = -1;
        } else {
            omt_read_result pr = omt_read_bounded(d->settings->diagnostics_host_pcap_file,
                                                  PCAP_MAX_BYTES, &pcap, err);
            if (pr != OMT_READ_OK) {
                if (pr == OMT_READ_MISSING) omt_err_set(err, "packet capture is missing");
                result = -1;
            } else {
                static const uint8_t magic[5][4] = {{0xd4, 0xc3, 0xb2, 0xa1},
                                                    {0xa1, 0xb2, 0xc3, 0xd4},
                                                    {0x4d, 0x3c, 0xb2, 0xa1},
                                                    {0xa1, 0xb2, 0x3c, 0x4d},
                                                    {0x0a, 0x0d, 0x0d, 0x0a}};
                bool magic_ok = false;
                omt_buf digest;
                omt_buf_init(&digest, 64);
                if (pcap.len != size || pcap.len < 4) {
                    omt_err_set(err, "packet capture has an unexpected size");
                    result = -1;
                } else {
                    for (int i = 0; i < 5; i++) magic_ok |= memcmp(pcap.data, magic[i], 4) == 0;
                    if (!magic_ok) {
                        omt_err_set(err, "packet capture magic is invalid");
                        result = -1;
                    } else if (!omt_sha256_hex(pcap.data, pcap.len, &digest) ||
                               !field_is(v[9], l[9], omt_buf_cstr(&digest))) {
                        omt_err_set(err, "packet capture SHA-256 does not match metadata");
                        result = -1;
                    } else {
                        omt_buf_append(data, pcap.data, pcap.len);
                        result = 1;
                    }
                }
                omt_buf_free(&digest);
                omt_buf_free(&pcap);
            }
        }
    }
    if (r == OMT_READ_OK) omt_buf_free(&meta);
    return result;
}

static bool valid_json(const char *text, size_t len, bool require_array) {
    omt_json_doc doc;
    omt_json *root = omt_json_parse(&doc, text, len, 0);
    bool ok = root && (!require_array || root->type == OMT_JSON_ARRAY);
    if (root) omt_json_doc_free(&doc);
    return ok;
}

bool omt_diagnostics_bundle(const omt_diagnostics *d, bool include_pcap, const char *version,
                            omt_buf *zip_out, char filename[64], omt_err *err) {
    const omt_web_settings *s = d->settings;
    uint64_t deadline = omt_now_ms() + ms_of(s->diagnostics_bundle_budget_s);
    omt_buf request_id;
    omt_buf_init(&request_id, 64);
    if (!omt_random_hex(&request_id, 16, err)) {
        omt_buf_free(&request_id);
        return false;
    }
    char request[512];
    snprintf(request, sizeof(request),
             "version=1\nrequest_id=%s\ncapture_pcap=%d\nrequested_at_epoch=%llu\n",
             omt_buf_cstr(&request_id), include_pcap ? 1 : 0, (unsigned long long)now_epoch());
    omt_err request_err;
    bool request_ok = omt_write_fixed_inode(s->diagnostics_host_request_file, request,
                                            strlen(request), 512, &request_err);

    omt_diagnostic_result runtime, discovery;
    omt_buf controller;
    omt_buf_init(&controller, 1024 * 1024);
    omt_diagnostics_runtime(d, &runtime, &controller);
    omt_diagnostics_discovery(d, &discovery);
    omt_source_configuration configuration;
    omt_playback_configuration(d->playback, &configuration);
    omt_buf receive;
    omt_buf_init(&receive, 2 * 1024 * 1024);
    if (s->diagnostics_receive_probe && omt_configuration_configured(&configuration)) {
        uint64_t left = omt_remaining_ms(deadline);
        if (left > 5000) left = 5000;
        if (left < 1) left = 1;
        const char *argv[] = {s->receiver_command, "probe", "--target", configuration.source,
                              "--timeout-ms",      "3000",  "--json",   NULL};
        omt_proc_result probe;
        run(argv, left, &probe);
        omt_buf_append(&receive, probe.out.data, probe.out.len);
        omt_proc_result_free(&probe);
    } else {
        json_error(&receive, "skipped: no current target or receive probe disabled");
    }
    omt_buf host;
    omt_buf_init(&host, 16u * 1024 * 1024 + 64);
    if (!request_ok) {
        char detail[400];
        snprintf(detail, sizeof(detail), "unable to submit host diagnostic request: %s",
                 request_err.msg);
        unavailable(&host, detail);
    } else {
        wait_for_host_report(d, omt_buf_cstr(&request_id), deadline, &host);
    }
    omt_buf pcap_meta, pcap;
    omt_buf_init(&pcap_meta, 64 * 1024);
    omt_buf_init(&pcap, PCAP_MAX_BYTES);
    char pcap_error[300] = "";
    int captured = 0;
    bool ok = true;
    if (include_pcap) {
        captured = capture(d, omt_buf_cstr(&request_id), &pcap_meta, &pcap, pcap_error,
                           sizeof(pcap_error), err);
        ok = captured >= 0;
    }

    omt_zip z;
    omt_zip_init(&z, 48u * 1024 * 1024);
    if (ok) {
        omt_buf member;
        omt_buf_init(&member, 16u * 1024 * 1024 + 64);
        omt_buf_printf(&member, "%s\n", version);
        omt_zip_add(&z, "version.txt", member.data, member.len, OMT_ZIP_DEFLATED);
        omt_buf_clear(&member);
        omt_web_settings_diagnostic_lines(s, &member);
        omt_zip_add(&z, "runtime-settings.txt", member.data, member.len, OMT_ZIP_DEFLATED);
        omt_zip_add(&z, "runtime.txt", runtime.command.out.data, runtime.command.out.len,
                    OMT_ZIP_DEFLATED);
        omt_buf_clear(&member);
        if (discovery.command.has_returncode && discovery.command.returncode == 0 &&
            valid_json(omt_buf_cstr(&discovery.command.out), discovery.command.out.len, true)) {
            omt_buf_append(&member, discovery.command.out.data, discovery.command.out.len);
        } else {
            omt_buf scratch;
            omt_buf_init(&scratch, 512 * 1024);
            json_error(&member, omt_proc_failure_detail(&discovery.command, &scratch));
            omt_buf_free(&scratch);
        }
        omt_zip_add(&z, "discovery.json", member.data, member.len, OMT_ZIP_DEFLATED);
        omt_buf_clear(&member);
        omt_buf_printf(&member, "%s\n", omt_buf_cstr(&controller));
        omt_zip_add(&z, "controller-status.txt", member.data, member.len, OMT_ZIP_DEFLATED);
        omt_buf_clear(&member);
        if (valid_json(omt_buf_cstr(&receive), receive.len, false))
            omt_buf_append(&member, receive.data, receive.len);
        else
            json_error(&member, omt_buf_cstr(&receive));
        omt_zip_add(&z, "current-target-receive-probe.json", member.data, member.len,
                    OMT_ZIP_DEFLATED);
        struct {
            const char *name;
            const char *path;
            size_t limit;
        } files[] = {{"playback-status.json", s->playback_status_file, 4096},
                     {"omt-settings.xml", s->runtime_config_file, 65536},
                     {"runtime-sha256.manifest", s->runtime_integrity_manifest, 262144}};
        for (size_t i = 0; i < 3; i++) {
            omt_buf file;
            omt_err ignored;
            if (omt_read_bounded(files[i].path, files[i].limit, &file, &ignored) == OMT_READ_OK) {
                omt_zip_add(&z, files[i].name, file.data, file.len, OMT_ZIP_DEFLATED);
                omt_buf_free(&file);
            } else {
                omt_buf_clear(&member);
                unavailable(&member, "file unavailable");
                omt_zip_add(&z, files[i].name, member.data, member.len, OMT_ZIP_DEFLATED);
            }
        }
        omt_zip_add(&z, "host-report.txt", host.data, host.len, OMT_ZIP_DEFLATED);
        omt_zip_add(&z, "host-network-pcap.txt", pcap_meta.data, pcap_meta.len, OMT_ZIP_DEFLATED);
        if (captured > 0) {
            omt_zip_add(&z, "host-network.pcap", pcap.data, pcap.len, OMT_ZIP_STORED);
        } else if (include_pcap) {
            omt_buf_clear(&member);
            unavailable(&member, pcap_error);
            omt_zip_add(&z, "host-network.pcap.unavailable.txt", member.data, member.len,
                        OMT_ZIP_DEFLATED);
        }
        omt_buf_free(&member);
        ok = omt_zip_finish(&z);
        if (!ok) omt_err_set(err, "unable to assemble the support bundle");
    }
    if (ok) {
        int64_t secs;
        uint32_t nanos;
        omt_wall_clock(&secs, &nanos);
        char stamp[32];
        omt_format_compact_utc(secs, stamp);
        snprintf(filename, 64, "omt-diagnostics-%s.zip", stamp);
        omt_buf_append(zip_out, z.out.data, z.out.len);
        ok = !zip_out->failed;
    }
    omt_zip_free(&z);
    omt_buf_free(&pcap);
    omt_buf_free(&pcap_meta);
    omt_buf_free(&host);
    omt_buf_free(&receive);
    omt_buf_free(&controller);
    omt_diagnostic_result_free(&runtime);
    omt_diagnostic_result_free(&discovery);
    omt_buf_free(&request_id);
    return ok;
}

void omt_diagnostics_request_reboot(const omt_diagnostics *d, omt_action_result *out) {
    memset(out, 0, sizeof(*out));
    omt_buf id;
    omt_buf_init(&id, 64);
    omt_err err;
    if (!omt_random_hex(&id, 16, &err)) {
        omt_strlcpy(out->error, "Unable to generate reboot request ID.", sizeof(out->error));
        omt_buf_free(&id);
        return;
    }
    char record[512];
    snprintf(record, sizeof(record),
             "version=1\naction=reboot\nrequest_id=%s\nrequested_at_epoch=%llu\n",
             omt_buf_cstr(&id), (unsigned long long)now_epoch());
    if (!omt_write_fixed_inode(d->settings->reboot_request_file, record, strlen(record), 512,
                               &err)) {
        snprintf(out->error, sizeof(out->error), "Unable to submit the host reboot request: %s",
                 err.msg);
        omt_buf_free(&id);
        return;
    }
    uint64_t deadline = omt_now_ms() + ms_of(d->settings->reboot_ack_timeout_s);
    while (omt_now_ms() < deadline) {
        omt_buf result;
        if (omt_read_text(d->settings->reboot_result_file, 512, &result, &err) == OMT_READ_OK) {
            static const char *const fields[] = {"version", "request_id", "status", "detail"};
            const char *v[4];
            size_t l[4];
            if (omt_parse_record(omt_buf_cstr(&result), result.len, fields, 4, false, v, l) &&
                field_is(v[0], l[0], "1") && field_is(v[1], l[1], omt_buf_cstr(&id))) {
                if (field_is(v[2], l[2], "accepted")) {
                    out->ok = true;
                    omt_strlcpy(out->message,
                                "OS reboot scheduled. This appliance will go offline shortly.",
                                sizeof(out->message));
                    omt_buf_free(&result);
                    omt_buf_free(&id);
                    return;
                }
                if (field_is(v[2], l[2], "rejected")) {
                    snprintf(out->error, sizeof(out->error),
                             "The host rejected the reboot request: %.*s", (int)l[3], v[3]);
                    omt_buf_free(&result);
                    omt_buf_free(&id);
                    return;
                }
            }
            omt_buf_free(&result);
        }
        sleep_ms(50);
    }
    omt_strlcpy(
        out->error,
        "The reboot request was submitted but the host did not acknowledge it. Check rc-service "
        "omt-client-reboot status and /var/log/messages before retrying.",
        sizeof(out->error));
    omt_buf_free(&id);
}

void omt_app_version(const omt_web_settings *s, omt_buf *out) {
    omt_buf text;
    omt_err err;
    if (omt_read_text(s->version_file, 256, &text, &err) == OMT_READ_OK) {
        omt_span t = omt_utf8_trim(omt_buf_cstr(&text), text.len);
        if (t.len) {
            omt_buf_append(out, t.p, t.len);
            omt_buf_free(&text);
            return;
        }
        omt_buf_free(&text);
    }
    /* The Rust frontend fell back to its crate version, which never carried
     * the leading 'v' the build stamps on. */
    omt_buf_puts(out, omt_version[0] == 'v' ? &omt_version[1] : omt_version);
}

void omt_legal_texts(const omt_web_settings *s, omt_buf *license, omt_buf *notices) {
    omt_buf text;
    omt_err err;
    if (omt_read_text(s->project_license_file, 1024 * 1024, &text, &err) == OMT_READ_OK) {
        omt_buf_append(license, text.data, text.len);
        omt_buf_free(&text);
    } else {
        omt_buf_puts(license, "License text unavailable.");
    }
    if (omt_read_text(s->third_party_notices_file, 8u * 1024 * 1024, &text, &err) == OMT_READ_OK) {
        omt_buf_append(notices, text.data, text.len);
        omt_buf_free(&text);
    } else {
        omt_buf_puts(notices, "Third-party notices unavailable.");
    }
}
