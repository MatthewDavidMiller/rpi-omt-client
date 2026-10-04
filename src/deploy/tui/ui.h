/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Drawing for the terminal deployer. Reads an app; never changes it.
 */
#ifndef DP_TUI_UI_H
#define DP_TUI_UI_H

#include "deploy/tui/app.h"
#include "deploy/tui/screen.h"

/* The header, a usable form, and the status bar do not fit below this. */
#define UI_MIN_WIDTH 40
#define UI_MIN_HEIGHT 11

typedef struct {
    char **items;
    size_t count, cap;
} ui_lines;

void ui_lines_free(ui_lines *l);
void ui_draw(screen *s, const app *a);

/* Exposed for the tests. */
void ui_wrap(const char *text, size_t width, ui_lines *out);
void ui_about_text(size_t width, ui_lines *out);
size_t ui_first_visible_slot(size_t focus, size_t count, size_t visible);
size_t ui_label_width(int width);
/* `text` cut to `width` characters, with `~` when it does not fit. */
void ui_truncate(const char *text, size_t width, omt_buf *out);

#endif
