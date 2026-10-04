/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * rpi-omt-deploy-tui: the deployment, driven from a terminal.
 *
 * A terminal frontend is what lets this ship as one static binary on Linux
 * and one console executable on Windows: it opens no graphics stack, and it
 * also works over SSH, which matters for an appliance that usually lives in a
 * rack.
 */
#include <stdio.h>
#include <string.h>

#include "common/version.h"
#include "deploy/tui/app.h"
#include "deploy/tui/term.h"
#include "deploy/tui/ui.h"

/* How long a redraw waits for a keystroke before looping. The worker delivers
 * progress on a queue rather than waking the loop, so this is also the
 * longest an operator waits to see a new line during a deployment. */
#define TICK_MS 100u

int main(int argc, char **argv) {
    if (argc > 1 && (strcmp(argv[1], "--version") == 0 || strcmp(argv[1], "-V") == 0)) {
        printf("rpi-omt-deploy-tui %s\n", omt_version);
        return 0;
    }
    if (argc > 1) {
        fprintf(stderr, "usage: rpi-omt-deploy-tui [--version]\n");
        return 2;
    }
    dp_err err;
    dp_err_init(&err);
    if (!dp_sys_init(&err) || !term_enter(&err)) {
        fprintf(stderr, "%s\n", dp_err_text(&err));
        dp_err_free(&err);
        return 1;
    }
    dp_err_free(&err);
    app a;
    app_init(&a);
    screen s;
    screen_init(&s);
    omt_buf frame;
    omt_buf_init(&frame, 64u * 1024u * 1024u);
    while (!a.should_quit) {
        app_poll_worker(&a);
        int w = 80, h = 24;
        term_size(&w, &h);
        if (screen_begin(&s, w, h)) {
            ui_draw(&s, &a);
            omt_buf_clear(&frame);
            screen_flush(&s, &frame);
            term_write((const char *)frame.data, frame.len);
        }
        key_event keys[64];
        int n = term_read_keys(keys, 64, TICK_MS);
        for (int i = 0; i < n && !a.should_quit; i++) app_handle_key(&a, &keys[i]);
    }
    /* Restore the terminal before anything else: a console left in raw mode
     * on the alternate screen is worse than whatever went wrong. */
    term_leave();
    omt_buf_free(&frame);
    screen_free(&s);
    /* A job still running belongs to the worker now; leaving lets the
     * process end it. */
    app_free(&a);
    return 0;
}
