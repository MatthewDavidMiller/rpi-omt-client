/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * User private keys: OpenSSH's own format (openssh-key-v1, PROTOCOL.key),
 * plain or passphrase-protected with bcrypt_pbkdf, and the PEM formats
 * (PKCS#1, SEC1, PKCS#8) through OpenSSL's decoder. Supported types are
 * Ed25519, ECDSA on the NIST curves, and RSA of at least 2048 bits.
 */
#include <openssl/bio.h>
#include <openssl/core_names.h>
#include <openssl/ec.h>
#include <openssl/param_build.h>
#include <openssl/params.h>
#include <openssl/pem.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

#define KEY_FILE_LIMIT (1024u * 1024u)
#define BEGIN_OPENSSH "-----BEGIN OPENSSH PRIVATE KEY-----"
#define END_OPENSSH "-----END OPENSSH PRIVATE KEY-----"
#define AUTH_MAGIC "openssh-key-v1"

typedef struct {
    const char *curve_id;
    const char *group;
    const char *digest;
} curve_info;

static const curve_info CURVES[] = {
    {"nistp256", "P-256", "SHA256"},
    {"nistp384", "P-384", "SHA384"},
    {"nistp521", "P-521", "SHA512"},
};

void ssh_privkey_free(ssh_privkey *k) {
    EVP_PKEY_free(k->pkey);
    k->pkey = NULL;
    omt_buf_free(&k->blob);
}

static EVP_PKEY *from_params(const char *name, OSSL_PARAM_BLD *bld) {
    OSSL_PARAM *params = OSSL_PARAM_BLD_to_param(bld);
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_name(NULL, name, NULL);
    EVP_PKEY *pkey = NULL;
    if (params && ctx && EVP_PKEY_fromdata_init(ctx) == 1 &&
        EVP_PKEY_fromdata(ctx, &pkey, EVP_PKEY_KEYPAIR, params) != 1) {
        pkey = NULL;
    }
    EVP_PKEY_CTX_free(ctx);
    OSSL_PARAM_free(params);
    return pkey;
}

/* Fills type and public blob from a loaded EVP_PKEY. */
static bool describe(ssh_privkey *k, dp_err *err) {
    omt_buf_init(&k->blob, 4096);
    int id = EVP_PKEY_get_base_id(k->pkey);
    if (id == EVP_PKEY_ED25519) {
        uint8_t pk[32];
        size_t len = sizeof(pk);
        if (EVP_PKEY_get_raw_public_key(k->pkey, pk, &len) != 1 || len != 32) goto bad;
        omt_strlcpy(k->type, "ssh-ed25519", sizeof(k->type));
        sb_cstring(&k->blob, k->type);
        sb_string(&k->blob, pk, 32);
        return !k->blob.failed;
    }
    if (id == EVP_PKEY_EC) {
        char group[32];
        size_t glen = 0;
        if (EVP_PKEY_get_utf8_string_param(k->pkey, OSSL_PKEY_PARAM_GROUP_NAME, group,
                                           sizeof(group), &glen) != 1) {
            goto bad;
        }
        const curve_info *curve = NULL;
        for (size_t i = 0; i < OMT_ARRAY_LEN(CURVES); i++) {
            /* OpenSSL may report a P-256 key as prime256v1 or secp384r1. */
            int nid = OBJ_txt2nid(group);
            if (strcmp(group, CURVES[i].group) == 0 || nid == EC_curve_nist2nid(CURVES[i].group)) {
                curve = &CURVES[i];
            }
        }
        if (!curve) {
            dp_fail(err, "unsupported ECDSA curve %s", group);
            return false;
        }
        EVP_PKEY_set_utf8_string_param(k->pkey, OSSL_PKEY_PARAM_EC_POINT_CONVERSION_FORMAT,
                                       "uncompressed");
        uint8_t q[200];
        size_t qlen = 0;
        if (EVP_PKEY_get_octet_string_param(k->pkey, OSSL_PKEY_PARAM_ENCODED_PUBLIC_KEY, q,
                                            sizeof(q), &qlen) != 1 ||
            qlen == 0 || q[0] != 0x04) {
            goto bad;
        }
        omt_snprintf(k->type, sizeof(k->type), "ecdsa-sha2-%s", curve->curve_id);
        k->ec_digest = curve->digest;
        sb_cstring(&k->blob, k->type);
        sb_cstring(&k->blob, curve->curve_id);
        sb_string(&k->blob, q, qlen);
        return !k->blob.failed;
    }
    if (id == EVP_PKEY_RSA) {
        BIGNUM *n = NULL, *e = NULL;
        bool ok = EVP_PKEY_get_bn_param(k->pkey, OSSL_PKEY_PARAM_RSA_N, &n) == 1 &&
                  EVP_PKEY_get_bn_param(k->pkey, OSSL_PKEY_PARAM_RSA_E, &e) == 1;
        if (ok && BN_num_bits(n) < 2048) {
            BN_free(n);
            BN_free(e);
            dp_fail(err, "RSA keys must have at least 2048 bits");
            return false;
        }
        omt_strlcpy(k->type, "ssh-rsa", sizeof(k->type));
        sb_cstring(&k->blob, k->type);
        ok = ok && sb_mpint_bn(&k->blob, e) && sb_mpint_bn(&k->blob, n);
        BN_free(n);
        BN_free(e);
        if (!ok) goto bad;
        return true;
    }
    dp_fail(err, "unsupported private key type");
    return false;
bad:
    dp_fail(err, "the private key could not be read");
    return false;
}

/* ------------------------------------------------------- openssh-key-v1 */

typedef struct {
    const char *name;
    const char *evp;
    size_t key_len;
    size_t iv_len;
    size_t block;
    size_t tag_len;
} key_cipher;

static const key_cipher KEY_CIPHERS[] = {
    {"none", NULL, 0, 0, 8, 0},
    {"aes128-ctr", "AES-128-CTR", 16, 16, 16, 0},
    {"aes192-ctr", "AES-192-CTR", 24, 16, 16, 0},
    {"aes256-ctr", "AES-256-CTR", 32, 16, 16, 0},
    {"aes128-cbc", "AES-128-CBC", 16, 16, 16, 0},
    {"aes192-cbc", "AES-192-CBC", 24, 16, 16, 0},
    {"aes256-cbc", "AES-256-CBC", 32, 16, 16, 0},
    {"aes128-gcm@openssh.com", "AES-128-GCM", 16, 12, 16, 16},
    {"aes256-gcm@openssh.com", "AES-256-GCM", 32, 12, 16, 16},
};

static bool decrypt_section(const key_cipher *cipher, const uint8_t *km, const uint8_t *data,
                            size_t len, const uint8_t *tag, uint8_t *out) {
    EVP_CIPHER *evp = EVP_CIPHER_fetch(NULL, cipher->evp, NULL);
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    int out_len = 0, final_len = 0;
    bool ok = evp && ctx && EVP_DecryptInit_ex2(ctx, evp, NULL, NULL, NULL) == 1;
    if (ok && cipher->tag_len) {
        ok = EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_GCM_SET_IVLEN, (int)cipher->iv_len, NULL) == 1;
    }
    ok = ok && EVP_DecryptInit_ex2(ctx, NULL, km, km + cipher->key_len, NULL) == 1 &&
         EVP_CIPHER_CTX_set_padding(ctx, 0) == 1 &&
         EVP_DecryptUpdate(ctx, out, &out_len, data, (int)len) == 1;
    if (ok && cipher->tag_len) {
        uint8_t tag_copy[16];
        memcpy(tag_copy, tag, 16);
        ok = EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_GCM_SET_TAG, 16, tag_copy) == 1;
    }
    ok = ok && EVP_DecryptFinal_ex(ctx, out + out_len, &final_len) == 1;
    EVP_CIPHER_CTX_free(ctx);
    EVP_CIPHER_free(evp);
    return ok;
}

static bool load_private_fields(omt_span *r, ssh_privkey *k, dp_err *err) {
    omt_span type;
    if (!sr_string(r, &type)) goto bad;
    if (span_is(type, "ssh-ed25519")) {
        omt_span pk, sk;
        if (!sr_string(r, &pk) || !sr_string(r, &sk) || pk.len != 32 || sk.len != 64 ||
            memcmp(sk.p + 32, pk.p, 32) != 0) {
            goto bad;
        }
        k->pkey = EVP_PKEY_new_raw_private_key(EVP_PKEY_ED25519, NULL, sk.p, 32);
        return k->pkey != NULL || (dp_fail(err, "the private key could not be read"), false);
    }
    for (size_t i = 0; i < OMT_ARRAY_LEN(CURVES); i++) {
        char name[40];
        omt_snprintf(name, sizeof(name), "ecdsa-sha2-%s", CURVES[i].curve_id);
        if (!span_is(type, name)) continue;
        omt_span curve, q;
        BIGNUM *d = NULL;
        if (!sr_string(r, &curve) || !span_is(curve, CURVES[i].curve_id) || !sr_string(r, &q) ||
            !sr_mpint_bn(r, &d)) {
            goto bad;
        }
        OSSL_PARAM_BLD *bld = OSSL_PARAM_BLD_new();
        if (bld) {
            OSSL_PARAM_BLD_push_utf8_string(bld, OSSL_PKEY_PARAM_GROUP_NAME, CURVES[i].group, 0);
            OSSL_PARAM_BLD_push_octet_string(bld, OSSL_PKEY_PARAM_PUB_KEY, q.p, q.len);
            OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_PRIV_KEY, d);
            k->pkey = from_params("EC", bld);
            OSSL_PARAM_BLD_free(bld);
        }
        BN_clear_free(d);
        if (!k->pkey) goto bad;
        return true;
    }
    if (span_is(type, "ssh-rsa")) {
        BIGNUM *n = NULL, *e = NULL, *d = NULL, *iqmp = NULL, *p = NULL, *q = NULL;
        BIGNUM *dmp1 = BN_secure_new(), *dmq1 = BN_secure_new(), *t = BN_secure_new();
        BN_CTX *bn = BN_CTX_new();
        bool ok = dmp1 && dmq1 && t && bn && sr_mpint_bn(r, &n) && sr_mpint_bn(r, &e) &&
                  sr_mpint_bn(r, &d) && sr_mpint_bn(r, &iqmp) && sr_mpint_bn(r, &p) &&
                  sr_mpint_bn(r, &q);
        /* The CRT exponents are not stored; derive them. */
        ok = ok && BN_sub(t, p, BN_value_one()) == 1 && BN_mod(dmp1, d, t, bn) == 1 &&
             BN_sub(t, q, BN_value_one()) == 1 && BN_mod(dmq1, d, t, bn) == 1;
        if (ok) {
            OSSL_PARAM_BLD *bld = OSSL_PARAM_BLD_new();
            if (bld) {
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_N, n);
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_E, e);
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_D, d);
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_FACTOR1, p);
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_FACTOR2, q);
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_EXPONENT1, dmp1);
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_EXPONENT2, dmq1);
                OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_COEFFICIENT1, iqmp);
                k->pkey = from_params("RSA", bld);
                OSSL_PARAM_BLD_free(bld);
            }
        }
        BN_free(n);
        BN_free(e);
        BN_clear_free(d);
        BN_clear_free(iqmp);
        BN_clear_free(p);
        BN_clear_free(q);
        BN_clear_free(dmp1);
        BN_clear_free(dmq1);
        BN_clear_free(t);
        BN_CTX_free(bn);
        if (!k->pkey) goto bad;
        return true;
    }
    dp_fail(err, "unsupported private key type %.*s", (int)omt_min_size(type.len, 64),
            (const char *)type.p);
    return false;
bad:
    dp_fail(err, "the private key could not be read");
    return false;
}

static bool parse_openssh(const uint8_t *data, size_t len, const char *passphrase,
                          size_t passphrase_len, ssh_privkey *k, dp_err *err) {
    const char *text = (const char *)data;
    const char *begin = strstr(text, BEGIN_OPENSSH);
    const char *end = begin ? strstr(begin, END_OPENSSH) : NULL;
    (void)len;
    omt_buf raw, plain;
    omt_buf_init(&raw, KEY_FILE_LIMIT);
    omt_buf_init(&plain, KEY_FILE_LIMIT);
    bool ok = false;
    if (!end) goto bad;
    const char *body = begin + strlen(BEGIN_OPENSSH);
    if (!ssh_base64_decode(body, (size_t)(end - body), true, &raw)) goto bad;
    omt_span r = omt_span_of(raw.data, raw.len), cname, kdf, kdfopts, pub, enc;
    uint32_t nkeys;
    if (raw.len < sizeof(AUTH_MAGIC) || memcmp(raw.data, AUTH_MAGIC, sizeof(AUTH_MAGIC)) != 0 ||
        !omt_span_skip(&r, sizeof(AUTH_MAGIC)) || !sr_string(&r, &cname) || !sr_string(&r, &kdf) ||
        !sr_string(&r, &kdfopts) || !sr_u32(&r, &nkeys) || nkeys != 1 || !sr_string(&r, &pub) ||
        !sr_string(&r, &enc)) {
        goto bad;
    }
    const key_cipher *cipher = NULL;
    for (size_t i = 0; i < OMT_ARRAY_LEN(KEY_CIPHERS); i++) {
        if (span_is(cname, KEY_CIPHERS[i].name)) cipher = &KEY_CIPHERS[i];
    }
    if (!cipher) {
        dp_fail(err, "the private key is encrypted with an unsupported cipher (%.*s)",
                (int)omt_min_size(cname.len, 64), (const char *)cname.p);
        goto out;
    }
    if (r.len != cipher->tag_len || enc.len % cipher->block != 0) goto bad;
    if (!cipher->evp) {
        if (!span_is(kdf, "none")) goto bad;
        omt_buf_append(&plain, enc.p, enc.len);
    } else {
        if (!span_is(kdf, "bcrypt")) {
            dp_fail(err, "the private key uses an unsupported key derivation");
            goto out;
        }
        if (passphrase_len == 0) {
            dp_fail(err, "the private key is encrypted and no passphrase was given");
            goto out;
        }
        omt_span salt;
        uint32_t rounds;
        if (!sr_string(&kdfopts, &salt) || !sr_u32(&kdfopts, &rounds) || kdfopts.len != 0 ||
            rounds == 0 || rounds > 100000) {
            goto bad;
        }
        uint8_t km[64];
        if (!ssh_bcrypt_pbkdf((const uint8_t *)passphrase, passphrase_len, salt.p, salt.len, km,
                              cipher->key_len + cipher->iv_len, rounds) ||
            !omt_buf_reserve(&plain, enc.len + 16)) {
            OPENSSL_cleanse(km, sizeof(km));
            goto bad;
        }
        bool decrypted = decrypt_section(cipher, km, enc.p, enc.len, enc.p + enc.len, plain.data);
        OPENSSL_cleanse(km, sizeof(km));
        if (!decrypted) {
            dp_fail(err, "incorrect passphrase supplied to decrypt private key");
            goto out;
        }
        plain.len = enc.len;
    }
    omt_span s = omt_span_of(plain.data, plain.len), comment;
    uint32_t check1, check2;
    if (!sr_u32(&s, &check1) || !sr_u32(&s, &check2)) goto bad;
    if (check1 != check2) {
        dp_fail(err, "incorrect passphrase supplied to decrypt private key");
        goto out;
    }
    if (!load_private_fields(&s, k, err)) goto out;
    if (!sr_string(&s, &comment)) goto bad;
    for (size_t i = 0; i < s.len; i++) {
        if (s.p[i] != (uint8_t)(i + 1)) goto bad; /* deterministic padding */
    }
    ok = describe(k, err);
    if (ok && (k->blob.len != pub.len || memcmp(k->blob.data, pub.p, pub.len) != 0)) {
        dp_fail(err, "the private key does not match its public half");
        ok = false;
    }
    goto out;
bad:
    dp_fail(err, "the private key file is malformed");
out:
    omt_buf_free_secret(&raw);
    omt_buf_free_secret(&plain);
    return ok;
}

/* ------------------------------------------------------------------- PEM */

typedef struct {
    const char *text;
    size_t len;
} pem_pass;

static int pem_callback(char *buf, int size, int rwflag, void *user) {
    (void)rwflag;
    const pem_pass *pass = user;
    if (!pass || pass->len == 0 || pass->len > (size_t)size) return 0;
    memcpy(buf, pass->text, pass->len);
    return (int)pass->len;
}

static bool parse_pem(const uint8_t *data, size_t len, const char *passphrase,
                      size_t passphrase_len, ssh_privkey *k, dp_err *err) {
    BIO *bio = BIO_new_mem_buf(data, (int)len);
    pem_pass pass = {passphrase, passphrase_len};
    k->pkey = bio ? PEM_read_bio_PrivateKey(bio, NULL, pem_callback, &pass) : NULL;
    BIO_free(bio);
    if (!k->pkey) {
        dp_fail(err, passphrase_len ? "the private key could not be decrypted or read"
                                    : "the private key could not be read; if it is encrypted, "
                                      "supply key_passphrase");
        return false;
    }
    return describe(k, err);
}

bool ssh_privkey_parse(const uint8_t *data, size_t len, const char *passphrase,
                       size_t passphrase_len, ssh_privkey *out, dp_err *err) {
    memset(out, 0, sizeof(*out));
    omt_buf_init(&out->blob, 4096);
    /* The parsers below treat the data as text; it is NUL-terminated here. */
    omt_buf text;
    omt_buf_init(&text, KEY_FILE_LIMIT + 1);
    omt_buf_append(&text, data, len);
    bool ok;
    if (text.failed || memchr(data, 0, len)) {
        dp_fail(err, "the private key file is malformed");
        ok = false;
    } else if (strstr(omt_buf_cstr(&text), BEGIN_OPENSSH)) {
        ok = parse_openssh(text.data, text.len, passphrase, passphrase_len, out, err);
    } else {
        ok = parse_pem(text.data, text.len, passphrase, passphrase_len, out, err);
    }
    omt_buf_free_secret(&text);
    if (!ok) ssh_privkey_free(out);
    return ok;
}

bool ssh_privkey_load(const char *path, const char *passphrase, size_t passphrase_len,
                      ssh_privkey *out, dp_err *err) {
    omt_buf data;
    omt_buf_init(&data, KEY_FILE_LIMIT + 1);
    memset(out, 0, sizeof(*out));
    bool ok = dp_read_file(path, KEY_FILE_LIMIT, &data, err) &&
              ssh_privkey_parse(data.data, data.len, passphrase, passphrase_len, out, err);
    omt_buf_free_secret(&data);
    return ok;
}

static bool sign_raw(EVP_PKEY *pkey, const char *digest, const uint8_t *data, size_t len,
                     uint8_t **sig, size_t *sig_len) {
    EVP_MD_CTX *md = EVP_MD_CTX_new();
    *sig = NULL;
    *sig_len = 0;
    bool ok = md && EVP_DigestSignInit_ex(md, NULL, digest, NULL, NULL, pkey, NULL) == 1 &&
              EVP_DigestSign(md, NULL, sig_len, data, len) == 1;
    if (ok) {
        *sig = OPENSSL_malloc(*sig_len);
        ok = *sig && EVP_DigestSign(md, *sig, sig_len, data, len) == 1;
    }
    EVP_MD_CTX_free(md);
    if (!ok) {
        OPENSSL_free(*sig);
        *sig = NULL;
    }
    return ok;
}

bool ssh_privkey_sign(const ssh_privkey *k, const char *alg, const uint8_t *data, size_t len,
                      omt_buf *sig, dp_err *err) {
    uint8_t *raw = NULL;
    size_t raw_len = 0;
    bool ok = false;
    int id = EVP_PKEY_get_base_id(k->pkey);
    if (id == EVP_PKEY_ED25519) {
        ok = sign_raw(k->pkey, NULL, data, len, &raw, &raw_len) && raw_len == 64;
        if (ok) {
            sb_cstring(sig, "ssh-ed25519");
            sb_string(sig, raw, raw_len);
        }
    } else if (id == EVP_PKEY_EC) {
        ok = sign_raw(k->pkey, k->ec_digest, data, len, &raw, &raw_len);
        const unsigned char *p = raw;
        ECDSA_SIG *es = ok ? d2i_ECDSA_SIG(NULL, &p, (long)raw_len) : NULL;
        ok = es != NULL;
        if (ok) {
            omt_buf inner;
            omt_buf_init(&inner, 512);
            ok = sb_mpint_bn(&inner, ECDSA_SIG_get0_r(es)) &&
                 sb_mpint_bn(&inner, ECDSA_SIG_get0_s(es));
            sb_cstring(sig, k->type);
            sb_string(sig, inner.data, inner.len);
            omt_buf_free(&inner);
        }
        ECDSA_SIG_free(es);
    } else if (id == EVP_PKEY_RSA) {
        const char *digest = strcmp(alg, "rsa-sha2-256") == 0 ? "SHA256" : "SHA512";
        ok = sign_raw(k->pkey, digest, data, len, &raw, &raw_len);
        if (ok) {
            sb_cstring(sig, alg);
            sb_string(sig, raw, raw_len);
        }
    }
    OPENSSL_free(raw);
    if (!ok || sig->failed) {
        dp_fail(err, "signing with the private key failed");
        return false;
    }
    return true;
}
