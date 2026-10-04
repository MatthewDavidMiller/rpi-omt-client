/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Strict JSON: a parser for the records this project reads from disk and
 * stdin, and the escaping its writers share.
 *
 * The parser accepts RFC 8259 JSON and nothing looser -- no comments, no
 * trailing commas, no lone surrogates, no invalid UTF-8 -- and additionally
 * refuses a repeated object key at any depth, because a duplicate is how a
 * crafted record makes two readers disagree about one field. Parsed values
 * live in an arena owned by an omt_json_doc and are freed together.
 */
#ifndef OMT_JSON_H
#define OMT_JSON_H

#include "common/base.h"
#include "common/buf.h"

typedef enum {
    OMT_JSON_NULL,
    OMT_JSON_BOOL,
    OMT_JSON_NUMBER,
    OMT_JSON_STRING,
    OMT_JSON_ARRAY,
    OMT_JSON_OBJECT,
} omt_json_type;

typedef struct omt_json omt_json;
struct omt_json {
    omt_json_type type;
    bool boolean;
    /* Numbers keep their lexical class: an integer literal has no fraction
     * or exponent, and a typed reader that wants an integer refuses `1.0`,
     * exactly as serde does. */
    bool is_integer;
    bool negative;
    uint64_t magnitude; /* valid when is_integer and it fits */
    bool magnitude_overflow;
    double number;
    char *string; /* NUL-terminated; string_len excludes the terminator */
    size_t string_len;
    size_t count; /* array items or object members */
    omt_json **items;
    char **keys;
    size_t *key_lens;
};

typedef struct omt_json_block omt_json_block;
typedef struct {
    omt_json_block *blocks;
    size_t used;
    size_t limit; /* total arena bytes this document may use */
    size_t total;
    char error[96];
} omt_json_doc;

/* Parses `len` bytes. Returns NULL on any error, with doc->error set. The
 * arena is capped at `arena_limit` bytes (0 means 64x the input plus 64 KiB). */
OMT_NODISCARD omt_json *omt_json_parse(omt_json_doc *doc, const char *text, size_t len,
                                       size_t arena_limit);
void omt_json_doc_free(omt_json_doc *doc);
/* Zeroes every value the document held before freeing it, for secrets. */
void omt_json_doc_free_secret(omt_json_doc *doc);

const omt_json *omt_json_get(const omt_json *object, const char *key);
/* True when every key of `object` is in `allowed` -- serde's
 * deny_unknown_fields. */
bool omt_json_only_keys(const omt_json *object, const char *const *allowed, size_t count);
bool omt_json_as_u64(const omt_json *value, uint64_t *out);
bool omt_json_as_i64(const omt_json *value, int64_t *out);
/* A string with no embedded NUL. */
const char *omt_json_as_str(const omt_json *value);

/* Appends `s` as a quoted JSON string with serde_json's escaping. */
void omt_json_write_string(omt_buf *out, const char *s, size_t len);
static inline void omt_json_write_cstr(omt_buf *out, const char *s) {
    omt_json_write_string(out, s, __builtin_strlen(s));
}
/* Appends a finite double the way serde_json does: shortest round-trip
 * digits, and a ".0" on integral values. */
void omt_json_write_f64(omt_buf *out, double value);

/* UTF-8 helpers shared with the other validators. */
bool omt_utf8_valid(const char *s, size_t len);
/* Decodes one code point at s[*i], advancing *i. Input must be valid. */
uint32_t omt_utf8_next(const char *s, size_t len, size_t *i);
/* Appends the UTF-8 encoding of `cp` to out. */
void omt_utf8_put(omt_buf *out, uint32_t cp);
/* Appends `s` with every invalid UTF-8 sequence replaced by U+FFFD, one per
 * maximal invalid subpart -- Rust's String::from_utf8_lossy. */
void omt_utf8_lossy(omt_buf *out, const char *s, size_t len);
/* Appends a finite double the way Rust's `Display` prints f64: shortest
 * round-trip digits, never an exponent, and no ".0" on integral values. */
void omt_fmt_f64_display(omt_buf *out, double value);

/* Rust's char::is_whitespace: the Unicode White_Space property. */
bool omt_unicode_is_whitespace(uint32_t cp);
/* Rust's str::trim over valid UTF-8: returns the trimmed view. */
omt_span omt_utf8_trim(const char *s, size_t len);

#endif
