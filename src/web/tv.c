/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/tv.h"

#include <stdlib.h>
#include <string.h>

#include "common/json.h"

#define BLOCK_SIZE 8192u

struct tv_arena_block {
    tv_arena_block *next;
    size_t used;
    size_t size;
    max_align_t data[];
};

const tv tv_undefined_value = {.kind = TV_UNDEFINED};
const tv tv_true_value = {.kind = TV_BOOL, .b = true};
const tv tv_false_value = {.kind = TV_BOOL, .b = false};

void tv_arena_init(tv_arena *a) {
    a->head = NULL;
    a->failed = false;
}

void tv_arena_free(tv_arena *a) {
    while (a->head) {
        tv_arena_block *next = a->head->next;
        free(a->head);
        a->head = next;
    }
}

static void *arena_alloc(tv_arena *a, size_t size) {
    size = (size + 15u) & ~(size_t)15u;
    if (!a->head || a->head->size - a->head->used < size) {
        size_t want = size > BLOCK_SIZE ? size : BLOCK_SIZE;
        tv_arena_block *b = malloc(sizeof(*b) + want);
        if (!b) {
            a->failed = true;
            return NULL;
        }
        b->next = a->head;
        b->used = 0;
        b->size = want;
        a->head = b;
    }
    void *p = (uint8_t *)a->head->data + a->head->used;
    a->head->used += size;
    memset(p, 0, size);
    return p;
}

static tv *make(tv_arena *a, tv_kind kind) {
    tv *v = arena_alloc(a, sizeof(tv));
    if (v) v->kind = kind;
    return v;
}

tv *tv_map(tv_arena *a) { return make(a, TV_MAP); }
tv *tv_list(tv_arena *a) { return make(a, TV_LIST); }
tv *tv_none(tv_arena *a) { return make(a, TV_NONE); }

tv *tv_strn(tv_arena *a, const char *s, size_t len) {
    tv *v = make(a, TV_STR);
    char *copy = arena_alloc(a, len + 1);
    if (!v || !copy) return NULL;
    if (len) memcpy(copy, s, len);
    copy[len] = 0;
    v->s = copy;
    v->len = len;
    return v;
}

tv *tv_str(tv_arena *a, const char *s) { return tv_strn(a, s ? s : "", s ? strlen(s) : 0); }

tv *tv_bool(tv_arena *a, bool b) {
    tv *v = make(a, TV_BOOL);
    if (v) v->b = b;
    return v;
}

tv *tv_int(tv_arena *a, int64_t i) {
    tv *v = make(a, TV_INT);
    if (v) v->i = i;
    return v;
}

tv *tv_float(tv_arena *a, double f) {
    tv *v = make(a, TV_FLOAT);
    if (v) v->f = f;
    return v;
}

static bool grow(tv_arena *a, tv *c) {
    if (c->count < c->cap) return true;
    size_t cap = c->cap ? c->cap * 2 : 8;
    tv **items = arena_alloc(a, cap * sizeof(tv *));
    const char **keys = c->kind == TV_MAP ? arena_alloc(a, cap * sizeof(char *)) : NULL;
    if (!items || (c->kind == TV_MAP && !keys)) return false;
    if (c->count) {
        memcpy(items, c->items, c->count * sizeof(tv *));
        if (keys) memcpy(keys, c->keys, c->count * sizeof(char *));
    }
    c->items = items;
    c->keys = keys;
    c->cap = cap;
    return true;
}

void tv_set(tv_arena *a, tv *map, const char *key, tv *value) {
    if (!map || map->kind != TV_MAP || !value) {
        a->failed = true;
        return;
    }
    for (size_t i = 0; i < map->count; i++)
        if (strcmp(map->keys[i], key) == 0) {
            map->items[i] = value;
            return;
        }
    tv *k = tv_str(a, key);
    if (!k || !grow(a, map)) {
        a->failed = true;
        return;
    }
    map->keys[map->count] = k->s;
    map->items[map->count++] = value;
}

void tv_push(tv_arena *a, tv *list, tv *value) {
    if (!list || list->kind != TV_LIST || !value || !grow(a, list)) {
        a->failed = true;
        return;
    }
    list->items[list->count++] = value;
}

const tv *tv_attr(const tv *v, const char *name) {
    if (!v || v->kind != TV_MAP) return &tv_undefined_value;
    for (size_t i = 0; i < v->count; i++)
        if (strcmp(v->keys[i], name) == 0) return v->items[i];
    return &tv_undefined_value;
}

const tv *tv_lookup(const tv_scope *scope, const char *name) {
    for (const tv_scope *s = scope; s; s = s->parent) {
        if (s->name && strcmp(s->name, name) == 0) return s->value;
        if (s->context) return tv_attr(s->context, name);
    }
    return &tv_undefined_value;
}

bool tv_truthy(const tv *v) {
    switch (v->kind) {
    case TV_UNDEFINED:
    case TV_NONE: return false;
    case TV_BOOL: return v->b;
    case TV_INT: return v->i != 0;
    case TV_FLOAT: return v->f != 0.0;
    case TV_STR: return v->len != 0;
    case TV_LIST:
    case TV_MAP: return v->count != 0;
    }
    return false;
}

bool tv_equal(const tv *a, const tv *b) {
    if (a->kind == TV_STR && b->kind == TV_STR)
        return a->len == b->len && memcmp(a->s, b->s, a->len) == 0;
    if ((a->kind == TV_INT || a->kind == TV_FLOAT) && (b->kind == TV_INT || b->kind == TV_FLOAT)) {
        double x = a->kind == TV_INT ? (double)a->i : a->f;
        double y = b->kind == TV_INT ? (double)b->i : b->f;
        return x == y;
    }
    if (a->kind != b->kind) return false;
    if (a->kind == TV_BOOL) return a->b == b->b;
    return a->kind == TV_NONE || a->kind == TV_UNDEFINED;
}

size_t tv_length(const tv *v) { return v->kind == TV_LIST || v->kind == TV_MAP ? v->count : 0; }

const tv *tv_index(const tv *v, size_t i) {
    return v->kind == TV_LIST && i < v->count ? v->items[i] : &tv_undefined_value;
}

void tv_html_escape(omt_buf *out, const char *s, size_t len) {
    size_t run = 0;
    for (size_t i = 0; i < len; i++) {
        const char *e = NULL;
        switch (s[i]) {
        case '&': e = "&amp;"; break;
        case '<': e = "&lt;"; break;
        case '>': e = "&gt;"; break;
        case '"': e = "&quot;"; break;
        case '\'': e = "&#x27;"; break;
        case '/': e = "&#x2f;"; break;
        default: break;
        }
        if (e) {
            omt_buf_append(out, s + i - run, run);
            run = 0;
            omt_buf_puts(out, e);
        } else {
            run++;
        }
    }
    omt_buf_append(out, s + len - run, run);
}

void tv_render(omt_buf *out, const tv *v) {
    switch (v->kind) {
    case TV_UNDEFINED: return;
    case TV_NONE: omt_buf_puts(out, "none"); return;
    case TV_BOOL: omt_buf_puts(out, v->b ? "true" : "false"); return;
    case TV_INT: omt_buf_printf(out, "%lld", (long long)v->i); return;
    case TV_FLOAT: {
        /* minijinja prints a float with Rust's Display and appends ".0" to
         * one that has no fractional part shown. */
        omt_buf text;
        omt_buf_init(&text, 64);
        omt_fmt_f64_display(&text, v->f);
        if (!memchr(text.data, '.', text.len) && !memchr(text.data, 'N', text.len) &&
            !memchr(text.data, 'i', text.len))
            omt_buf_puts(&text, ".0");
        omt_buf_append(out, text.data, text.len);
        omt_buf_free(&text);
        return;
    }
    case TV_STR: tv_html_escape(out, v->s, v->len); return;
    case TV_LIST:
    case TV_MAP:
        /* Never rendered by the shipped templates; refuse rather than invent
         * a serialization. */
        return;
    }
}
