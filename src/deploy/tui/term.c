/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * VT input decoding, shared by both terminal backends.
 */
#include "deploy/tui/term.h"

static bool push(key_event *out, int max, int *count, key_code code, uint32_t ch, int f,
                 bool ctrl) {
    if (*count >= max) return false;
    out[*count].code = code;
    out[*count].ch = ch;
    out[*count].f = f;
    out[*count].ctrl = ctrl;
    (*count)++;
    return true;
}

/* A CSI or SS3 sequence starting at data[0] == ESC. Returns its length, 0 if
 * incomplete, or -1 if it is not a sequence this decoder knows. */
static long escape(const uint8_t *data, size_t len, key_event *ev) {
    if (len < 2) return 0;
    ev->ch = 0;
    ev->f = 0;
    ev->ctrl = false;
    if (data[1] == 'O') {
        if (len < 3) return 0;
        switch (data[2]) {
        case 'P':
            ev->code = KEY_F;
            ev->f = 1;
            return 3;
        case 'Q':
            ev->code = KEY_F;
            ev->f = 2;
            return 3;
        case 'R':
            ev->code = KEY_F;
            ev->f = 3;
            return 3;
        case 'S':
            ev->code = KEY_F;
            ev->f = 4;
            return 3;
        case 'H': ev->code = KEY_HOME; return 3;
        case 'F': ev->code = KEY_END; return 3;
        case 'A': ev->code = KEY_UP; return 3;
        case 'B': ev->code = KEY_DOWN; return 3;
        case 'C': ev->code = KEY_RIGHT; return 3;
        case 'D': ev->code = KEY_LEFT; return 3;
        default: return 3;
        }
    }
    if (data[1] != '[') return -1;
    unsigned params[4] = {0, 0, 0, 0};
    int n = 0;
    size_t i = 2;
    for (; i < len && i < 32; i++) {
        uint8_t b = data[i];
        if (b >= '0' && b <= '9') {
            if (params[n] < 1000) params[n] = params[n] * 10 + (unsigned)(b - '0');
        } else if (b == ';') {
            if (n < 3) n++;
        } else if (b >= 0x40 && b <= 0x7e) {
            break;
        } else {
            return (long)i + 1; /* something odd: swallow it */
        }
    }
    if (i >= len) return len >= 32 ? (long)len : 0;
    uint8_t final_byte = data[i];
    /* xterm modifiers: 1 + (shift 1, alt 2, ctrl 4). */
    unsigned mod = n >= 1 ? params[1] : 0;
    ev->ctrl = mod >= 5 && ((mod - 1) & 4);
    switch (final_byte) {
    case 'A': ev->code = KEY_UP; break;
    case 'B': ev->code = KEY_DOWN; break;
    case 'C': ev->code = KEY_RIGHT; break;
    case 'D': ev->code = KEY_LEFT; break;
    case 'H': ev->code = KEY_HOME; break;
    case 'F': ev->code = KEY_END; break;
    case 'Z': ev->code = KEY_BACKTAB; break;
    case 'P':
        ev->code = KEY_F;
        ev->f = 1;
        break;
    case 'Q':
        ev->code = KEY_F;
        ev->f = 2;
        break;
    case 'S':
        ev->code = KEY_F;
        ev->f = 4;
        break;
    case '~':
        switch (params[0]) {
        case 1:
        case 7: ev->code = KEY_HOME; break;
        case 4:
        case 8: ev->code = KEY_END; break;
        case 3: ev->code = KEY_DELETE; break;
        case 5: ev->code = KEY_PAGE_UP; break;
        case 6: ev->code = KEY_PAGE_DOWN; break;
        case 11:
        case 12:
        case 13:
        case 14:
        case 15:
            ev->code = KEY_F;
            ev->f = (int)params[0] - 10;
            break;
        case 17:
        case 18:
        case 19:
        case 20:
        case 21:
            ev->code = KEY_F;
            ev->f = (int)params[0] - 11;
            break;
        case 23:
        case 24:
            ev->code = KEY_F;
            ev->f = (int)params[0] - 12;
            break;
        default: ev->code = KEY_NONE; break;
        }
        break;
    default: ev->code = KEY_NONE; break;
    }
    return (long)i + 1;
}

static size_t utf8_length(uint8_t lead) {
    if (lead < 0x80) return 1;
    if ((lead & 0xE0) == 0xC0) return 2;
    if ((lead & 0xF0) == 0xE0) return 3;
    if ((lead & 0xF8) == 0xF0) return 4;
    return 1;
}

size_t term_decode(const uint8_t *data, size_t len, bool final, key_event *out, int max,
                   int *count) {
    *count = 0;
    size_t pos = 0;
    while (pos < len && *count < max) {
        uint8_t b = data[pos];
        if (b == 0x1b) {
            key_event ev = {KEY_NONE, 0, 0, false};
            long n = escape(data + pos, len - pos, &ev);
            if (n == 0) {
                if (!final) break;
                push(out, max, count, KEY_ESC, 0, 0, false);
                pos++;
                continue;
            }
            if (n < 0) {
                /* ESC followed by an ordinary key: report the Esc alone and
                 * let the key decode on its own. */
                push(out, max, count, KEY_ESC, 0, 0, false);
                pos++;
                continue;
            }
            if (ev.code != KEY_NONE) push(out, max, count, ev.code, 0, ev.f, ev.ctrl);
            pos += (size_t)n;
            continue;
        }
        if (b == '\r' || b == '\n') {
            push(out, max, count, KEY_ENTER, 0, 0, false);
        } else if (b == '\t') {
            push(out, max, count, KEY_TAB, 0, 0, false);
        } else if (b == 0x7f || b == 0x08) {
            push(out, max, count, KEY_BACKSPACE, 0, 0, false);
        } else if (b >= 0x01 && b <= 0x1a) {
            push(out, max, count, KEY_CHAR, (uint32_t)('a' + b - 1), 0, true);
        } else if (b >= 0x20 && b < 0x7f) {
            push(out, max, count, KEY_CHAR, b, 0, false);
        } else if (b >= 0xc2) {
            size_t need = utf8_length(b);
            if (pos + need > len) {
                if (!final) break;
                pos++;
                continue;
            }
            uint32_t cp = b & (need == 2 ? 0x1Fu : need == 3 ? 0x0Fu : 0x07u);
            bool valid = true;
            for (size_t k = 1; k < need; k++) {
                uint8_t c = data[pos + k];
                if ((c & 0xC0) != 0x80) valid = false;
                cp = (cp << 6) | (c & 0x3Fu);
            }
            if (valid && cp >= 0xA0 && cp <= 0x10FFFF && !(cp >= 0xD800 && cp <= 0xDFFF)) {
                push(out, max, count, KEY_CHAR, cp, 0, false);
            }
            pos += valid ? need : 1;
            continue;
        }
        pos++;
    }
    return pos;
}
