/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The algorithm is the one in UAX #15: full canonical decomposition, the
 * canonical ordering of combining marks, then canonical composition. Hangul
 * syllables are decomposed and composed arithmetically rather than by table.
 */
#include "common/nfc.h"

#include <stdlib.h>
#include <string.h>

#include "common/json.h"
#include "common/nfc_tables.h"

enum {
    S_BASE = 0xAC00,
    L_BASE = 0x1100,
    V_BASE = 0x1161,
    T_BASE = 0x11A7,
    L_COUNT = 19,
    V_COUNT = 21,
    T_COUNT = 28,
    N_COUNT = V_COUNT * T_COUNT,
    S_COUNT = L_COUNT * N_COUNT,
};

uint8_t omt_nfc_combining_class(uint32_t cp) {
    size_t lo = 0, hi = omt_nfc_ccc_count;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (omt_nfc_ccc_table[mid].cp == cp) return omt_nfc_ccc_table[mid].ccc;
        if (omt_nfc_ccc_table[mid].cp < cp)
            lo = mid + 1;
        else
            hi = mid;
    }
    return 0;
}

static const omt_nfc_decomp *find_decomp(uint32_t cp) {
    size_t lo = 0, hi = omt_nfc_decomp_count;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        if (omt_nfc_decomp_table[mid].cp == cp) return &omt_nfc_decomp_table[mid];
        if (omt_nfc_decomp_table[mid].cp < cp)
            lo = mid + 1;
        else
            hi = mid;
    }
    return NULL;
}

static uint32_t compose_pair(uint32_t a, uint32_t b) {
    if (a >= L_BASE && a < L_BASE + L_COUNT && b >= V_BASE && b < V_BASE + V_COUNT)
        return S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT;
    if (a >= S_BASE && a < S_BASE + S_COUNT && (a - S_BASE) % T_COUNT == 0 && b > T_BASE &&
        b < T_BASE + T_COUNT)
        return a + (b - T_BASE);
    size_t lo = 0, hi = omt_nfc_comp_count;
    while (lo < hi) {
        size_t mid = lo + (hi - lo) / 2;
        const omt_nfc_comp *c = &omt_nfc_comp_table[mid];
        if (c->first == a && c->second == b) return c->composite;
        if (c->first < a || (c->first == a && c->second < b))
            lo = mid + 1;
        else
            hi = mid;
    }
    return 0;
}

typedef struct {
    uint32_t *cp;
    size_t len;
    size_t cap;
    size_t limit;
} cps;

static bool push(cps *v, uint32_t cp) {
    if (v->len == v->cap) {
        if (v->cap >= v->limit) return false;
        size_t cap = v->cap ? v->cap * 2 : 32;
        if (cap > v->limit) cap = v->limit;
        uint32_t *grown = realloc(v->cp, cap * sizeof(uint32_t));
        if (!grown) return false;
        v->cp = grown;
        v->cap = cap;
    }
    v->cp[v->len++] = cp;
    return true;
}

/* Canonical mappings nest at most a few levels; the depth bound is generous. */
static bool decompose(cps *v, uint32_t cp, int depth) {
    if (depth > 8) return false;
    if (cp >= S_BASE && cp < S_BASE + S_COUNT) {
        uint32_t index = cp - S_BASE;
        if (!push(v, L_BASE + index / N_COUNT)) return false;
        if (!push(v, V_BASE + (index % N_COUNT) / T_COUNT)) return false;
        if (index % T_COUNT && !push(v, T_BASE + index % T_COUNT)) return false;
        return true;
    }
    const omt_nfc_decomp *d = find_decomp(cp);
    if (!d) return push(v, cp);
    if (!decompose(v, d->first, depth + 1)) return false;
    return d->second == 0 || decompose(v, d->second, depth + 1);
}

static bool to_nfc(const char *s, size_t len, cps *v) {
    if (!omt_utf8_valid(s, len)) return false;
    size_t i = 0;
    while (i < len)
        if (!decompose(v, omt_utf8_next(s, len, &i), 0)) return false;
    /* Canonical ordering: a stable insertion sort of each run of non-starters
     * by combining class. */
    for (size_t k = 1; k < v->len; k++) {
        uint8_t c = omt_nfc_combining_class(v->cp[k]);
        if (c == 0) continue;
        size_t j = k;
        uint32_t cp = v->cp[k];
        while (j > 0) {
            uint8_t prev = omt_nfc_combining_class(v->cp[j - 1]);
            if (prev == 0 || prev <= c) break;
            v->cp[j] = v->cp[j - 1];
            j--;
        }
        v->cp[j] = cp;
    }
    /* Canonical composition. */
    if (v->len == 0) return true;
    size_t starter = SIZE_MAX;
    size_t out = 0;
    int last_class = -1;
    for (size_t k = 0; k < v->len; k++) {
        uint32_t cp = v->cp[k];
        int c = omt_nfc_combining_class(cp);
        if (starter != SIZE_MAX) {
            bool blocked = last_class != -1 && (last_class == 0 || last_class >= c);
            /* last_class tracks the most recent character kept after the
             * starter; a starter directly before is never blocking. */
            if (!blocked) {
                uint32_t composite = compose_pair(v->cp[starter], cp);
                if (composite) {
                    v->cp[starter] = composite;
                    continue;
                }
            }
        }
        if (c == 0) {
            starter = out;
            last_class = -1;
        } else {
            last_class = c;
        }
        v->cp[out++] = cp;
    }
    v->len = out;
    return true;
}

bool omt_nfc_normalize(const char *s, size_t len, omt_buf *out) {
    cps v = {NULL, 0, 0, 0};
    if (!omt_mul(len, (size_t)4, &v.limit)) return false;
    v.limit += 16;
    bool ok = to_nfc(s, len, &v);
    for (size_t k = 0; ok && k < v.len; k++) omt_utf8_put(out, v.cp[k]);
    free(v.cp);
    return ok && !out->failed;
}

bool omt_nfc_is_normalized(const char *s, size_t len, size_t limit) {
    if (len > limit) return false;
    cps v = {NULL, 0, 0, 0};
    v.limit = len * 4 + 16;
    bool ok = to_nfc(s, len, &v);
    if (ok) {
        size_t i = 0, k = 0;
        while (ok && i < len) {
            if (k >= v.len || omt_utf8_next(s, len, &i) != v.cp[k++]) ok = false;
        }
        if (k != v.len) ok = false;
    }
    free(v.cp);
    return ok;
}
