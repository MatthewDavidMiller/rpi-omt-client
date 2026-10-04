/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The reference codec's Exp-Golomb bit reader. The C original reads through a
 * fixed 0xFF-filled slice buffer, lets its bit counter go negative on damaged
 * input, and relies on undefined shift behaviour to recover. This port keeps
 * the reference behaviour for every well-formed stream and turns each of those
 * undefined cases into an explicit `corrupt` flag instead.
 *
 * Everything here is inline: it is the innermost loop of the entropy decode.
 */
#ifndef OMT_VMX_BITSTREAM_H
#define OMT_VMX_BITSTREAM_H

#include <stdlib.h>
#include <string.h>

#include "common/base.h"

/* Bytes of 0xFF padding kept past the loaded payload so the reader's
 * eight-byte lookahead always lands inside the allocation. */
#define VMX_PADDING 64u

typedef struct {
    uint8_t *buffer;
    size_t allocated;
    /* Exclusive end of the loaded payload plus its padding. Reads past this
     * are corrupt; bytes left from a larger earlier load are not visible. */
    size_t length;
    size_t capacity; /* largest payload this reader accepts */
    size_t position;
    int32_t bits_left;
    uint64_t window;
    bool corrupt;
} vmx_bits;

static inline uint64_t vmx_bits_window_at(vmx_bits *r, size_t position) {
    if (position > r->length || r->length - position < 8) {
        r->corrupt = true;
        return UINT64_MAX;
    }
    const uint8_t *p = r->buffer + position;
    return ((uint64_t)p[0] << 56) | ((uint64_t)p[1] << 48) | ((uint64_t)p[2] << 40) |
           ((uint64_t)p[3] << 32) | ((uint64_t)p[4] << 24) | ((uint64_t)p[5] << 16) |
           ((uint64_t)p[6] << 8) | (uint64_t)p[7];
}

/* VMX_ResetData. */
static inline void vmx_bits_reset(vmx_bits *r) {
    r->position = 0;
    r->bits_left = 64;
    r->corrupt = false;
    r->window = vmx_bits_window_at(r, 0);
}

/* Creates an empty reader whose payload may grow up to `capacity` bytes. The
 * padded store is allocated only on load, so a 1080p decoder does not touch
 * megabytes per slice before the first frame arrives. */
static inline bool vmx_bits_init(vmx_bits *r, size_t capacity) {
    memset(r, 0, sizeof(*r));
    if (capacity > SIZE_MAX - VMX_PADDING) return false;
    r->buffer = malloc(VMX_PADDING);
    if (!r->buffer) return false;
    memset(r->buffer, 0xFF, VMX_PADDING);
    r->allocated = VMX_PADDING;
    r->length = VMX_PADDING;
    r->capacity = capacity;
    r->bits_left = 64;
    return true;
}

static inline void vmx_bits_free(vmx_bits *r) {
    free(r->buffer);
    r->buffer = NULL;
}

/* Replaces the slice contents, restoring the 0xFF padding the reference
 * codec depends on to terminate an over-read. */
static inline bool vmx_bits_load(vmx_bits *r, const uint8_t *data, size_t len) {
    if (len > r->capacity) return false;
    size_t needed = len + VMX_PADDING;
    if (needed > r->allocated) {
        uint8_t *grown = realloc(r->buffer, needed);
        if (!grown) return false;
        r->buffer = grown;
        r->allocated = needed;
    }
    if (len) memcpy(r->buffer, data, len);
    memset(r->buffer + len, 0xFF, VMX_PADDING);
    r->length = needed;
    vmx_bits_reset(r);
    return true;
}

static inline uint64_t vmx_shl64(uint64_t v, uint32_t n) { return n >= 64 ? 0 : v << n; }
static inline int32_t vmx_clz64(uint64_t v) { return v ? __builtin_clzll(v) : 64; }

/* FLUSHREADBITS. */
static inline void vmx_bits_flush(vmx_bits *r) {
    if (r->bits_left == 0) {
        r->bits_left = 64;
        r->position += 8;
        r->window = vmx_bits_window_at(r, r->position);
    }
}

/* RELOADBITS. */
static inline void vmx_bits_reload(vmx_bits *r) {
    if (r->bits_left < 32) {
        if (r->bits_left < 0) {
            r->corrupt = true;
            return;
        }
        size_t advance = (size_t)((64 - r->bits_left) >> 3);
        r->position += advance;
        r->window = vmx_bits_window_at(r, r->position);
        r->bits_left += (int32_t)(advance << 3);
    }
}

/* Reads the `count` bits immediately above the new bits_left. */
static inline uint64_t vmx_bits_take(vmx_bits *r, int32_t count) {
    r->bits_left -= count;
    if (r->bits_left < 0 || count < 0 || count > 64) {
        r->corrupt = true;
        if (r->bits_left < 0) r->bits_left = 0;
        return 0;
    }
    uint64_t mask = count >= 64 ? UINT64_MAX : (((uint64_t)1 << count) - 1);
    uint64_t shifted = r->bits_left >= 64 ? 0 : r->window >> r->bits_left;
    return shifted & mask;
}

/* GETBITB: one bit without a reload. */
static inline uint64_t vmx_bits_bit_bare(vmx_bits *r) { return vmx_bits_take(r, 1); }

/* GETBIT: one bit, flushing an exhausted window. */
static inline uint64_t vmx_bits_bit(vmx_bits *r) {
    uint64_t v = vmx_bits_take(r, 1);
    vmx_bits_flush(r);
    return v;
}

static inline int32_t vmx_bits_leading_zeros(vmx_bits *r) {
    if (r->bits_left <= 0 || r->bits_left > 64) {
        r->corrupt = true;
        return 0;
    }
    return vmx_clz64(vmx_shl64(r->window, (uint32_t)(64 - r->bits_left)));
}

/* GETZEROSB: leading zeros inside the current window. */
static inline int32_t vmx_bits_zeros_bare(vmx_bits *r) {
    int32_t count = vmx_bits_leading_zeros(r);
    if (count > r->bits_left) {
        /* The reference lets the counter go negative and recovers through an
         * undefined shift. Refuse the stream instead. */
        r->corrupt = true;
        return 0;
    }
    r->bits_left -= count;
    return count;
}

/* GETZEROS: leading zeros, continuing into the next window if needed. */
static inline int32_t vmx_bits_zeros(vmx_bits *r) {
    int32_t count = vmx_bits_leading_zeros(r);
    if (count >= r->bits_left) {
        count = r->bits_left;
        r->bits_left = 0;
        vmx_bits_flush(r);
        int32_t extra = vmx_clz64(r->window);
        if (extra > r->bits_left) {
            r->corrupt = true;
            return 0;
        }
        r->bits_left -= extra;
        count += extra;
    } else {
        r->bits_left -= count;
    }
    return count;
}

/* GETBITSB: `count` bits without a reload. */
static inline uint64_t vmx_bits_bits_bare(vmx_bits *r, int32_t count) {
    return vmx_bits_take(r, count);
}

/* GETBITS: `count` bits, flushing across window boundaries. */
static inline uint64_t vmx_bits_bits(vmx_bits *r, int32_t count) {
    if (count < 0) {
        r->corrupt = true;
        return 0;
    }
    int32_t remaining = count;
    uint64_t value = 0;
    while (remaining > 0) {
        int32_t take = remaining < r->bits_left ? remaining : r->bits_left;
        if (take <= 0) {
            r->corrupt = true;
            return value;
        }
        if (value != 0) value = vmx_shl64(value, (uint32_t)take);
        value |= vmx_bits_take(r, take);
        remaining -= take;
        vmx_bits_flush(r);
    }
    return value;
}

/* FLUSHREMAININGREADBITS: realign to the next byte boundary. */
static inline void vmx_bits_align(vmx_bits *r) {
    if (r->bits_left < 64) {
        int32_t remainder = r->bits_left & 7;
        (void)vmx_bits_bits(r, remainder);
    }
    vmx_bits_flush(r);
}

#endif
