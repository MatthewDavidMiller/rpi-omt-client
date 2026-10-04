/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Shared between the decoder's translation units; not part of its API.
 *
 * Arithmetic notes. The reference codec is written in SSE2 over 16-bit lanes,
 * and several of its steps saturate, wrap, or shift negative values. Every
 * helper below spells the exact semantics out: wrapping is done in unsigned
 * arithmetic, and right shifts of negative values rely on the arithmetic shift
 * GCC and Clang guarantee (checked by a static assertion in tables.c).
 */
#ifndef OMT_VMX_INTERNAL_H
#define OMT_VMX_INTERNAL_H

#include "vmx/vmx.h"

#define VMX_SLICE_HEIGHT 16u
#define VMX_QUALITY_COUNT 25u
#define VMX_BITS_INV_ACC 5
#define VMX_SHIFT_INV_ROW (16 - VMX_BITS_INV_ACC)
#define VMX_SHIFT_INV_COL (1 + VMX_BITS_INV_ACC)
#define VMX_IRND_INV_ROW ((int16_t)(1024 * (6 - VMX_BITS_INV_ACC)))
#define VMX_IRND_INV_COL ((int16_t)(16 * (VMX_BITS_INV_ACC - 3)))
#define VMX_IRND_INV_CORR ((int16_t)(16 * (VMX_BITS_INV_ACC - 3) - 1))
#define VMX_TG_1_16 ((int16_t)13036)
#define VMX_TG_2_16 ((int16_t)27146)
#define VMX_TG_3_16 ((int16_t)-21746)
#define VMX_COS_4_16 ((int16_t)-19195)

extern const uint16_t vmx_quantization_matrix[64];
extern const int32_t vmx_quality[VMX_QUALITY_COUNT];
extern const int16_t vmx_yuv_rgb_709[5];
extern const int16_t vmx_yuv_rgb_601[5];

/* The inverse transform's tables, visible to the kernels at compile time.
 * As constants the row pass's multiplies fold into vector multiply-adds; as
 * extern data in another file the compiler had to load and multiply each
 * coefficient, which made the portable transform about twice as slow. */
/* The row-major position of each coefficient, indexed by its place in the
 * stream's zig-zag order. The entropy decoder stores every coefficient where
 * the transform reads it, so neither kernel permutes the block. */
static const uint8_t vmx_zigzag_natural[64] = {
    0,  1,  8,  16, 9,  2,  3,  10, 17, 24, 32, 25, 18, 11, 4,  5,  12, 19, 26, 33, 40, 48,
    41, 34, 27, 20, 13, 6,  7,  14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23,
    30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
};
static const int16_t vmx_tab_i_04[32] = {
    16384,  21407,  16384, 8867,  16384,  -8867,  16384, -21407, 16384, 8867,   -16384,
    -21407, -16384, 21407, 16384, -8867,  22725,  19266, 19266,  -4520, 12873,  -22725,
    4520,   -12873, 12873, 4520,  -22725, -12873, 4520,  19266,  19266, -22725,
};
static const int16_t vmx_tab_i_17[32] = {
    22725,  29692,  22725, 12299, 22725,  -12299, 22725, -29692, 22725, 12299,  -22725,
    -29692, -22725, 29692, 22725, -12299, 31521,  26722, 26722,  -6270, 17855,  -31521,
    6270,   -17855, 17855, 6270,  -31521, -17855, 6270,  26722,  26722, -31521,
};
static const int16_t vmx_tab_i_26[32] = {
    21407,  27969,  21407, 11585, 21407,  -11585, 21407, -27969, 21407, 11585,  -21407,
    -27969, -21407, 27969, 21407, -11585, 29692,  25172, 25172,  -5906, 16819,  -29692,
    5906,   -16819, 16819, 5906,  -29692, -16819, 5906,  25172,  25172, -29692,
};
static const int16_t vmx_tab_i_35[32] = {
    19266,  25172,  19266, 10426, 19266,  -10426, 19266, -25172, 19266, 10426,  -19266,
    -25172, -19266, 25172, 19266, -10426, 26722,  22654, 22654,  -5315, 15137,  -26722,
    5315,   -15137, 15137, 5315,  -26722, -15137, 5315,  22654,  22654, -26722,
};

void vmx_decode_matrix(size_t index, uint16_t out[64]);
size_t vmx_quality_index(int32_t quality);

/* --- 16-bit lane arithmetic, as the SSE2 reference performs it --------- */

static inline int16_t vmx_sat16(int32_t v) {
    return (int16_t)(v < INT16_MIN ? INT16_MIN : v > INT16_MAX ? INT16_MAX : v);
}
static inline uint8_t vmx_sat8(int16_t v) { return (uint8_t)(v < 0 ? 0 : v > 255 ? 255 : v); }
static inline int16_t vmx_adds(int16_t a, int16_t b) { return vmx_sat16((int32_t)a + b); }
static inline int16_t vmx_subs(int16_t a, int16_t b) { return vmx_sat16((int32_t)a - b); }
/* _mm_mulhi_epi16: the high half of the signed 32-bit product. */
static inline int16_t vmx_mulhi(int16_t a, int16_t b) {
    return (int16_t)(((int32_t)a * (int32_t)b) >> 16);
}
/* Two's-complement wrapping addition and multiplication, without UB. */
static inline int32_t vmx_wrap_add32(int32_t a, int32_t b) {
    return (int32_t)((uint32_t)a + (uint32_t)b);
}
static inline int32_t vmx_wrap_sub32(int32_t a, int32_t b) {
    return (int32_t)((uint32_t)a - (uint32_t)b);
}
static inline int16_t vmx_wrap_add16(int16_t a, int16_t b) {
    return (int16_t)(uint16_t)((uint16_t)a + (uint16_t)b);
}
static inline int16_t vmx_wrap_mul16(int16_t a, int16_t b) {
    return (int16_t)(uint16_t)((uint32_t)(uint16_t)a * (uint32_t)(uint16_t)b);
}

/* --- kernels ---------------------------------------------------------- */

void vmx_idct_scalar(const int16_t block[64], const uint16_t matrix[64], uint8_t *dst,
                     size_t stride, int16_t add_value);
#if defined(__aarch64__)
void vmx_idct_neon(const int16_t block[64], const uint16_t matrix[64], uint8_t *dst, size_t stride,
                   int16_t add_value);
#define vmx_idct vmx_idct_neon
#else
#define vmx_idct vmx_idct_scalar
#endif
void vmx_broadcast_dc(int16_t dc, uint8_t *dst, size_t stride, int16_t add_value);

/* One output row: `width` pixels from luma[width] and blue/red[width/2]. */
void vmx_uyvy_row(const uint8_t *luma, const uint8_t *blue, const uint8_t *red, size_t width,
                  uint8_t *out);
void vmx_bgra_row_scalar(const uint8_t *luma, const uint8_t *blue, const uint8_t *red, size_t width,
                         uint8_t *out, const int16_t coefficients[5]);
#if defined(__aarch64__)
void vmx_bgra_row_neon(const uint8_t *luma, const uint8_t *blue, const uint8_t *red, size_t width,
                       uint8_t *out, const int16_t coefficients[5]);
#define vmx_bgra_row vmx_bgra_row_neon
#else
#define vmx_bgra_row vmx_bgra_row_scalar
#endif

#endif
