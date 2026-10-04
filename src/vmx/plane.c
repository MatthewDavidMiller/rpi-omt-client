/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * VMX_DecodePlaneInternal128.
 *
 * The reference kernel also carries a 16 KiB twelve-bit lookahead table that
 * resolves a value code, and any zero code following it, in one step, and
 * deliberately over-reads a trailing zero code into the next plane, which
 * REWINDOVERREAD then gives back. This port resolves one code per lookup
 * instead (vmx_ac_lookahead), and only when the whole code is already in the
 * window, so it never over-reads and needs no rewind. A code the table cannot
 * resolve, or one at the edge of the window, takes the bit-by-bit path, which
 * is the reference's manual path and the definition the table is held to.
 *
 * The reference truncates its unsigned bitstream intermediates into shorts,
 * which is the behaviour the casts below reproduce.
 */
#include "vmx/plane.h"

/* Upper bound on Golomb symbols read for one 8x8 block. Every iteration
 * consumes at least one bit, so this only ever fires on damaged input. */
#define MAX_SYMBOLS_PER_BLOCK 4096u

/* GetIntFrom2MagSign(value - 1) in the reference codec's unsigned 64-bit
 * arithmetic, truncated to a short. */
int16_t vmx_mag_sign(uint64_t value) {
    uint64_t input = value - 1u;
    uint64_t parity = input & 1u;
    uint64_t adjusted = input + parity;
    uint64_t result = (adjusted >> 1) - adjusted * parity;
    return (int16_t)(uint16_t)result;
}

/* VMX_ShiftSignedShort. */
int16_t vmx_shift_signed(int16_t value, int32_t shift) {
    if (shift <= 0) return value;
    if (shift >= 16) return 0;
    return (int16_t)(uint16_t)((uint32_t)(uint16_t)value << shift);
}

bool vmx_decode_plane(vmx_slice_streams *s, size_t stride, int16_t bias, const uint16_t matrix[64],
                      int32_t dc_shift, uint8_t *dst, size_t dst_len) {
    if (stride == 0 || stride % 8 != 0 || dst_len / VMX_SLICE_HEIGHT < stride) return false;

    int16_t block[64];
    uint32_t pending = 0;
    int16_t dc_prediction = 0;

    for (size_t row = 0; row < VMX_SLICE_HEIGHT; row += 8) {
        for (size_t column = 0; column < stride; column += 8) {
            memset(block, 0, sizeof(block));
            bool decoded_terms = pending < 64;
            uint32_t guard = 0;
            while (pending < 64) {
                if (++guard > MAX_SYMBOLS_PER_BLOCK) return false;
                /* With at least VMX_LOOKAHEAD_BITS unread, a code the table
                 * resolves is wholly inside the window, so consuming its
                 * length leaves the reader exactly where the bit-by-bit path
                 * would, and none of that path's corrupt cases can arise. */
                uint32_t entry = 0;
                if (s->ac.bits_left >= VMX_LOOKAHEAD_BITS)
                    entry = vmx_ac_lookahead[vmx_shl64(s->ac.window,
                                                       (uint32_t)(64 - s->ac.bits_left)) >>
                                             (64 - VMX_LOOKAHEAD_BITS)];
                if (entry != 0) {
                    s->ac.bits_left -= (int32_t)((entry >> 16) & 0xFFu);
                    if (entry >> 24 == VMX_LOOKAHEAD_VALUE)
                        block[vmx_zigzag_natural[pending++]] = (int16_t)(uint16_t)entry;
                    else
                        pending += entry & 0xFFFFu;
                } else if (vmx_bits_bit_bare(&s->ac) == 1) {
                    if (vmx_bits_bit_bare(&s->ac) == 1) {
                        pending += 1;
                    } else {
                        int32_t width = vmx_bits_zeros_bare(&s->ac) + 2;
                        uint64_t run = vmx_bits_bits_bare(&s->ac, width);
                        uint64_t next = (uint64_t)pending + run;
                        pending = next > UINT32_MAX ? UINT32_MAX : (uint32_t)next;
                    }
                } else {
                    int32_t width = vmx_bits_zeros_bare(&s->ac) + 2;
                    uint64_t value = vmx_bits_bits_bare(&s->ac, width);
                    if (pending < 64) block[vmx_zigzag_natural[pending]] = vmx_mag_sign(value);
                    pending += 1;
                }
                vmx_bits_reload(&s->ac);
                if (s->ac.corrupt) return false;
            }
            pending -= 64;

            if (vmx_bits_bit(&s->dc) == 1) {
                (void)vmx_bits_bit(&s->dc);
            } else {
                int32_t width = vmx_bits_zeros(&s->dc) + 2;
                uint64_t value = vmx_bits_bits(&s->dc, width);
                block[0] = vmx_shift_signed(vmx_mag_sign(value), dc_shift);
            }
            if (s->dc.corrupt) return false;

            block[0] = vmx_wrap_add16(block[0], dc_prediction);
            dc_prediction = block[0];

            uint8_t *target = dst + row * stride + column;
            if (decoded_terms)
                vmx_idct(block, matrix, target, stride, bias);
            else
                vmx_broadcast_dc(block[0], target, stride, bias);
        }
    }

    vmx_bits_align(&s->dc);
    vmx_bits_align(&s->ac);
    return !(s->dc.corrupt || s->ac.corrupt);
}
