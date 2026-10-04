/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Bounded diagnostics: the discovery, runtime, and direct-connect checks, the
 * support bundle (docs/DIAGNOSTICS_BUNDLE.md), and the host reboot request.
 */
#ifndef OMT_WEB_DIAGNOSTICS_H
#define OMT_WEB_DIAGNOSTICS_H

#include "common/proc.h"
#include "web/playback.h"
#include "web/settings.h"

/* Host capture and the in-memory ZIP share this ceiling so a support download
 * cannot push the appliance over its container memory cap. */
#define PCAP_MAX_BYTES (8 * 1024 * 1024)

typedef struct {
    char title[64];
    omt_proc_result command;
    bool skipped;
    char sources[OMT_MAX_CHOICES][OMT_SOURCE_NAME_MAX_BYTES + 1];
    size_t source_count;
} omt_diagnostic_result;

void omt_diagnostic_result_free(omt_diagnostic_result *r);

typedef struct {
    const omt_web_settings *settings;
    omt_playback *playback;
} omt_diagnostics;

/* control-omt.sh status, as report text. */
void omt_diagnostics_status(const omt_diagnostics *d, omt_buf *out);
void omt_diagnostics_discovery(const omt_diagnostics *d, omt_diagnostic_result *out);
/* The runtime check; `status_text` receives the controller status. */
void omt_diagnostics_runtime(const omt_diagnostics *d, omt_diagnostic_result *out,
                             omt_buf *status_text);
void omt_diagnostics_direct(const omt_diagnostics *d, const char *address,
                            omt_diagnostic_result *out);
/* Builds the support bundle into `zip` and names it in `filename`. */
OMT_NODISCARD bool omt_diagnostics_bundle(const omt_diagnostics *d, bool include_pcap,
                                          const char *version, omt_buf *zip, char filename[64],
                                          omt_err *err);
void omt_diagnostics_request_reboot(const omt_diagnostics *d, omt_action_result *out);

/* The appliance version (from the version file, else the build's own). */
void omt_app_version(const omt_web_settings *s, omt_buf *out);
void omt_legal_texts(const omt_web_settings *s, omt_buf *license, omt_buf *notices);

/* Parses a key=value record: every key exactly once, exactly `required`, and
 * (with allow_body) stopping at the first empty line. Exposed for tests. */
bool omt_parse_record(const char *text, size_t len, const char *const *required, size_t count,
                      bool allow_body, const char **values, size_t *value_lens);

#endif
