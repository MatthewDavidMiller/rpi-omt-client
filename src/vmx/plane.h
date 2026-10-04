/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#ifndef OMT_VMX_PLANE_H
#define OMT_VMX_PLANE_H

#include "vmx/bitstream.h"
#include "vmx/internal.h"

/* Decode state that persists across the planes of one slice. */
typedef struct {
    vmx_bits dc;
    vmx_bits ac;
} vmx_slice_streams;

/* Decodes one plane of one slice into `dst`, which holds at least
 * stride * VMX_SLICE_HEIGHT bytes. `bias` is 128 for luma, 0 for chroma.
 * Returns false when the stream was rejected as damaged. */
bool vmx_decode_plane(vmx_slice_streams *streams, size_t stride, int16_t bias,
                      const uint16_t matrix[64], int32_t dc_shift, uint8_t *dst, size_t dst_len);

/* Every AC code of at most VMX_LOOKAHEAD_BITS bits, resolved from the next
 * VMX_LOOKAHEAD_BITS of the stream (tools/gen/gen_vmx_tables.py). An entry is
 * payload | length << 16 | kind << 24, and zero when the code is longer. */
#define VMX_LOOKAHEAD_BITS 12
#define VMX_LOOKAHEAD_ZEROS 1u
#define VMX_LOOKAHEAD_VALUE 2u
extern const uint32_t vmx_ac_lookahead[1u << VMX_LOOKAHEAD_BITS];

int16_t vmx_mag_sign(uint64_t value);
int16_t vmx_shift_signed(int16_t value, int32_t shift);

#endif
