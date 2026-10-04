/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The packet ciphers: chacha20-poly1305@openssh.com, AES-GCM (RFC 5647 as
 * OpenSSH implements it), and AES-CTR with an HMAC-SHA2 tag, either over the
 * plaintext (RFC 6668) or over the ciphertext (the -etm@openssh.com MACs).
 * There is no CBC, no SHA-1, and no unauthenticated mode.
 */
#include <openssl/core_names.h>
#include <openssl/crypto.h>
#include <openssl/params.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

static const ssh_cipher_alg CIPHERS[] = {
    {"chacha20-poly1305@openssh.com", SSH_CIPHER_CHACHA, 64, 0, 8},
    {"aes256-gcm@openssh.com", SSH_CIPHER_GCM, 32, 12, 16},
    {"aes128-gcm@openssh.com", SSH_CIPHER_GCM, 16, 12, 16},
    {"aes256-ctr", SSH_CIPHER_CTR, 32, 16, 16},
    {"aes192-ctr", SSH_CIPHER_CTR, 24, 16, 16},
    {"aes128-ctr", SSH_CIPHER_CTR, 16, 16, 16},
};

const char SSH_CIPHER_LIST[] = "chacha20-poly1305@openssh.com,aes256-gcm@openssh.com,"
                               "aes128-gcm@openssh.com,aes256-ctr,aes192-ctr,aes128-ctr";

static const ssh_mac_alg MACS[] = {
    {"hmac-sha2-512-etm@openssh.com", "SHA512", 64, 64, true},
    {"hmac-sha2-256-etm@openssh.com", "SHA256", 32, 32, true},
    {"hmac-sha2-512", "SHA512", 64, 64, false},
    {"hmac-sha2-256", "SHA256", 32, 32, false},
};

const char SSH_MAC_LIST[] = "hmac-sha2-512-etm@openssh.com,hmac-sha2-256-etm@openssh.com,"
                            "hmac-sha2-512,hmac-sha2-256";

const ssh_cipher_alg *ssh_cipher_find(omt_span name) {
    for (size_t i = 0; i < OMT_ARRAY_LEN(CIPHERS); i++) {
        if (span_is(name, CIPHERS[i].name)) return &CIPHERS[i];
    }
    return NULL;
}

const ssh_mac_alg *ssh_mac_find(omt_span name) {
    for (size_t i = 0; i < OMT_ARRAY_LEN(MACS); i++) {
        if (span_is(name, MACS[i].name)) return &MACS[i];
    }
    return NULL;
}

static const EVP_CIPHER *ctr_cipher(size_t key_len) {
    switch (key_len) {
    case 16: return EVP_aes_128_ctr();
    case 24: return EVP_aes_192_ctr();
    default: return EVP_aes_256_ctr();
    }
}

bool ssh_cipher_init(ssh_cipher *c, const ssh_cipher_alg *alg, const ssh_mac_alg *mac,
                     const uint8_t *key, const uint8_t *iv, const uint8_t *mac_key, bool encrypt) {
    ssh_cipher_free(c);
    c->alg = alg;
    c->mac = alg->kind == SSH_CIPHER_CTR ? mac : NULL;
    memcpy(c->key, key, alg->key_len);
    if (alg->iv_len) memcpy(c->iv, iv, alg->iv_len);
    if (c->mac) memcpy(c->mac_key, mac_key, c->mac->key_len);
    c->ctx = EVP_CIPHER_CTX_new();
    if (!c->ctx) return false;
    switch (alg->kind) {
    case SSH_CIPHER_CTR:
        if (!c->mac) return false;
        c->mac_impl = EVP_MAC_fetch(NULL, "HMAC", NULL);
        return c->mac_impl && EVP_CipherInit_ex(c->ctx, ctr_cipher(alg->key_len), NULL, c->key,
                                                c->iv, encrypt ? 1 : 0) == 1;
    case SSH_CIPHER_GCM: {
        const EVP_CIPHER *gcm = alg->key_len == 16 ? EVP_aes_128_gcm() : EVP_aes_256_gcm();
        return EVP_CipherInit_ex(c->ctx, gcm, NULL, NULL, NULL, encrypt ? 1 : 0) == 1 &&
               EVP_CIPHER_CTX_ctrl(c->ctx, EVP_CTRL_GCM_SET_IVLEN, 12, NULL) == 1 &&
               EVP_CipherInit_ex(c->ctx, NULL, NULL, c->key, NULL, -1) == 1;
    }
    case SSH_CIPHER_CHACHA:
        c->mac_impl = EVP_MAC_fetch(NULL, "POLY1305", NULL);
        return c->mac_impl != NULL;
    case SSH_CIPHER_NONE: break;
    }
    return false;
}

void ssh_cipher_free(ssh_cipher *c) {
    EVP_CIPHER_CTX_free(c->ctx);
    EVP_MAC_free(c->mac_impl);
    OPENSSL_cleanse(c, sizeof(*c));
    c->ctx = NULL;
    c->mac_impl = NULL;
    c->alg = NULL;
    c->mac = NULL;
}

size_t ssh_cipher_block(const ssh_cipher *c) {
    return c->alg && c->alg->block > 8 ? c->alg->block : 8;
}

size_t ssh_cipher_tag_len(const ssh_cipher *c) {
    if (!c->alg) return 0;
    if (c->alg->kind == SSH_CIPHER_CTR) return c->mac->mac_len;
    return 16;
}

bool ssh_cipher_aead(const ssh_cipher *c) {
    return c->alg && (c->alg->kind != SSH_CIPHER_CTR || c->mac->etm);
}

size_t ssh_cipher_first_len(const ssh_cipher *c) {
    if (c->alg && c->alg->kind == SSH_CIPHER_CTR && !c->mac->etm) return c->alg->block;
    return 4;
}

/* HMAC over seq || data, into out[mac_len]. */
static bool hmac(ssh_cipher *c, uint32_t seq, const uint8_t *data, size_t len, uint8_t *out) {
    EVP_MAC_CTX *ctx = EVP_MAC_CTX_new(c->mac_impl);
    if (!ctx) return false;
    char digest[16];
    omt_strlcpy(digest, c->mac->digest, sizeof(digest));
    OSSL_PARAM params[] = {OSSL_PARAM_utf8_string(OSSL_MAC_PARAM_DIGEST, digest, 0),
                           OSSL_PARAM_END};
    uint8_t seqbuf[4];
    omt_put_be32(seqbuf, seq);
    size_t out_len = 0;
    bool ok = EVP_MAC_init(ctx, c->mac_key, c->mac->key_len, params) == 1 &&
              EVP_MAC_update(ctx, seqbuf, 4) == 1 && EVP_MAC_update(ctx, data, len) == 1 &&
              EVP_MAC_final(ctx, out, &out_len, c->mac->mac_len) == 1 && out_len == c->mac->mac_len;
    EVP_MAC_CTX_free(ctx);
    return ok;
}

/* One ChaCha20 operation in OpenSSH's original-construction layout: a 64-bit
 * block counter followed by the 64-bit big-endian sequence number. OpenSSL's
 * 16-byte IV is a 32-bit little-endian counter and a 96-bit nonce, so the
 * counter's high word becomes the nonce's first four bytes. */
static bool chacha(const uint8_t key[32], uint32_t seq, uint32_t counter, const uint8_t *in,
                   uint8_t *out, size_t len) {
    uint8_t iv[16] = {0};
    omt_put_le32(iv, counter);
    omt_put_be32(iv + 12, seq); /* the sequence number's high word is zero */
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    if (!ctx) return false;
    int out_len = 0;
    bool ok = EVP_EncryptInit_ex(ctx, EVP_chacha20(), NULL, key, iv) == 1 &&
              (len == 0 || EVP_EncryptUpdate(ctx, out, &out_len, in, (int)len) == 1);
    EVP_CIPHER_CTX_free(ctx);
    return ok;
}

static bool poly1305(ssh_cipher *c, uint32_t seq, const uint8_t *data, size_t len,
                     uint8_t tag[16]) {
    uint8_t zero[64] = {0}, block[64];
    if (!chacha(c->key, seq, 0, zero, block, sizeof(block))) return false;
    EVP_MAC_CTX *ctx = EVP_MAC_CTX_new(c->mac_impl);
    size_t out_len = 0;
    bool ok = ctx && EVP_MAC_init(ctx, block, 32, NULL) == 1 &&
              EVP_MAC_update(ctx, data, len) == 1 && EVP_MAC_final(ctx, tag, &out_len, 16) == 1 &&
              out_len == 16;
    EVP_MAC_CTX_free(ctx);
    OPENSSL_cleanse(block, sizeof(block));
    return ok;
}

static void gcm_next_iv(uint8_t iv[12]) {
    for (int i = 11; i >= 4; i--) {
        if (++iv[i] != 0) break;
    }
}

static bool crypt_ctx(EVP_CIPHER_CTX *ctx, uint8_t *data, size_t len) {
    while (len > 0) {
        int chunk = len > (1u << 30) ? (1 << 30) : (int)len;
        int out_len = 0;
        if (EVP_CipherUpdate(ctx, data, &out_len, data, chunk) != 1 || out_len != chunk)
            return false;
        data += chunk;
        len -= (size_t)chunk;
    }
    return true;
}

bool ssh_cipher_seal(ssh_cipher *c, uint32_t seq, uint8_t *packet, size_t len, uint8_t *tag) {
    if (!c->alg) return true;
    switch (c->alg->kind) {
    case SSH_CIPHER_CTR:
        if (c->mac->etm) {
            return crypt_ctx(c->ctx, packet + 4, len - 4) && hmac(c, seq, packet, len, tag);
        }
        return hmac(c, seq, packet, len, tag) && crypt_ctx(c->ctx, packet, len);
    case SSH_CIPHER_GCM: {
        int out_len = 0;
        uint8_t final[16];
        bool ok = EVP_EncryptInit_ex(c->ctx, NULL, NULL, NULL, c->iv) == 1 &&
                  EVP_EncryptUpdate(c->ctx, NULL, &out_len, packet, 4) == 1 &&
                  crypt_ctx(c->ctx, packet + 4, len - 4) &&
                  EVP_EncryptFinal_ex(c->ctx, final, &out_len) == 1 &&
                  EVP_CIPHER_CTX_ctrl(c->ctx, EVP_CTRL_GCM_GET_TAG, 16, tag) == 1;
        gcm_next_iv(c->iv);
        return ok;
    }
    case SSH_CIPHER_CHACHA:
        return chacha(c->key + 32, seq, 0, packet, packet, 4) &&
               chacha(c->key, seq, 1, packet + 4, packet + 4, len - 4) &&
               poly1305(c, seq, packet, len, tag);
    case SSH_CIPHER_NONE: break;
    }
    return false;
}

bool ssh_cipher_length(ssh_cipher *c, uint32_t seq, uint8_t *first, uint32_t *len) {
    if (c->alg && c->alg->kind == SSH_CIPHER_CTR && !c->mac->etm) {
        if (!crypt_ctx(c->ctx, first, c->alg->block)) return false;
    } else if (c->alg && c->alg->kind == SSH_CIPHER_CHACHA) {
        uint8_t plain[4];
        if (!chacha(c->key + 32, seq, 0, first, plain, 4)) return false;
        *len = omt_be32(plain);
        return true;
    }
    *len = omt_be32(first);
    return true;
}

bool ssh_cipher_open(ssh_cipher *c, uint32_t seq, uint8_t *packet, size_t len, const uint8_t *tag) {
    if (!c->alg) return true;
    switch (c->alg->kind) {
    case SSH_CIPHER_CTR: {
        uint8_t expect[64];
        if (c->mac->etm) {
            return hmac(c, seq, packet, len, expect) &&
                   CRYPTO_memcmp(expect, tag, c->mac->mac_len) == 0 &&
                   crypt_ctx(c->ctx, packet + 4, len - 4);
        }
        /* The first block was decrypted to read the length. */
        size_t block = c->alg->block;
        return crypt_ctx(c->ctx, packet + block, len - block) &&
               hmac(c, seq, packet, len, expect) &&
               CRYPTO_memcmp(expect, tag, c->mac->mac_len) == 0;
    }
    case SSH_CIPHER_GCM: {
        int out_len = 0;
        uint8_t tag_copy[16], final[16];
        memcpy(tag_copy, tag, 16);
        bool ok = EVP_DecryptInit_ex(c->ctx, NULL, NULL, NULL, c->iv) == 1 &&
                  EVP_DecryptUpdate(c->ctx, NULL, &out_len, packet, 4) == 1 &&
                  crypt_ctx(c->ctx, packet + 4, len - 4) &&
                  EVP_CIPHER_CTX_ctrl(c->ctx, EVP_CTRL_GCM_SET_TAG, 16, tag_copy) == 1 &&
                  EVP_DecryptFinal_ex(c->ctx, final, &out_len) == 1;
        gcm_next_iv(c->iv);
        return ok;
    }
    case SSH_CIPHER_CHACHA: {
        uint8_t expect[16];
        if (!poly1305(c, seq, packet, len, expect) || CRYPTO_memcmp(expect, tag, 16) != 0) {
            return false;
        }
        uint8_t plain[4];
        return chacha(c->key + 32, seq, 0, packet, plain, 4) &&
               (memcpy(packet, plain, 4), chacha(c->key, seq, 1, packet + 4, packet + 4, len - 4));
    }
    case SSH_CIPHER_NONE: break;
    }
    return false;
}
