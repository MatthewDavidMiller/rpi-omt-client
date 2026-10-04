/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * A strict SSH session: commands and uploads over one authenticated
 * connection.
 *
 * Unknown or changed host keys are fatal, and known_hosts must exist.
 * Algorithms exclude SHA-1 signatures and MACs, CBC, and compression; strict
 * key exchange is mandatory. Uploads use SFTP when the server offers it and
 * fall back to `cat` on an exec channel for factory headless images.
 */
#ifndef DP_SSH_H
#define DP_SSH_H

#include "common/buf.h"
#include "deploy/core/deploy.h"

typedef struct ssh_session ssh_session;

typedef struct {
    int exit_code;
    omt_buf out; /* lossily decoded UTF-8 */
    omt_buf err;
} ssh_result;

void ssh_result_init(ssh_result *r);
void ssh_result_free(ssh_result *r);
static inline bool ssh_result_success(const ssh_result *r) { return r->exit_code == 0; }
/* stdout, a newline if needed, then stderr. */
void ssh_result_combined(const ssh_result *r, omt_buf *out);

/* known_hosts as used for a connection: the explicit path, or
 * ~/.ssh/known_hosts from HOME or USERPROFILE. */
bool ssh_known_hosts_path(const dp_connection *c, omt_buf *out, dp_err *err);

bool ssh_connect(const dp_connection *c, const dp_cancel *cancel, ssh_session **out, dp_err *err);
void ssh_close(ssh_session *s);

bool ssh_run(ssh_session *s, const char *command, const char *input, size_t input_len,
             const dp_cancel *cancel, ssh_result *result, dp_err *err);
/* Runs on a pseudo-terminal and sends `input` only once `marker` appears in
 * the output. Reserved for tools such as Alpine's `su`, which read a password
 * from a terminal only. */
bool ssh_run_pty_after_marker(ssh_session *s, const char *command, const char *marker,
                              const char *input, size_t input_len, const dp_cancel *cancel,
                              ssh_result *result, dp_err *err);
bool ssh_upload_file(ssh_session *s, const char *local, const char *remote, const dp_cancel *cancel,
                     dp_err *err);
bool ssh_upload_bytes(ssh_session *s, const uint8_t *data, size_t len, const char *remote,
                      const dp_cancel *cancel, dp_err *err);

/* "umask 077 && cat > '<remote>'", the upload fallback's command. */
void ssh_shell_upload_command(omt_buf *out, const char *remote);
/* Whether needle occurs in haystack as a complete byte sequence. */
bool ssh_contains_bytes(const uint8_t *haystack, size_t len, const char *needle);

#endif
