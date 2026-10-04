/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The terminal: raw input decoded into key events, and output written as VT
 * sequences. term_posix.c drives termios; term_win32.c drives the Windows
 * console with virtual-terminal processing, which Windows 10 and later
 * support in both conhost and Windows Terminal.
 */
#ifndef DP_TERM_H
#define DP_TERM_H

#include "common/base.h"
#include "deploy/core/sys.h"

typedef enum {
    KEY_NONE,
    KEY_CHAR,
    KEY_ENTER,
    KEY_TAB,
    KEY_BACKTAB,
    KEY_ESC,
    KEY_BACKSPACE,
    KEY_DELETE,
    KEY_LEFT,
    KEY_RIGHT,
    KEY_UP,
    KEY_DOWN,
    KEY_HOME,
    KEY_END,
    KEY_PAGE_UP,
    KEY_PAGE_DOWN,
    KEY_F,
} key_code;

typedef struct {
    key_code code;
    uint32_t ch; /* KEY_CHAR: the code point, lower case for Ctrl+letter */
    int f;       /* KEY_F: 1-12 */
    bool ctrl;
} key_event;

/* Raw mode, the alternate screen, and a hidden cursor. */
bool term_enter(dp_err *err);
/* Restores the terminal. Safe to call more than once. */
void term_leave(void);
/* The window size in cells. */
bool term_size(int *width, int *height);
/* Waits up to timeout_ms for input; returns how many events were decoded. */
int term_read_keys(key_event *out, int max, uint32_t timeout_ms);
void term_write(const char *data, size_t len);

/* Decodes VT input bytes into events. Exposed so the decoder can be tested
 * and shared: Windows delivers the same sequences with VT input enabled.
 * Returns the bytes consumed; an incomplete escape sequence at the end is
 * left unconsumed unless `final` says no more is coming. */
size_t term_decode(const uint8_t *data, size_t len, bool final, key_event *out, int max,
                   int *count);

#endif
