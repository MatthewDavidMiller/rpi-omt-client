/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Blowfish with the expensive key schedule (EksBlowfish) that bcrypt_pbkdf
 * builds on. Only what bcrypt_pbkdf needs: no modes, no decryption.
 */
#ifndef DP_BLOWFISH_H
#define DP_BLOWFISH_H

#include "common/base.h"

typedef struct {
    uint32_t s[4][256];
    uint32_t p[18];
} ssh_blowfish;

extern const uint32_t ssh_blowfish_init_p[18];
extern const uint32_t ssh_blowfish_init_s[4][256];

void ssh_blowfish_initstate(ssh_blowfish *c);
void ssh_blowfish_expand0state(ssh_blowfish *c, const uint8_t *key, size_t key_len);
void ssh_blowfish_expandstate(ssh_blowfish *c, const uint8_t *data, size_t data_len,
                              const uint8_t *key, size_t key_len);
/* Encrypts `blocks` 64-bit blocks held as pairs of words, in place. */
void ssh_blowfish_enc(const ssh_blowfish *c, uint32_t *data, size_t blocks);

#endif
