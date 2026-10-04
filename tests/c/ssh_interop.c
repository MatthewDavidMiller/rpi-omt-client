/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Drives the SSH client against a real server for
 * tests/integration/test_ssh_client.sh. Not a unit suite: it needs a server.
 *
 *   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- run COMMAND [STDIN]
 *   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- pty COMMAND MARKER STDIN
 *   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- upload LOCAL REMOTE
 *   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- upload-bytes SIZE REMOTE
 *   ssh_interop HOST PORT USER KNOWN_HOSTS KEY|- shell-upload LOCAL REMOTE
 *
 * The password (or key passphrase) comes from OMT_TEST_SECRET. A run prints
 * the remote stdout, then "exit=N" on stderr.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

static int fail(const dp_err *err) {
    fprintf(stderr, "ERROR: %s\n", dp_err_text(err));
    return 1;
}

int main(int argc, char **argv) {
    if (argc < 8) {
        fprintf(stderr, "usage: see the header of tests/c/ssh_interop.c\n");
        return 2;
    }
    dp_err err;
    dp_err_init(&err);
    if (!dp_sys_init(&err)) return fail(&err);
    dp_connection c;
    dp_connection_init(&c);
    c.host = dp_strdup(argv[1]);
    c.port = (uint16_t)strtoul(argv[2], NULL, 10);
    c.username = dp_strdup(argv[3]);
    c.known_hosts_path = dp_strdup(argv[4]);
    const char *secret = getenv("OMT_TEST_SECRET");
    if (strcmp(argv[5], "-") == 0) {
        c.auth = DP_AUTH_PASSWORD;
        (void)dp_secret_assign_cstr(&c.password, secret ? secret : "");
    } else {
        c.auth = DP_AUTH_KEY;
        c.key_path = dp_strdup(argv[5]);
        if (secret) (void)dp_secret_assign_cstr(&c.key_passphrase, secret);
    }
    dp_cancel cancel = false;
    ssh_session *s = NULL;
    const char *mode = argv[6];
    int status = 0;
    if (!ssh_connect(&c, &cancel, &s, &err)) {
        status = fail(&err);
    } else if (strcmp(mode, "run") == 0 || strcmp(mode, "pty") == 0) {
        ssh_result r;
        ssh_result_init(&r);
        bool pty = strcmp(mode, "pty") == 0;
        const char *input = pty ? (argc > 9 ? argv[9] : "") : (argc > 8 ? argv[8] : "");
        bool ok = pty ? ssh_run_pty_after_marker(s, argv[7], argc > 8 ? argv[8] : "", input,
                                                 strlen(input), &cancel, &r, &err)
                      : ssh_run(s, argv[7], input, strlen(input), &cancel, &r, &err);
        if (!ok) {
            status = fail(&err);
        } else {
            fwrite(r.out.data, 1, r.out.len, stdout);
            if (r.err.len) fprintf(stderr, "stderr=%s", omt_buf_cstr(&r.err));
            fprintf(stderr, "exit=%d\n", r.exit_code);
        }
        ssh_result_free(&r);
    } else if (strcmp(mode, "upload") == 0 && argc > 8) {
        if (!ssh_upload_file(s, argv[7], argv[8], &cancel, &err)) status = fail(&err);
    } else if (strcmp(mode, "shell-upload") == 0 && argc > 8) {
        ssh_source src = {NULL, 0, 0, dp_file_open_read(argv[7], &err)};
        if (!src.file || !ssh_shell_upload(s, &src, argv[8], &cancel, &err)) status = fail(&err);
        dp_file_close(src.file);
    } else if (strcmp(mode, "upload-bytes") == 0 && argc > 8) {
        size_t size = (size_t)strtoull(argv[7], NULL, 10);
        uint8_t *data = malloc(size ? size : 1);
        for (size_t i = 0; data && i < size; i++) data[i] = (uint8_t)(i * 131u + (i >> 9));
        if (!data || !ssh_upload_bytes(s, data, size, argv[8], &cancel, &err)) status = fail(&err);
        free(data);
    } else {
        fprintf(stderr, "unknown mode %s\n", mode);
        status = 2;
    }
    ssh_close(s);
    dp_connection_free(&c);
    dp_err_free(&err);
    return status;
}
