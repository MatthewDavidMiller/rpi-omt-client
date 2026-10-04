/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Public keys and signature verification: ssh-ed25519 (RFC 8709), ECDSA over
 * the NIST curves (RFC 5656), and RSA with SHA-2 only (RFC 8332). ssh-rsa
 * signatures, which hash with SHA-1, are refused.
 */
#include <openssl/core_names.h>
#include <openssl/ec.h>
#include <openssl/param_build.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

const char SSH_HOSTKEY_LIST[] = "ssh-ed25519,ecdsa-sha2-nistp256,ecdsa-sha2-nistp384,"
                                "ecdsa-sha2-nistp521,rsa-sha2-512,rsa-sha2-256";

/* The smallest RSA modulus accepted for a host or user key. */
#define SSH_RSA_MIN_BITS 2048

typedef struct {
    const char *key_type;
    const char *curve_id;
    const char *group;
    const char *digest;
} ec_curve;

static const ec_curve CURVES[] = {
    {"ecdsa-sha2-nistp256", "nistp256", "P-256", "SHA256"},
    {"ecdsa-sha2-nistp384", "nistp384", "P-384", "SHA384"},
    {"ecdsa-sha2-nistp521", "nistp521", "P-521", "SHA512"},
};

bool ssh_hostkey_alg_matches(const char *alg, const char *key_type) {
    if (strcmp(alg, "rsa-sha2-256") == 0 || strcmp(alg, "rsa-sha2-512") == 0) {
        return strcmp(key_type, "ssh-rsa") == 0;
    }
    return strcmp(alg, key_type) == 0;
}

static EVP_PKEY *from_params(const char *name, OSSL_PARAM_BLD *bld, int selection) {
    OSSL_PARAM *params = OSSL_PARAM_BLD_to_param(bld);
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_name(NULL, name, NULL);
    EVP_PKEY *pkey = NULL;
    if (params && ctx && EVP_PKEY_fromdata_init(ctx) == 1) {
        if (EVP_PKEY_fromdata(ctx, &pkey, selection, params) != 1) pkey = NULL;
    }
    EVP_PKEY_CTX_free(ctx);
    OSSL_PARAM_free(params);
    return pkey;
}

static bool public_check(EVP_PKEY *pkey) {
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_pkey(NULL, pkey, NULL);
    bool ok = ctx && EVP_PKEY_public_check(ctx) == 1;
    EVP_PKEY_CTX_free(ctx);
    return ok;
}

bool ssh_pubkey_parse(omt_span blob, ssh_pubkey *out, dp_err *err) {
    memset(out, 0, sizeof(*out));
    omt_span r = blob, type;
    if (!sr_string(&r, &type) || type.len >= sizeof(out->name)) {
        dp_fail(err, "malformed public key");
        return false;
    }
    memcpy(out->name, type.p, type.len);
    out->name[type.len] = 0;
    if (span_is(type, "ssh-ed25519")) {
        omt_span pk;
        if (!sr_string(&r, &pk) || pk.len != 32 || r.len != 0) goto malformed;
        out->type = EVP_PKEY_ED25519;
        out->pkey = EVP_PKEY_new_raw_public_key(EVP_PKEY_ED25519, NULL, pk.p, 32);
        if (!out->pkey) goto malformed;
        return true;
    }
    for (size_t i = 0; i < OMT_ARRAY_LEN(CURVES); i++) {
        if (!span_is(type, CURVES[i].key_type)) continue;
        omt_span curve, q;
        if (!sr_string(&r, &curve) || !span_is(curve, CURVES[i].curve_id) || !sr_string(&r, &q) ||
            r.len != 0 || q.len == 0 || q.p[0] != 0x04) {
            goto malformed;
        }
        OSSL_PARAM_BLD *bld = OSSL_PARAM_BLD_new();
        if (!bld) goto malformed;
        OSSL_PARAM_BLD_push_utf8_string(bld, OSSL_PKEY_PARAM_GROUP_NAME, CURVES[i].group, 0);
        OSSL_PARAM_BLD_push_octet_string(bld, OSSL_PKEY_PARAM_PUB_KEY, q.p, q.len);
        out->pkey = from_params("EC", bld, EVP_PKEY_PUBLIC_KEY);
        OSSL_PARAM_BLD_free(bld);
        if (!out->pkey || !public_check(out->pkey)) goto malformed;
        out->type = EVP_PKEY_EC;
        out->digest = CURVES[i].digest;
        return true;
    }
    if (span_is(type, "ssh-rsa")) {
        BIGNUM *e = NULL, *n = NULL;
        if (!sr_mpint_bn(&r, &e) || !sr_mpint_bn(&r, &n) || r.len != 0) {
            BN_free(e);
            BN_free(n);
            goto malformed;
        }
        if (BN_num_bits(n) < SSH_RSA_MIN_BITS || BN_num_bits(n) > 16384 || !BN_is_odd(e)) {
            BN_free(e);
            BN_free(n);
            dp_fail(err, "RSA keys must have at least %d bits", SSH_RSA_MIN_BITS);
            ssh_pubkey_free(out);
            return false;
        }
        OSSL_PARAM_BLD *bld = OSSL_PARAM_BLD_new();
        if (bld) {
            OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_N, n);
            OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_RSA_E, e);
            out->pkey = from_params("RSA", bld, EVP_PKEY_PUBLIC_KEY);
            OSSL_PARAM_BLD_free(bld);
        }
        BN_free(e);
        BN_free(n);
        if (!out->pkey) goto malformed;
        out->type = EVP_PKEY_RSA;
        return true;
    }
    dp_fail(err, "unsupported public key type %s", out->name);
    ssh_pubkey_free(out);
    return false;
malformed:
    ssh_pubkey_free(out);
    dp_fail(err, "malformed public key");
    return false;
}

void ssh_pubkey_free(ssh_pubkey *k) {
    EVP_PKEY_free(k->pkey);
    k->pkey = NULL;
}

static bool verify_raw(EVP_PKEY *pkey, const char *digest, const uint8_t *sig, size_t sig_len,
                       const uint8_t *data, size_t len) {
    EVP_MD_CTX *md = EVP_MD_CTX_new();
    if (!md) return false;
    bool ok = EVP_DigestVerifyInit_ex(md, NULL, digest, NULL, NULL, pkey, NULL) == 1 &&
              EVP_DigestVerify(md, sig, sig_len, data, len) == 1;
    EVP_MD_CTX_free(md);
    return ok;
}

bool ssh_pubkey_verify(const ssh_pubkey *k, const char *alg, omt_span sig, const uint8_t *data,
                       size_t len, dp_err *err) {
    omt_span r = sig, name, body;
    if (!sr_string(&r, &name) || !sr_string(&r, &body) || r.len != 0 || !span_is(name, alg) ||
        !ssh_hostkey_alg_matches(alg, k->name)) {
        dp_fail(err, "malformed or mismatched signature");
        return false;
    }
    bool ok = false;
    if (k->type == EVP_PKEY_ED25519) {
        ok = body.len == 64 && verify_raw(k->pkey, NULL, body.p, body.len, data, len);
    } else if (k->type == EVP_PKEY_EC) {
        BIGNUM *rr = NULL, *ss = NULL;
        omt_span b = body;
        ECDSA_SIG *es = ECDSA_SIG_new();
        if (es && sr_mpint_bn(&b, &rr) && sr_mpint_bn(&b, &ss) && b.len == 0 &&
            ECDSA_SIG_set0(es, rr, ss) == 1) {
            rr = ss = NULL;
            unsigned char *der = NULL;
            int der_len = i2d_ECDSA_SIG(es, &der);
            ok = der_len > 0 && verify_raw(k->pkey, k->digest, der, (size_t)der_len, data, len);
            OPENSSL_free(der);
        }
        BN_free(rr);
        BN_free(ss);
        ECDSA_SIG_free(es);
    } else if (k->type == EVP_PKEY_RSA) {
        const char *digest = strcmp(alg, "rsa-sha2-512") == 0 ? "SHA512" : "SHA256";
        size_t modulus = (size_t)EVP_PKEY_get_size(k->pkey);
        if (body.len <= modulus) {
            /* A signature shorter than the modulus has lost leading zero
             * bytes; OpenSSH pads it back rather than refusing it. */
            uint8_t *padded = OPENSSL_zalloc(modulus);
            if (padded) {
                memcpy(padded + (modulus - body.len), body.p, body.len);
                ok = verify_raw(k->pkey, digest, padded, modulus, data, len);
                OPENSSL_free(padded);
            }
        }
    }
    if (!ok) dp_fail(err, "signature verification failed");
    return ok;
}
