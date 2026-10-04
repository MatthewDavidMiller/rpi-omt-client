/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * omt-web: the appliance's HTTPS operator interface, and the small helper
 * commands the container scripts call (initialize, set-password,
 * play-target, video-ceiling, playout-delay).
 */
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "common/json.h"
#include "common/version.h"
#include "web/app.h"
#include "web/state.h"

static int fail(const char *message) {
    fprintf(stderr, "omt-web: %s\n", message);
    return 1;
}

static volatile int stop_requested;

static void on_signal(int signal_number) {
    (void)signal_number;
    stop_requested = 1;
}

static int set_password(const omt_web_settings *s) {
    /* At most the longest valid password, its newline, and one byte more to
     * tell an over-long one apart. */
    char input[OMT_MAXIMUM_PASSWORD_BYTES + 3];
    size_t len = 0;
    while (len < sizeof(input) - 1) {
        ssize_t n = read(STDIN_FILENO, input + len, sizeof(input) - 1 - len);
        if (n < 0) {
            omt_err e;
            omt_err_os(&e, errno);
            char message[300];
            snprintf(message, sizeof(message), "unable to read the new password: %s", e.msg);
            return fail(message);
        }
        if (n == 0) break;
        len += (size_t)n;
    }
    input[len] = 0;
    if (!omt_utf8_valid(input, len))
        return fail("unable to read the new password: stream did not contain valid UTF-8");
    if (len && input[len - 1] == '\n') input[--len] = 0;
    omt_err err;
    bool ok = omt_replace_password(s, input, len, &err);
    explicit_bzero(input, sizeof(input));
    if (!ok) return fail(err.msg);
    printf("Web GUI password updated.\n");
    return 0;
}

int main(int argc, char **argv) {
    signal(SIGPIPE, SIG_IGN);
    static omt_web_settings settings;
    omt_err err;
    if (!omt_web_settings_load(&settings, &err)) return fail(err.msg);
    if (argc == 2 && (strcmp(argv[1], "--version") == 0 || strcmp(argv[1], "-V") == 0)) {
        printf("%s\n", omt_version);
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "initialize") == 0) {
        omt_buf password;
        omt_buf_init(&password, 64);
        if (!omt_auth_initialize(&settings, &password, &err)) return fail(err.msg);
        if (password.len) printf("%s\n", omt_buf_cstr(&password));
        omt_buf_free_secret(&password);
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "set-password") == 0) return set_password(&settings);
    if (argc == 3 && strcmp(argv[1], "play-target") == 0) {
        omt_source_target target;
        int r = omt_read_source(argv[2], &target, &err);
        if (r < 0) return fail(err.msg);
        if (r == 0) return fail("saved OMT target is missing");
        printf("%s\n", target.value);
        return 0;
    }
    if (argc == 4 && strcmp(argv[1], "video-ceiling") == 0) {
        omt_buf value;
        omt_buf_init(&value, 256);
        if (!omt_effective_video_ceiling(argv[2], argv[3], &value, &err)) return fail(err.msg);
        printf("%s\n", omt_buf_cstr(&value));
        omt_buf_free(&value);
        return 0;
    }
    if (argc == 4 && strcmp(argv[1], "playout-delay") == 0) {
        uint64_t ms;
        if (!omt_effective_playout_delay(argv[2], argv[3], &ms, &err)) return fail(err.msg);
        printf("%llu\n", (unsigned long long)ms);
        return 0;
    }
    if (argc != 1)
        return fail(
            "usage: omt-web [initialize | set-password | play-target PATH | video-ceiling PATH "
            "BOARD_DEFAULT | playout-delay PATH DEFAULT]");

    static omt_app app;
    if (!omt_app_init(&app, &settings, &err)) return fail(err.msg);
    omt_remove_legacy_secret(&settings);
    omt_http_server *server;
    if (!omt_http_server_open(&server, settings.web_port, settings.tls_cert_file,
                              settings.tls_key_file, settings.max_request_bytes, omt_app_handle,
                              &app, &err)) {
        omt_app_free(&app);
        return fail(err.msg);
    }
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = on_signal;
    sigemptyset(&sa.sa_mask);
    sigaction(SIGINT, &sa, NULL);
    sigaction(SIGTERM, &sa, NULL);
    printf("omt-web listening on https://0.0.0.0:%u\n", (unsigned)settings.web_port);
    fflush(stdout);
    bool ok = omt_http_server_run(server, &stop_requested, &err);
    omt_http_server_close(server);
    omt_app_free(&app);
    if (!ok) {
        char message[300];
        snprintf(message, sizeof(message), "web server failed: %s", err.msg);
        return fail(message);
    }
    return 0;
}
