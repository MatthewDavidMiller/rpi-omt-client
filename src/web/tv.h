/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Template values: the small dynamic tree a page's context is built from, and
 * the operations the generated template code performs on it -- lookup,
 * truthiness, equality, and HTML-escaped output.
 *
 * Every value of one render lives in an arena that is released in one call,
 * so building a context cannot leak along an error path. Lookups of a missing
 * name or attribute yield `undefined`, which renders as nothing and is false,
 * as minijinja's lenient mode does.
 */
#ifndef OMT_WEB_TV_H
#define OMT_WEB_TV_H

#include "common/base.h"
#include "common/buf.h"

typedef enum {
    TV_UNDEFINED,
    TV_NONE,
    TV_BOOL,
    TV_INT,
    TV_FLOAT,
    TV_STR,
    TV_LIST,
    TV_MAP,
} tv_kind;

typedef struct tv tv;
struct tv {
    tv_kind kind;
    bool b;
    int64_t i;
    double f;
    const char *s;
    size_t len;
    /* list items, or map values with parallel keys */
    tv **items;
    const char **keys;
    size_t count;
    size_t cap;
};

typedef struct tv_arena_block tv_arena_block;
typedef struct {
    tv_arena_block *head;
    bool failed;
} tv_arena;

void tv_arena_init(tv_arena *a);
void tv_arena_free(tv_arena *a);

tv *tv_map(tv_arena *a);
tv *tv_list(tv_arena *a);
tv *tv_str(tv_arena *a, const char *s);
tv *tv_strn(tv_arena *a, const char *s, size_t len);
tv *tv_bool(tv_arena *a, bool b);
tv *tv_int(tv_arena *a, int64_t i);
tv *tv_float(tv_arena *a, double f);
tv *tv_none(tv_arena *a);
/* Insert or replace a key; the key is copied. */
void tv_set(tv_arena *a, tv *map, const char *key, tv *value);
void tv_push(tv_arena *a, tv *list, tv *value);
static inline void tv_set_str(tv_arena *a, tv *map, const char *key, const char *s) {
    tv_set(a, map, key, tv_str(a, s));
}
static inline void tv_set_bool(tv_arena *a, tv *map, const char *key, bool b) {
    tv_set(a, map, key, tv_bool(a, b));
}
static inline void tv_set_int(tv_arena *a, tv *map, const char *key, int64_t i) {
    tv_set(a, map, key, tv_int(a, i));
}

extern const tv tv_undefined_value;
extern const tv tv_true_value;
extern const tv tv_false_value;

/* A chain of name bindings: a template's loop variables and `set` names in
 * front of the render context. */
typedef struct tv_scope tv_scope;
struct tv_scope {
    const tv_scope *parent;
    const char *name;
    const tv *value;
    const tv *context; /* only on the root scope */
};

const tv *tv_lookup(const tv_scope *scope, const char *name);
const tv *tv_attr(const tv *v, const char *name);
bool tv_truthy(const tv *v);
bool tv_equal(const tv *a, const tv *b);
static inline const tv *tv_from_bool(bool b) { return b ? &tv_true_value : &tv_false_value; }
size_t tv_length(const tv *v);
const tv *tv_index(const tv *v, size_t i);

/* Appends `v` as minijinja prints it into HTML: strings auto-escaped,
 * booleans as true/false, none as "none", undefined as nothing. */
void tv_render(omt_buf *out, const tv *v);
void tv_html_escape(omt_buf *out, const char *s, size_t len);

#endif
