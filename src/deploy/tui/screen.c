/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "deploy/tui/screen.h"

#include <stdlib.h>
#include <string.h>

#include "common/json.h"

void screen_init(screen *s) { memset(s, 0, sizeof(*s)); }

void screen_free(screen *s) {
    free(s->cells);
    free(s->prev);
    memset(s, 0, sizeof(*s));
}

static const style PLAIN = {COLOR_DEFAULT, COLOR_DEFAULT, 0};

bool screen_begin(screen *s, int w, int h) {
    if (w < 1) w = 1;
    if (h < 1) h = 1;
    if (w > 1000) w = 1000;
    if (h > 1000) h = 1000;
    if (w != s->w || h != s->h || !s->cells) {
        free(s->cells);
        free(s->prev);
        size_t n = (size_t)w * (size_t)h;
        s->cells = calloc(n, sizeof(cell));
        s->prev = calloc(n, sizeof(cell));
        if (!s->cells || !s->prev) {
            free(s->cells);
            free(s->prev);
            s->cells = s->prev = NULL;
            s->w = s->h = 0;
            return false;
        }
        s->w = w;
        s->h = h;
        s->redraw = true;
    }
    for (int i = 0; i < w * h; i++) {
        s->cells[i].ch = ' ';
        s->cells[i].st = PLAIN;
    }
    s->cursor_visible = false;
    return true;
}

int screen_char_width(uint32_t cp) {
    if (cp == 0) return 0;
    /* Combining marks and zero-width format characters. */
    if ((cp >= 0x0300 && cp <= 0x036F) || (cp >= 0x200B && cp <= 0x200F) ||
        (cp >= 0xFE00 && cp <= 0xFE0F)) {
        return 0;
    }
    /* East Asian wide and fullwidth ranges, and emoji. */
    if ((cp >= 0x1100 && cp <= 0x115F) || (cp >= 0x2E80 && cp <= 0x303E) ||
        (cp >= 0x3041 && cp <= 0x33FF) || (cp >= 0x3400 && cp <= 0x4DBF) ||
        (cp >= 0x4E00 && cp <= 0x9FFF) || (cp >= 0xA000 && cp <= 0xA4CF) ||
        (cp >= 0xAC00 && cp <= 0xD7A3) || (cp >= 0xF900 && cp <= 0xFAFF) ||
        (cp >= 0xFE30 && cp <= 0xFE4F) || (cp >= 0xFF00 && cp <= 0xFF60) ||
        (cp >= 0xFFE0 && cp <= 0xFFE6) || (cp >= 0x1F300 && cp <= 0x1F64F) ||
        (cp >= 0x1F900 && cp <= 0x1F9FF) || (cp >= 0x20000 && cp <= 0x3FFFD)) {
        return 2;
    }
    return 1;
}

/* One code point, or U+FFFD for a byte that does not start a valid sequence
 * (which then advances by one byte). */
static uint32_t next_cp(const char *text, size_t len, size_t *i) {
    const uint8_t *p = (const uint8_t *)text + *i;
    size_t left = len - *i;
    uint8_t lead = p[0];
    size_t need = lead < 0x80                    ? 1
                  : lead >= 0xF0 && lead <= 0xF4 ? 4
                  : lead >= 0xE0                 ? 3
                  : lead >= 0xC2 && lead < 0xE0  ? 2
                                                 : 0;
    if (need == 0 || need > left || !omt_utf8_valid((const char *)p, need)) {
        (*i)++;
        return 0xFFFD;
    }
    uint32_t cp = need == 1 ? lead : lead & (need == 2 ? 0x1Fu : need == 3 ? 0x0Fu : 0x07u);
    for (size_t k = 1; k < need; k++) cp = (cp << 6) | (p[k] & 0x3Fu);
    *i += need;
    return cp;
}

int screen_text_width(const char *text, size_t len) {
    int w = 0;
    size_t i = 0;
    while (i < len) {
        uint32_t cp = next_cp(text, len, &i);
        w += cp < 0x20 || (cp >= 0x7F && cp < 0xA0) ? 1 : screen_char_width(cp);
    }
    return w;
}

void screen_put(screen *s, int x, int y, uint32_t ch, style st) {
    if (x < 0 || y < 0 || x >= s->w || y >= s->h) return;
    cell *c = &s->cells[y * s->w + x];
    c->ch = ch;
    c->st = st;
}

int screen_text(screen *s, int x, int y, int max_w, const char *text, size_t len, style st) {
    int used = 0;
    size_t i = 0;
    while (i < len && used < max_w) {
        uint32_t cp = next_cp(text, len, &i);
        if (cp < 0x20 || (cp >= 0x7F && cp < 0xA0)) cp = 0xFFFD;
        int w = screen_char_width(cp);
        if (w == 0) continue;
        if (used + w > max_w) break;
        screen_put(s, x + used, y, cp, st);
        if (w == 2) screen_put(s, x + used + 1, y, 0, st);
        used += w;
    }
    return used;
}

void screen_fill(screen *s, rect r, uint32_t ch, style st) {
    for (int y = r.y; y < r.y + r.h; y++) {
        for (int x = r.x; x < r.x + r.w; x++) screen_put(s, x, y, ch, st);
    }
}

rect screen_block(screen *s, rect r, const char *title, style border) {
    rect inner = {r.x + 1, r.y + 1, r.w - 2, r.h - 2};
    if (inner.w < 0) inner.w = 0;
    if (inner.h < 0) inner.h = 0;
    if (r.w < 1 || r.h < 1) return inner;
    for (int x = r.x + 1; x < r.x + r.w - 1; x++) {
        screen_put(s, x, r.y, 0x2500, border);
        screen_put(s, x, r.y + r.h - 1, 0x2500, border);
    }
    for (int y = r.y + 1; y < r.y + r.h - 1; y++) {
        screen_put(s, r.x, y, 0x2502, border);
        screen_put(s, r.x + r.w - 1, y, 0x2502, border);
    }
    screen_put(s, r.x, r.y, 0x250C, border);
    screen_put(s, r.x + r.w - 1, r.y, 0x2510, border);
    screen_put(s, r.x, r.y + r.h - 1, 0x2514, border);
    screen_put(s, r.x + r.w - 1, r.y + r.h - 1, 0x2518, border);
    if (title && r.w > 2) screen_text(s, r.x + 1, r.y, r.w - 2, title, strlen(title), border);
    return inner;
}

void screen_set_cursor(screen *s, int x, int y) {
    s->cursor_visible = x >= 0 && y >= 0 && x < s->w && y < s->h;
    s->cursor_x = x;
    s->cursor_y = y;
}

static void sgr(omt_buf *out, style st) {
    omt_buf_puts(out, "\x1b[0");
    if (st.attr & ATTR_BOLD) omt_buf_puts(out, ";1");
    if (st.fg) omt_buf_printf(out, ";%d", 29 + st.fg);
    if (st.bg) omt_buf_printf(out, ";%d", 39 + st.bg);
    omt_buf_putc(out, 'm');
}

static bool same(const cell *a, const cell *b) {
    return a->ch == b->ch && a->st.fg == b->st.fg && a->st.bg == b->st.bg &&
           a->st.attr == b->st.attr;
}

void screen_flush(screen *s, omt_buf *out) {
    omt_buf_puts(out, "\x1b[?25l");
    if (s->redraw) omt_buf_puts(out, "\x1b[0m\x1b[2J");
    bool have_style = false;
    style current = PLAIN;
    int cx = -1, cy = -1;
    for (int y = 0; y < s->h; y++) {
        for (int x = 0; x < s->w; x++) {
            const cell *c = &s->cells[y * s->w + x];
            if (!s->redraw && same(c, &s->prev[y * s->w + x])) continue;
            if (c->ch == 0) continue; /* covered by the wide character before it */
            if (cx != x || cy != y) omt_buf_printf(out, "\x1b[%d;%dH", y + 1, x + 1);
            if (!have_style || memcmp(&current, &c->st, sizeof(style)) != 0) {
                sgr(out, c->st);
                current = c->st;
                have_style = true;
            }
            omt_utf8_put(out, c->ch);
            cx = x + screen_char_width(c->ch);
            cy = y;
        }
    }
    omt_buf_puts(out, "\x1b[0m");
    if (s->cursor_visible) {
        omt_buf_printf(out, "\x1b[%d;%dH\x1b[?25h", s->cursor_y + 1, s->cursor_x + 1);
    }
    memcpy(s->prev, s->cells, (size_t)s->w * (size_t)s->h * sizeof(cell));
    s->redraw = false;
}

void screen_row_text(const screen *s, int y, omt_buf *out) {
    if (y < 0 || y >= s->h) return;
    for (int x = 0; x < s->w; x++) {
        uint32_t ch = s->cells[y * s->w + x].ch;
        if (ch) omt_utf8_put(out, ch);
    }
}
