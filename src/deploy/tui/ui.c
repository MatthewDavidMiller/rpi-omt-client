/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The terminal deployer's views, drawn into a screen buffer each frame.
 */
#include "deploy/tui/ui.h"

#include <stdlib.h>
#include <string.h>

/* Terminals vary in how many colours they honour, so emphasis carries through
 * bold as well as colour: a monochrome console still shows focus. */
static const style FOCUS = {COLOR_BLACK, COLOR_CYAN, ATTR_BOLD};
static const style PLAIN = {COLOR_DEFAULT, COLOR_DEFAULT, 0};
static const style BOLD = {COLOR_DEFAULT, COLOR_DEFAULT, ATTR_BOLD};
static const style WARNING = {COLOR_YELLOW, COLOR_DEFAULT, 0};

/* ------------------------------------------------------------------ lines */

void ui_lines_free(ui_lines *l) {
    for (size_t i = 0; i < l->count; i++) free(l->items[i]);
    free(l->items);
    l->items = NULL;
    l->count = l->cap = 0;
}

static void lines_push(ui_lines *l, const char *text, size_t len) {
    if (l->count == l->cap) {
        size_t cap = l->cap ? l->cap * 2 : 64;
        char **grown = realloc(l->items, cap * sizeof(*grown));
        if (!grown) return;
        l->items = grown;
        l->cap = cap;
    }
    char *copy = dp_strndup(text, len);
    if (copy) l->items[l->count++] = copy;
}

static size_t chars(const char *text, size_t len) { return dp_utf8_count(text, len); }

/* The byte length of the first `n` characters. */
static size_t prefix_bytes(const char *text, size_t len, size_t n) {
    size_t seen = 0;
    for (size_t i = 0; i < len; i++) {
        if (((uint8_t)text[i] & 0xC0) != 0x80) {
            if (seen == n) return i;
            seen++;
        }
    }
    return len;
}

static bool is_space(char c) {
    return c == ' ' || c == '\t' || c == '\r' || c == '\f' || c == '\v';
}

/* ------------------------------------------------------------------ text */

void ui_truncate(const char *text, size_t width, omt_buf *out) {
    size_t len = strlen(text);
    if (chars(text, len) <= width) {
        omt_buf_puts(out, text);
        return;
    }
    size_t keep = width > 0 ? width - 1 : 0;
    omt_buf_append(out, text, prefix_bytes(text, len, keep));
    omt_buf_putc(out, '~');
}

/* Breaks a word too long for the area into pieces that fit: the capsule
 * digest is 64 characters, and broken across rows it can still be read. */
static void push_word_pieces(const char *word, size_t len, size_t width, ui_lines *pieces) {
    if (width == 0) width = 1;
    size_t start = 0;
    while (start < len) {
        size_t take = prefix_bytes(word + start, len - start, width);
        lines_push(pieces, word + start, take);
        start += take;
    }
}

/* Wraps text to `width`, keeping each source line's indentation. A line that
 * already fits is kept exactly, so the key legend's columns survive. */
void ui_wrap(const char *text, size_t width, ui_lines *out) {
    if (width < 1) width = 1;
    size_t total = strlen(text);
    size_t pos = 0;
    while (pos < total) {
        const char *src = text + pos;
        const char *nl = memchr(src, '\n', total - pos);
        size_t len = nl ? (size_t)(nl - src) : total - pos;
        pos += len + (nl ? 1 : 0);
        if (len > 0 && src[len - 1] == '\r') len--;
        if (chars(src, len) <= width) {
            lines_push(out, src, len);
            continue;
        }
        size_t indent = 0;
        while (indent < len && is_space(src[indent])) indent++;
        if (indent >= width) indent = 0;
        size_t margin = indent;
        omt_buf line;
        omt_buf_init(&line, 1u << 20);
        omt_buf_append(&line, src, indent);
        size_t i = 0;
        while (i < len) {
            while (i < len && is_space(src[i])) i++;
            if (i >= len) break;
            size_t w0 = i;
            while (i < len && !is_space(src[i])) i++;
            ui_lines pieces = {0};
            push_word_pieces(src + w0, i - w0, width - margin, &pieces);
            for (size_t k = 0; k < pieces.count; k++) {
                const char *word = pieces.items[k];
                size_t filled = chars((const char *)line.data, line.len);
                bool occupied = filled > margin;
                if (occupied && filled + 1 + chars(word, strlen(word)) > width) {
                    lines_push(out, (const char *)line.data, line.len);
                    omt_buf_clear(&line);
                    omt_buf_append(&line, src, indent);
                } else if (occupied) {
                    omt_buf_putc(&line, ' ');
                }
                omt_buf_puts(&line, word);
            }
            ui_lines_free(&pieces);
        }
        lines_push(out, (const char *)line.data, line.len);
        omt_buf_free(&line);
    }
}

/* The About document: the licence and the third-party notices, which the
 * single-file deployer is the only copy of an operator receives. */
void ui_about_text(size_t width, ui_lines *out) {
    omt_buf head;
    omt_buf_init(&head, 1u << 20);
    omt_buf_printf(&head, "Raspberry Pi OMT client deployer %s\n\n", OMT_VERSION);
    dp_err err;
    dp_err_init(&err);
    /* The digest is the point: a single-file deployer is otherwise opaque
     * about which appliance build it carries. */
    if (!dp_capsule_report(&head, &err)) {
        omt_buf_puts(&head, "This deployer was built without " DP_IMAGE_MEMBER ".");
    }
    dp_err_free(&err);
    omt_buf_puts(&head, "\n\nEverything this deploys is compiled in; no checkout is needed.\n\n"
                        "Keys:\n"
                        "  F1-F8 / Ctrl+Left / Ctrl+Right   switch view\n"
                        "  Tab / Shift+Tab                  move between fields\n"
                        "  Enter                            toggle, or run the focused action\n"
                        "  y / n                            answer a confirmation\n"
                        "  Ctrl+R                           reveal or hide secrets\n"
                        "  Esc                              cancel a running job\n"
                        "  PageUp / PageDown / Up / Down    scroll Activity and About\n"
                        "  Home / End                       jump to the ends of this view\n"
                        "  Ctrl+Q                           quit, asking while a job runs");
    ui_wrap(omt_buf_cstr(&head), width, out);
    omt_buf_free(&head);

    size_t len;
    const char *license = dp_license_text(&len);
    omt_buf trimmed;
    omt_buf_init(&trimmed, 1u << 24);
    ui_wrap("\nLICENSE\n-------", width, out);
    while (len > 0 && (license[len - 1] == '\n' || is_space(license[len - 1]))) len--;
    omt_buf_append(&trimmed, license, len);
    ui_wrap(omt_buf_cstr(&trimmed), width, out);
    ui_wrap("\nTHIRD-PARTY NOTICES\n-------------------", width, out);
    const char *notices = dp_notices_text(&len);
    while (len > 0 && (notices[len - 1] == '\n' || is_space(notices[len - 1]))) len--;
    omt_buf_clear(&trimmed);
    omt_buf_append(&trimmed, notices, len);
    ui_wrap(omt_buf_cstr(&trimmed), width, out);
    omt_buf_free(&trimmed);
}

/* ------------------------------------------------------------------ views */

static void draw_tabs(screen *s, rect area, const app *a) {
    char title[96];
    omt_snprintf(title, sizeof(title), " Raspberry Pi OMT deployer %s ", OMT_VERSION);
    rect inner = screen_block(s, area, title, PLAIN);
    int x = inner.x, end = inner.x + inner.w;
    for (int v = 0; v < VIEW_COUNT && x < end; v++) {
        if (v > 0) {
            screen_put(s, x, inner.y, 0x2502, PLAIN);
            x++;
        }
        char label[64];
        omt_snprintf(label, sizeof(label), "F%d %s", v + 1, view_title((view)v));
        style st = (view)v == a->current ? FOCUS : PLAIN;
        screen_put(s, x, inner.y, ' ', PLAIN);
        x++;
        x += screen_text(s, x, inner.y, end - x, label, strlen(label), st);
        if (x < end) screen_put(s, x, inner.y, ' ', PLAIN);
        x++;
    }
}

size_t ui_first_visible_slot(size_t focus, size_t count, size_t visible) {
    size_t last_page = count > visible ? count - visible : 0;
    size_t first = focus > (visible > 0 ? visible - 1 : 0) ? focus - (visible - 1) : 0;
    return first < last_page ? first : last_page;
}

size_t ui_label_width(int width) {
    size_t half = (size_t)(width > 0 ? width : 0) / 2;
    return half < 12 ? 12 : half > 32 ? 32 : half;
}

static size_t value_scroll(size_t cursor, size_t columns) {
    size_t keep = columns > 0 ? columns - 1 : 0;
    return cursor > keep ? cursor - keep : 0;
}

/* One row per slot, only the rows that fit, the window following the focus.
 * Rows laid out unconditionally used to vanish while staying focusable, so a
 * short terminal offered an invisible button that erases the Pi's disk. */
static void draw_form(screen *s, rect area, const app *a) {
    size_t count;
    const slot *slots = view_slots(a->current, &count);
    rect inner = {area.x + 1, area.y + 1, area.w - 2, area.h - 2};
    size_t height = inner.h > 0 ? (size_t)inner.h : 0;
    size_t visible = height < 1 ? 1 : height;
    if (visible > count) visible = count;
    size_t first = ui_first_visible_slot(a->focus, count, visible);
    char title[128];
    if (visible < count) {
        omt_snprintf(title, sizeof(title), " %s (fields %zu-%zu of %zu) ", view_title(a->current),
                     first + 1, first + visible, count);
    } else {
        omt_snprintf(title, sizeof(title), " %s ", view_title(a->current));
    }
    screen_block(s, area, title, PLAIN);
    size_t gutter = ui_label_width(inner.w);
    size_t columns = (size_t)inner.w > gutter ? (size_t)inner.w - gutter : 0;
    omt_buf label, value;
    omt_buf_init(&label, 4096);
    omt_buf_init(&value, 64u * 1024u);
    for (size_t offset = 0; offset < visible && (int)offset < inner.h; offset++) {
        size_t index = first + offset;
        slot sl = slots[index];
        bool focused = index == a->focus;
        size_t scroll = focused ? value_scroll(a->cursor, columns) : 0;
        int y = inner.y + (int)offset;
        omt_buf_clear(&label);
        omt_buf_puts(&label, focused ? "> " : "  ");
        ui_truncate(slot_label(sl), gutter - 2, &label);
        size_t pad = gutter - 2 - chars(slot_label(sl), strlen(slot_label(sl)));
        if (chars(slot_label(sl), strlen(slot_label(sl))) > gutter - 2) pad = 0;
        for (size_t i = 0; i < pad; i++) omt_buf_putc(&label, ' ');
        int used = screen_text(s, inner.x, y, inner.w, (const char *)label.data, label.len,
                               focused ? FOCUS : PLAIN);
        omt_buf_clear(&value);
        const char *v = app_value(a, sl);
        switch (slot_kind(sl)) {
        case KIND_TEXT: omt_buf_puts(&value, v); break;
        case KIND_SECRET:
            if (a->reveal) {
                omt_buf_puts(&value, v);
            } else {
                for (size_t i = chars(v, strlen(v)); i > 0; i--) omt_buf_putc(&value, '*');
            }
            break;
        case KIND_TOGGLE: omt_buf_puts(&value, app_toggle(a, sl) ? "[x]" : "[ ]"); break;
        case KIND_BUTTON:
            omt_buf_puts(&value, app_busy(a) ? "[ running... ]" : "[ press Enter ]");
            break;
        }
        size_t skip = prefix_bytes((const char *)value.data, value.len, scroll);
        screen_text(s, inner.x + used, y, inner.w - used, (const char *)value.data + skip,
                    value.len - skip, PLAIN);
        kind k = slot_kind(sl);
        if (focused && (k == KIND_TEXT || k == KIND_SECRET)) {
            /* The operator's own terminal shows the insertion point. */
            screen_set_cursor(s, inner.x + (int)(gutter + a->cursor - scroll), y);
        }
    }
    omt_buf_free(&label);
    omt_buf_free(&value);
}

static void draw_activity(screen *s, rect area, const app *a) {
    char title[96];
    if (a->follow_log) {
        omt_snprintf(title, sizeof(title), " Activity (following) ");
    } else {
        omt_snprintf(title, sizeof(title), " Activity (scrolled, %zu lines) ", a->log_len);
    }
    rect inner = screen_block(s, area, title, PLAIN);
    size_t height = inner.h > 0 ? (size_t)inner.h : 0;
    /* Following pins the view to the tail; otherwise the operator's scroll
     * position is authoritative even as new lines arrive. */
    size_t start;
    if (a->follow_log) {
        start = a->log_len > height ? a->log_len - height : 0;
    } else {
        size_t last = a->log_len > 0 ? a->log_len - 1 : 0;
        start = a->log_scroll < last ? a->log_scroll : last;
    }
    for (size_t i = 0; i < height && start + i < a->log_len; i++) {
        const char *line = a->log[start + i];
        screen_text(s, inner.x, inner.y + (int)i, inner.w, line, strlen(line), PLAIN);
    }
}

static void draw_about(screen *s, rect area, const app *a) {
    rect inner = {area.x + 1, area.y + 1, area.w - 2, area.h - 2};
    ui_lines lines = {0};
    ui_about_text(inner.w > 0 ? (size_t)inner.w : 1, &lines);
    size_t height = inner.h > 0 ? (size_t)inner.h : 0;
    /* The far end is clamped here, where the wrapped length is known. */
    size_t max_offset = lines.count > height ? lines.count - height : 0;
    size_t offset = a->about_scroll < max_offset ? a->about_scroll : max_offset;
    size_t last = offset + height < lines.count ? offset + height : lines.count;
    char title[96];
    if (lines.count > height) {
        omt_snprintf(title, sizeof(title), " About (lines %zu-%zu of %zu) ", offset + 1, last,
                     lines.count);
    } else {
        omt_snprintf(title, sizeof(title), " About ");
    }
    screen_block(s, area, title, PLAIN);
    for (size_t i = offset; i < last; i++) {
        screen_text(s, inner.x, inner.y + (int)(i - offset), inner.w, lines.items[i],
                    strlen(lines.items[i]), PLAIN);
    }
    ui_lines_free(&lines);
}

static void draw_status(screen *s, rect area, const app *a) {
    rect inner = screen_block(s, area, NULL, PLAIN);
    omt_buf text;
    omt_buf_init(&text, DP_ERR_LIMIT);
    omt_buf_printf(&text, " %s ", omt_buf_cstr(&a->status));
    /* A multi-line status shows its first line here; the log has the rest. */
    char *nl = memchr(text.data, '\n', text.len);
    if (nl) {
        text.len = (size_t)(nl - (char *)text.data);
        omt_buf_puts(&text, " ");
    }
    int used = screen_text(s, inner.x, inner.y, inner.w, (const char *)text.data, text.len, BOLD);
    omt_buf_clear(&text);
    omt_buf_printf(&text, "| secrets %s | %s", a->reveal ? "shown" : "hidden",
                   app_busy(a) ? "Esc cancel" : "Enter act  Ctrl+R secrets  Ctrl+Q quit");
    screen_text(s, inner.x + used, inner.y, inner.w - used, (const char *)text.data, text.len,
                PLAIN);
    omt_buf_free(&text);
}

/* A centred prompt over the view, for the actions that interrupt a running
 * appliance. */
static void draw_confirm(screen *s, const char *prompt) {
    int width = (int)chars(prompt, strlen(prompt)) + 8;
    if (width > s->w) width = s->w;
    int height = 5 < s->h ? 5 : s->h;
    rect region = {(s->w - width) / 2, (s->h - height) / 2, width, height};
    screen_fill(s, region, ' ', PLAIN);
    rect inner = screen_block(s, region, " Confirm ", WARNING);
    if (inner.h > 0) screen_text(s, inner.x, inner.y, inner.w, prompt, strlen(prompt), PLAIN);
    if (inner.h > 1) screen_text(s, inner.x, inner.y + 1, inner.w, "y = yes, n = no", 15, PLAIN);
}

void ui_draw(screen *s, const app *a) {
    if (s->w < UI_MIN_WIDTH || s->h < UI_MIN_HEIGHT) {
        char text[128];
        omt_snprintf(text, sizeof(text), "Terminal is %dx%d; this needs at least %dx%d.", s->w,
                     s->h, UI_MIN_WIDTH, UI_MIN_HEIGHT);
        ui_lines lines = {0};
        ui_wrap(text, (size_t)s->w, &lines);
        for (size_t i = 0; i < lines.count && (int)i < s->h; i++) {
            screen_text(s, 0, (int)i, s->w, lines.items[i], strlen(lines.items[i]), PLAIN);
        }
        ui_lines_free(&lines);
        return;
    }
    rect tabs = {0, 0, s->w, 3};
    rect body = {0, 3, s->w, s->h - 6};
    rect status = {0, s->h - 3, s->w, 3};
    draw_tabs(s, tabs, a);
    if (a->current == VIEW_ACTIVITY) {
        draw_activity(s, body, a);
    } else if (a->current == VIEW_ABOUT) {
        draw_about(s, body, a);
    } else {
        draw_form(s, body, a);
    }
    draw_status(s, status, a);
    if (a->confirm.active) {
        s->cursor_visible = false;
        draw_confirm(s, a->confirm.prompt);
    }
}
