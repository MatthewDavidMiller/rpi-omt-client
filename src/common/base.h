/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Shared definitions every module includes: fixed-width types, checked
 * arithmetic, and the attributes that make an ignored error a warning.
 */
#ifndef OMT_BASE_H
#define OMT_BASE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define OMT_NODISCARD __attribute__((warn_unused_result))
#if defined(__MINGW32__)
/* With __USE_MINGW_ANSI_STDIO the formatter is the C99 one, not msvcrt's. */
#define OMT_PRINTF(fmt, args) __attribute__((format(gnu_printf, fmt, args)))
#else
#define OMT_PRINTF(fmt, args) __attribute__((format(printf, fmt, args)))
#endif
#define OMT_UNUSED __attribute__((unused))
/* For the lane helpers of the hot decode kernels, where leaving the decision
 * to the inliner lost the constant folding the kernels depend on. */
#define OMT_ALWAYS_INLINE inline __attribute__((always_inline))
#define OMT_ARRAY_LEN(a) (sizeof(a) / sizeof((a)[0]))

/* Checked size arithmetic. Each returns false on overflow and leaves *out
 * unspecified, so a caller cannot use a wrapped length by accident. */
#define omt_add(a, b, out) (!__builtin_add_overflow((a), (b), (out)))
#define omt_mul(a, b, out) (!__builtin_mul_overflow((a), (b), (out)))
#define omt_sub(a, b, out) (!__builtin_sub_overflow((a), (b), (out)))

static inline size_t omt_min_size(size_t a, size_t b) { return a < b ? a : b; }
static inline size_t omt_max_size(size_t a, size_t b) { return a > b ? a : b; }

/* Little-endian loads and stores on byte arrays, independent of the host. */
static inline uint16_t omt_le16(const uint8_t *p) {
    return (uint16_t)(p[0] | (uint16_t)(p[1] << 8));
}
static inline uint32_t omt_le32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
static inline uint64_t omt_le64(const uint8_t *p) {
    return (uint64_t)omt_le32(p) | ((uint64_t)omt_le32(p + 4) << 32);
}
static inline void omt_put_le16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)v;
    p[1] = (uint8_t)(v >> 8);
}
static inline void omt_put_le32(uint8_t *p, uint32_t v) {
    for (int i = 0; i < 4; i++) p[i] = (uint8_t)(v >> (8 * i));
}
static inline void omt_put_le64(uint8_t *p, uint64_t v) {
    for (int i = 0; i < 8; i++) p[i] = (uint8_t)(v >> (8 * i));
}
static inline uint32_t omt_be32(const uint8_t *p) {
    return ((uint32_t)p[0] << 24) | ((uint32_t)p[1] << 16) | ((uint32_t)p[2] << 8) | (uint32_t)p[3];
}
static inline void omt_put_be32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)(v >> 24);
    p[1] = (uint8_t)(v >> 16);
    p[2] = (uint8_t)(v >> 8);
    p[3] = (uint8_t)v;
}

#endif
