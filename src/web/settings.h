/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The Web frontend's configuration, read once from the environment. Every
 * value is validated at startup; an invalid one stops the service with a
 * message naming the variable rather than falling back to a default.
 */
#ifndef OMT_WEB_SETTINGS_H
#define OMT_WEB_SETTINGS_H

#include "common/base.h"
#include "common/buf.h"
#include "common/err.h"

#define OMT_PATH_MAX 4096

typedef struct {
    size_t count;
    uint64_t window_ms;
} omt_rate_limit;

typedef struct {
    char config_dir[OMT_PATH_MAX];
    char runtime_dir[OMT_PATH_MAX];
    char password_file[OMT_PATH_MAX];
    uint64_t session_lifetime_s;
    size_t max_request_bytes;
    omt_rate_limit login_limit;
    omt_rate_limit diagnostic_action_limit;
    omt_rate_limit diagnostic_download_limit;
    omt_rate_limit reboot_limit;
    char control_command[OMT_PATH_MAX];
    char receiver_command[OMT_PATH_MAX];
    double control_timeout_s;
    double source_cache_ttl_s;
    char source_target_file[OMT_PATH_MAX];
    char video_ceiling_file[OMT_PATH_MAX];
    char board_label[256];
    char board_video_ceiling[256];
    char playback_status_file[OMT_PATH_MAX];
    char sdk_config_dir[OMT_PATH_MAX];
    char runtime_config_file[OMT_PATH_MAX];
    uint64_t playback_status_stale_s;
    char diagnostics_host_report_file[OMT_PATH_MAX];
    char diagnostics_host_request_file[OMT_PATH_MAX];
    char diagnostics_host_pcap_file[OMT_PATH_MAX];
    char diagnostics_host_pcap_metadata_file[OMT_PATH_MAX];
    double diagnostics_host_timeout_s;
    uint64_t diagnostics_host_budget;
    double diagnostics_bundle_budget_s;
    bool diagnostics_receive_probe;
    char version_file[OMT_PATH_MAX];
    char runtime_integrity_manifest[OMT_PATH_MAX];
    char project_license_file[OMT_PATH_MAX];
    char third_party_notices_file[OMT_PATH_MAX];
    char reboot_request_file[OMT_PATH_MAX];
    char reboot_result_file[OMT_PATH_MAX];
    double reboot_ack_timeout_s;
    uint16_t web_port;
    char tls_cert_file[OMT_PATH_MAX];
    char tls_key_file[OMT_PATH_MAX];
} omt_web_settings;

OMT_NODISCARD bool omt_web_settings_load(omt_web_settings *s, omt_err *err);
/* The settings lines the support bundle records (runtime-settings.txt). */
void omt_web_settings_diagnostic_lines(const omt_web_settings *s, omt_buf *out);
/* Parses "N per unit[s]" as the Rust frontend did. */
OMT_NODISCARD bool omt_rate_limit_parse(const char *raw, omt_rate_limit *out);

#endif
