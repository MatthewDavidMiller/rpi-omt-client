/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * An immediate-mode cell buffer. A frame is drawn into it from scratch, then
 * compared with the previous one, and only the cells that changed are
 * written to the terminal.
 */
#ifndef DP_SCREEN_H
#define DP_SCREEN_H

#include "common/base.h"
#include "common/buf.h"

enum {
    COLOR_DEFAULT = 0,
    COLOR_BLACK,
    COLOR_RED,
    COLOR_GREEN,
    COLOR_YELLOW,
    COLOR_BLUE,
    COLOR_MAGENTA,
    COLOR_CYAN,
    COLOR_WHITE
};

#define ATTR_BOLD 1u

typedef struct {
    uint8_t fg, bg, attr;
} style;

typedef struct {
    uint32_t ch; /* 0 marks the second half of a wide character */
    style st;
} cell;

typedef struct {
    int x, y, w, h;
} rect;

typedef struct {
    int w, h;
    cell *cells;
    cell *prev;
    bool redraw; /* the previous frame is unknown: write everything */
    bool cursor_visible;
    int cursor_x, cursor_y;
} screen;

void screen_init(screen *s);
void screen_free(screen *s);
/* Resizes for a new frame and blanks it. */
bool screen_begin(screen *s, int w, int h);
/* Columns a code point occupies: 0, 1, or 2. */
int screen_char_width(uint32_t cp);
/* Display columns of UTF-8 text. */
int screen_text_width(const char *text, size_t len);
void screen_put(screen *s, int x, int y, uint32_t ch, style st);
/* Draws UTF-8 text on one row, clipped to max_w columns; returns the columns
 * used. Control characters are drawn as U+FFFD. */
int screen_text(screen *s, int x, int y, int max_w, const char *text, size_t len, style st);
void screen_fill(screen *s, rect r, uint32_t ch, style st);
/* A box with an optional title on its top border; returns the inner area. */
rect screen_block(screen *s, rect r, const char *title, style border);
void screen_set_cursor(screen *s, int x, int y);
/* The VT bytes that turn the previous frame into this one. */
void screen_flush(screen *s, omt_buf *out);
/* Row y as UTF-8 text, for the tests. */
void screen_row_text(const screen *s, int y, omt_buf *out);

#endif
