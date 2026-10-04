/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The session the deployment operations hold: connect and authenticate, then
 * commands and uploads on channels of that one connection.
 */
#include <stdlib.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

bool ssh_known_hosts_path(const dp_connection *c, omt_buf *out, dp_err *err) {
    if (c->known_hosts_path) {
        omt_buf_puts(out, c->known_hosts_path);
        return !out->failed;
    }
    omt_buf home;
    omt_buf_init(&home, 4096);
    bool found = (dp_getenv("HOME", &home) && home.len > 0) ||
                 (omt_buf_clear(&home), dp_getenv("USERPROFILE", &home) && home.len > 0);
    if (!found) {
        omt_buf_free(&home);
        dp_fail(err, "home directory is unavailable for strict host-key verification");
        return false;
    }
    omt_buf ssh_dir;
    omt_buf_init(&ssh_dir, 4200);
    dp_path_join(&ssh_dir, omt_buf_cstr(&home), ".ssh");
    dp_path_join(out, omt_buf_cstr(&ssh_dir), "known_hosts");
    omt_buf_free(&home);
    omt_buf_free(&ssh_dir);
    return !out->failed;
}

bool ssh_connect(const dp_connection *c, const dp_cancel *cancel, ssh_session **out, dp_err *err) {
    *out = NULL;
    /* known_hosts has to exist before anything is sent: a missing file is the
     * operator's to fix, and its message should not read as a network one. */
    omt_buf path;
    omt_buf_init(&path, 8192);
    bool ok = ssh_known_hosts_path(c, &path, err);
    if (ok && !dp_is_file(omt_buf_cstr(&path))) {
        dp_fail(err,
                "OpenSSH known_hosts was not found at %s. Connect with ssh first and verify the "
                "Raspberry Pi host key.",
                omt_buf_cstr(&path));
        ok = false;
    }
    omt_buf_free(&path);
    if (!ok) return false;

    ssh_session *s = calloc(1, sizeof(*s));
    if (!s) {
        dp_fail(err, "out of memory");
        return false;
    }
    ssh_transport_init(&s->t);
    if (!ssh_transport_open(&s->t, c, cancel, err)) {
        const char *text = dp_err_text(err);
        if (strcmp(text, "SSH connection timed out") != 0 &&
            strcmp(text, "operation cancelled") != 0 &&
            !omt_has_prefix(text, "OpenSSH known_hosts was not found")) {
            omt_buf detail;
            omt_buf_init(&detail, DP_ERR_LIMIT);
            omt_buf_puts(&detail, text);
            dp_fail(err,
                    "SSH connection or host-key verification failed: %s. Unknown or changed host "
                    "keys are rejected.",
                    omt_buf_cstr(&detail));
            omt_buf_free(&detail);
        }
        ssh_close(s);
        return false;
    }
    if (!ssh_authenticate(&s->t, c, err)) {
        ssh_close(s);
        return false;
    }
    /* Commands are cancelled by their own flag from here on. */
    s->t.cancel = NULL;
    *out = s;
    return true;
}

void ssh_close(ssh_session *s) {
    if (!s) return;
    ssh_disconnect(&s->t, 11, "disconnected by user");
    ssh_transport_free(&s->t);
    free(s);
}

static bool upload(ssh_session *s, ssh_source *src, const char *remote, const dp_cancel *cancel,
                   dp_err *err) {
    if (ssh_sftp_upload(s, src, remote, cancel, err)) return true;
    omt_buf sftp_error;
    omt_buf_init(&sftp_error, DP_ERR_LIMIT);
    omt_buf_puts(&sftp_error, dp_err_text(err));
    bool ok = ssh_source_rewind(src, err) && ssh_shell_upload(s, src, remote, cancel, err);
    if (!ok) {
        omt_buf shell_error;
        omt_buf_init(&shell_error, DP_ERR_LIMIT);
        omt_buf_puts(&shell_error, dp_err_text(err));
        dp_fail(err, "SFTP upload failed (%s); shell fallback failed (%s)",
                omt_buf_cstr(&sftp_error), omt_buf_cstr(&shell_error));
        omt_buf_free(&shell_error);
    }
    omt_buf_free(&sftp_error);
    return ok;
}

bool ssh_upload_file(ssh_session *s, const char *local, const char *remote, const dp_cancel *cancel,
                     dp_err *err) {
    ssh_source src = {NULL, 0, 0, dp_file_open_read(local, err)};
    if (!src.file) return false;
    bool ok = upload(s, &src, remote, cancel, err);
    dp_file_close(src.file);
    return ok;
}

bool ssh_upload_bytes(ssh_session *s, const uint8_t *data, size_t len, const char *remote,
                      const dp_cancel *cancel, dp_err *err) {
    ssh_source src = {data, len, 0, NULL};
    return upload(s, &src, remote, cancel, err);
}
