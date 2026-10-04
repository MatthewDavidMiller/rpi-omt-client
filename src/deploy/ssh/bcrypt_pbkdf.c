/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * bcrypt_pbkdf, the key derivation OpenSSH uses for passphrase-protected
 * private keys, and the Blowfish it is built from. Both follow OpenBSD's
 * definitions (blf.c and bcrypt_pbkdf.c) exactly, because the output has to be
 * bit-identical to what ssh-keygen derived when it encrypted the key.
 */
#include <openssl/crypto.h>
#include <openssl/evp.h>
#include <string.h>

#include "deploy/ssh/blowfish.h"
#include "deploy/ssh/ssh_internal.h"

static uint32_t feistel(const ssh_blowfish *c, uint32_t x) {
    return ((c->s[0][x >> 24] + c->s[1][(x >> 16) & 0xff]) ^ c->s[2][(x >> 8) & 0xff]) +
           c->s[3][x & 0xff];
}

static void encipher(const ssh_blowfish *c, uint32_t *xl, uint32_t *xr) {
    uint32_t l = *xl, r = *xr;
    l ^= c->p[0];
    for (int i = 1; i <= 16; i += 2) {
        r ^= feistel(c, l) ^ c->p[i];
        l ^= feistel(c, r) ^ c->p[i + 1];
    }
    *xl = r ^ c->p[17];
    *xr = l;
}

/* Four bytes of a cyclically repeated stream, big-endian. */
static uint32_t stream2word(const uint8_t *data, size_t len, size_t *j) {
    uint32_t word = 0;
    for (int i = 0; i < 4; i++) {
        if (*j >= len) *j = 0;
        word = (word << 8) | data[*j];
        (*j)++;
    }
    return word;
}

void ssh_blowfish_initstate(ssh_blowfish *c) {
    memcpy(c->p, ssh_blowfish_init_p, sizeof(c->p));
    memcpy(c->s, ssh_blowfish_init_s, sizeof(c->s));
}

void ssh_blowfish_expand0state(ssh_blowfish *c, const uint8_t *key, size_t key_len) {
    size_t j = 0;
    for (int i = 0; i < 18; i++) c->p[i] ^= stream2word(key, key_len, &j);
    uint32_t l = 0, r = 0;
    for (int i = 0; i < 18; i += 2) {
        encipher(c, &l, &r);
        c->p[i] = l;
        c->p[i + 1] = r;
    }
    for (int i = 0; i < 4; i++) {
        for (int k = 0; k < 256; k += 2) {
            encipher(c, &l, &r);
            c->s[i][k] = l;
            c->s[i][k + 1] = r;
        }
    }
}

void ssh_blowfish_expandstate(ssh_blowfish *c, const uint8_t *data, size_t data_len,
                              const uint8_t *key, size_t key_len) {
    size_t j = 0;
    for (int i = 0; i < 18; i++) c->p[i] ^= stream2word(key, key_len, &j);
    j = 0;
    uint32_t l = 0, r = 0;
    for (int i = 0; i < 18; i += 2) {
        l ^= stream2word(data, data_len, &j);
        r ^= stream2word(data, data_len, &j);
        encipher(c, &l, &r);
        c->p[i] = l;
        c->p[i + 1] = r;
    }
    for (int i = 0; i < 4; i++) {
        for (int k = 0; k < 256; k += 2) {
            l ^= stream2word(data, data_len, &j);
            r ^= stream2word(data, data_len, &j);
            encipher(c, &l, &r);
            c->s[i][k] = l;
            c->s[i][k + 1] = r;
        }
    }
}

void ssh_blowfish_enc(const ssh_blowfish *c, uint32_t *data, size_t blocks) {
    for (size_t i = 0; i < blocks; i++) encipher(c, &data[2 * i], &data[2 * i + 1]);
}

#define BCRYPT_WORDS 8
#define BCRYPT_HASHSIZE (BCRYPT_WORDS * 4)

static void bcrypt_hash(const uint8_t sha2pass[64], const uint8_t sha2salt[64],
                        uint8_t out[BCRYPT_HASHSIZE]) {
    static const char magic[BCRYPT_HASHSIZE + 1] = "OxychromaticBlowfishSwatDynamite";
    ssh_blowfish state;
    ssh_blowfish_initstate(&state);
    ssh_blowfish_expandstate(&state, sha2salt, 64, sha2pass, 64);
    for (int i = 0; i < 64; i++) {
        ssh_blowfish_expand0state(&state, sha2salt, 64);
        ssh_blowfish_expand0state(&state, sha2pass, 64);
    }
    uint32_t cdata[BCRYPT_WORDS];
    size_t j = 0;
    for (int i = 0; i < BCRYPT_WORDS; i++)
        cdata[i] = stream2word((const uint8_t *)magic, BCRYPT_HASHSIZE, &j);
    for (int i = 0; i < 64; i++) ssh_blowfish_enc(&state, cdata, BCRYPT_WORDS / 2);
    for (int i = 0; i < BCRYPT_WORDS; i++) omt_put_le32(out + 4 * i, cdata[i]);
    OPENSSL_cleanse(cdata, sizeof(cdata));
    OPENSSL_cleanse(&state, sizeof(state));
}

static bool sha512(const uint8_t *a, size_t a_len, const uint8_t *b, size_t b_len,
                   uint8_t out[64]) {
    EVP_MD_CTX *md = EVP_MD_CTX_new();
    unsigned int n = 0;
    bool ok = md && EVP_DigestInit_ex2(md, EVP_sha512(), NULL) == 1 &&
              EVP_DigestUpdate(md, a, a_len) == 1 &&
              (b_len == 0 || EVP_DigestUpdate(md, b, b_len) == 1) &&
              EVP_DigestFinal_ex(md, out, &n) == 1 && n == 64;
    EVP_MD_CTX_free(md);
    return ok;
}

bool ssh_bcrypt_pbkdf(const uint8_t *pass, size_t pass_len, const uint8_t *salt, size_t salt_len,
                      uint8_t *key, size_t key_len, uint32_t rounds) {
    if (rounds < 1 || pass_len == 0 || salt_len == 0 || key_len == 0 ||
        key_len > BCRYPT_HASHSIZE * BCRYPT_HASHSIZE || salt_len > (1u << 20)) {
        return false;
    }
    uint8_t sha2pass[64], sha2salt[64], out[BCRYPT_HASHSIZE], tmp[BCRYPT_HASHSIZE];
    size_t stride = (key_len + sizeof(out) - 1) / sizeof(out);
    size_t amt = (key_len + stride - 1) / stride;
    size_t remaining = key_len;
    bool ok = sha512(pass, pass_len, NULL, 0, sha2pass);
    for (uint32_t count = 1; ok && remaining > 0; count++) {
        uint8_t countsalt[4];
        omt_put_be32(countsalt, count);
        ok = sha512(salt, salt_len, countsalt, 4, sha2salt);
        if (!ok) break;
        bcrypt_hash(sha2pass, sha2salt, tmp);
        memcpy(out, tmp, sizeof(out));
        for (uint32_t r = 1; ok && r < rounds; r++) {
            ok = sha512(tmp, sizeof(tmp), NULL, 0, sha2salt);
            bcrypt_hash(sha2pass, sha2salt, tmp);
            for (size_t j = 0; j < sizeof(out); j++) out[j] ^= tmp[j];
        }
        /* Output bytes are interleaved across the blocks, so each block
         * contributes to the whole key rather than to one stretch of it. */
        amt = amt < remaining ? amt : remaining;
        size_t i;
        for (i = 0; i < amt; i++) {
            size_t dest = i * stride + (count - 1);
            if (dest >= key_len) break;
            key[dest] = out[i];
        }
        remaining -= i;
    }
    OPENSSL_cleanse(sha2pass, sizeof(sha2pass));
    OPENSSL_cleanse(sha2salt, sizeof(sha2salt));
    OPENSSL_cleanse(out, sizeof(out));
    OPENSSL_cleanse(tmp, sizeof(tmp));
    return ok;
}
