/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The pieces of the operations the unit tests hold to their contracts: the
 * fixed scripts and commands, privilege escalation, and the parsers of what
 * the Raspberry Pi reports back.
 */
#ifndef DP_OPS_INTERNAL_H
#define DP_OPS_INTERNAL_H

#include "common/buf.h"
#include "deploy/core/deploy.h"
#include "deploy/ssh/ssh.h"

#define DP_SET_HOSTNAME_MEMBER "deploy/host/set-hostname.sh"
#define DP_SET_HOSTNAME_COMPLETE "=== Appliance hostname set ==="
#define DP_SETUP_SYS_MEMBER "deploy/host/setup-sys.sh"
#define DP_SETUP_SYS_COMPLETE "=== Alpine sys install complete ==="

extern const char DP_WEB_PASSWORD_COMMAND[];
extern const char DP_WIFI_SCRIPT[];

typedef struct {
    char uid[32];
    bool has_bash;
    bool has_sudo;
    bool has_doas;
} dp_host_tooling;

dp_host_tooling dp_parse_host_tooling(const char *output);
/* How to become root on a host without sudo; NULL (err set) if there is no
 * way. */
const char *dp_bootstrap_escalation(const dp_host_tooling *tooling, dp_err *err);
bool dp_needs_su_bootstrap(const dp_host_tooling *tooling, const dp_connection *c);

bool dp_require_success(const ssh_result *result, const char *operation, dp_err *err);
const char *dp_sudo_prefix(const dp_connection *c);
void dp_privileged_command(const dp_connection *c, const char *command, omt_buf *out);
void dp_privileged_stdin_command(const dp_connection *c, const char *command, omt_buf *out);
void dp_privileged_argv_command(const dp_connection *c, const char *const *argv, omt_buf *out);
void dp_sudo_input(const dp_connection *c, omt_buf *out);

bool dp_parse_sha256_line(const char *output, char out[65]);
bool dp_is_supported_board(const char *model, size_t len);
bool dp_require_supported_appliance(const char *output, dp_err *err);
bool dp_probed_board(const char *output, char *out, size_t size);
void dp_redact(const char *message, const char *const *secrets, size_t count, omt_buf *out);
bool dp_installer_summary(const char *output, omt_buf *out);
bool dp_first_web_password(const char *logs, omt_buf *out);
/* The rename command: only the uploaded script's path, never the name. */
void dp_ssh_rename_command(const char *remote_q, omt_buf *out);

#endif
