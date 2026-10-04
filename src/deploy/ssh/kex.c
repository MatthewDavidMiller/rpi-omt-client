/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Key exchange: algorithm negotiation (RFC 4253 section 7), the exchange
 * methods, host-key verification against known_hosts, and key derivation.
 *
 * Methods, in preference order: mlkem768x25519-sha256 (post-quantum hybrid),
 * curve25519-sha256 (RFC 8731), diffie-hellman-group-exchange-sha256
 * (RFC 4419), and the RFC 8268 MODP groups. Strict key exchange is mandatory:
 * a server that does not offer it is refused, because without it an attacker
 * on the path can delete the first packets after the handshake undetected.
 */
#include <openssl/core_names.h>
#include <openssl/rand.h>
#include <string.h>

#include "deploy/ssh/ssh_internal.h"

#define STRICT_C "kex-strict-c-v00@openssh.com"
#define STRICT_S "kex-strict-s-v00@openssh.com"

typedef enum {
    KEX_MLKEM768X25519,
    KEX_CURVE25519,
    KEX_DH_GEX,
    KEX_DH_GROUP,
} kex_kind;

typedef struct {
    const char *name;
    kex_kind kind;
    const char *hash;
    int group_bits; /* for the fixed MODP groups */
} kex_method;

static const kex_method METHODS[] = {
    {"mlkem768x25519-sha256", KEX_MLKEM768X25519, "SHA256", 0},
    {"curve25519-sha256", KEX_CURVE25519, "SHA256", 0},
    {"curve25519-sha256@libssh.org", KEX_CURVE25519, "SHA256", 0},
    {"diffie-hellman-group-exchange-sha256", KEX_DH_GEX, "SHA256", 0},
    {"diffie-hellman-group18-sha512", KEX_DH_GROUP, "SHA512", 8192},
    {"diffie-hellman-group17-sha512", KEX_DH_GROUP, "SHA512", 6144},
    {"diffie-hellman-group16-sha512", KEX_DH_GROUP, "SHA512", 4096},
    {"diffie-hellman-group15-sha512", KEX_DH_GROUP, "SHA512", 3072},
    {"diffie-hellman-group14-sha256", KEX_DH_GROUP, "SHA256", 2048},
};

/* Group exchange bounds: no group smaller than the smallest fixed one. */
#define GEX_MIN 2048u
#define GEX_PREFERRED 4096u
#define GEX_MAX 8192u

static bool mlkem_available(void) {
    static int cached = -1;
    if (cached < 0) {
        EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_name(NULL, "ML-KEM-768", NULL);
        cached = ctx != NULL;
        EVP_PKEY_CTX_free(ctx);
    }
    return cached == 1;
}

static void kex_list(omt_buf *out) {
    for (size_t i = 0; i < OMT_ARRAY_LEN(METHODS); i++) {
        if (METHODS[i].kind == KEX_MLKEM768X25519 && !mlkem_available()) continue;
        if (out->len) omt_buf_putc(out, ',');
        omt_buf_puts(out, METHODS[i].name);
    }
    omt_buf_puts(out, ",ext-info-c," STRICT_C);
}

/* -------------------------------------------------------------- KEXINIT */

typedef struct {
    omt_span lists[10];
    bool first_follows;
} kexinit;

static bool parse_kexinit(const omt_buf *payload, kexinit *k) {
    omt_span r = omt_span_of(payload->data, payload->len);
    uint8_t type;
    uint32_t reserved;
    if (!sr_u8(&r, &type) || type != SSH_MSG_KEXINIT || !omt_span_skip(&r, 16)) return false;
    for (int i = 0; i < 10; i++) {
        if (!sr_string(&r, &k->lists[i])) return false;
    }
    return sr_bool(&r, &k->first_follows) && sr_u32(&r, &reserved);
}

/* The first entry of ours that theirs also lists. */
static bool choose(const char *ours, omt_span theirs, char *out, size_t size) {
    const char *p = ours;
    while (*p) {
        const char *comma = strchr(p, ',');
        size_t n = comma ? (size_t)(comma - p) : strlen(p);
        char name[96];
        if (n < sizeof(name)) {
            memcpy(name, p, n);
            name[n] = 0;
            if (namelist_has(theirs, name)) return omt_strlcpy(out, name, size);
        }
        p += n + (comma ? 1 : 0);
    }
    return false;
}

/* The first entry of a name-list. */
static omt_span first_name(omt_span list) {
    const uint8_t *comma = list.len ? memchr(list.p, ',', list.len) : NULL;
    if (comma) list.len = (size_t)(comma - list.p);
    return list;
}

static bool send_kexinit(ssh_transport *t, dp_err *err) {
    omt_buf *b = &t->i_c;
    omt_buf_clear(b);
    sb_u8(b, SSH_MSG_KEXINIT);
    uint8_t cookie[16];
    if (RAND_bytes(cookie, sizeof(cookie)) != 1) {
        dp_fail(err, "randomness is unavailable");
        return false;
    }
    omt_buf_append(b, cookie, sizeof(cookie));
    omt_buf kex;
    omt_buf_init(&kex, 2048);
    kex_list(&kex);
    sb_string(b, kex.data, kex.len);
    omt_buf_free(&kex);
    sb_string(b, t->preferred_hostkeys.data, t->preferred_hostkeys.len);
    sb_cstring(b, SSH_CIPHER_LIST);
    sb_cstring(b, SSH_CIPHER_LIST);
    sb_cstring(b, SSH_MAC_LIST);
    sb_cstring(b, SSH_MAC_LIST);
    sb_cstring(b, "none");
    sb_cstring(b, "none");
    sb_cstring(b, "");
    sb_cstring(b, "");
    sb_bool(b, false);
    sb_u32(b, 0);
    return ssh_send(t, b, err);
}

/* ------------------------------------------------------------- hashing */

typedef struct {
    EVP_MD_CTX *md;
    const char *name;
} hasher;

static bool hash_begin(hasher *h, const char *name) {
    h->name = name;
    h->md = EVP_MD_CTX_new();
    return h->md && EVP_DigestInit_ex2(h->md, EVP_get_digestbyname(name), NULL) == 1;
}

static void hash_add(hasher *h, const omt_buf *b) {
    if (h->md) EVP_DigestUpdate(h->md, b->data, b->len);
}

static bool hash_end(hasher *h, uint8_t *out, size_t *len) {
    unsigned int n = 0;
    bool ok = h->md && EVP_DigestFinal_ex(h->md, out, &n) == 1;
    EVP_MD_CTX_free(h->md);
    h->md = NULL;
    *len = n;
    return ok;
}

static bool digest(const char *name, const uint8_t *data, size_t len, uint8_t *out,
                   size_t *out_len) {
    unsigned int n = 0;
    bool ok = EVP_Digest(data, len, out, &n, EVP_get_digestbyname(name), NULL) == 1;
    *out_len = n;
    return ok;
}

/* ---------------------------------------------------------- the methods */

typedef struct {
    omt_buf k;      /* the shared secret, encoded as the method encodes it */
    omt_buf k_s;    /* host key blob */
    omt_buf sig;    /* signature blob */
    omt_buf prefix; /* method-specific exchange-hash fields after K_S */
} kex_out;

static ssh_recv_status expect(ssh_transport *t, uint8_t type, uint64_t deadline, dp_err *err) {
    for (;;) {
        ssh_recv_status st = ssh_read_packet(t, deadline, err);
        if (st == SSH_RECV_TIMEOUT) dp_fail(err, "SSH key exchange timed out");
        if (st != SSH_RECV_OK) return st;
        uint8_t got = t->payload.data[0];
        if (got == type) return SSH_RECV_OK;
        if (got == SSH_MSG_DISCONNECT) {
            dp_fail(err, "the SSH server disconnected during key exchange");
            return SSH_RECV_ERROR;
        }
        /* Strict mode allows nothing else during the first exchange; a
         * re-key may still interleave the harmless transport messages. */
        if (t->first_kex_done && (got == SSH_MSG_IGNORE || got == SSH_MSG_DEBUG)) continue;
        dp_fail(err, "unexpected SSH message %u during key exchange", (unsigned)got);
        return SSH_RECV_ERROR;
    }
}

static EVP_PKEY *keygen(const char *name) { return EVP_PKEY_Q_keygen(NULL, NULL, name); }

static bool raw_public(EVP_PKEY *key, omt_buf *out, size_t expected) {
    uint8_t raw[1600];
    size_t len = sizeof(raw);
    if (EVP_PKEY_get_octet_string_param(key, OSSL_PKEY_PARAM_ENCODED_PUBLIC_KEY, raw, sizeof(raw),
                                        &len) != 1 &&
        EVP_PKEY_get_raw_public_key(key, raw, &len) != 1) {
        return false;
    }
    if (len != expected) return false;
    omt_buf_append(out, raw, len);
    return true;
}

static bool x25519_derive(EVP_PKEY *mine, const uint8_t peer_raw[32], uint8_t secret[32]) {
    EVP_PKEY *peer = EVP_PKEY_new_raw_public_key(EVP_PKEY_X25519, NULL, peer_raw, 32);
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new(mine, NULL);
    size_t len = 32;
    bool ok = peer && ctx && EVP_PKEY_derive_init(ctx) == 1 &&
              EVP_PKEY_derive_set_peer(ctx, peer) == 1 && EVP_PKEY_derive(ctx, secret, &len) == 1 &&
              len == 32;
    EVP_PKEY_CTX_free(ctx);
    EVP_PKEY_free(peer);
    /* An all-zero result means the peer sent a low-order point. */
    uint8_t any = 0;
    for (int i = 0; i < 32; i++) any |= secret[i];
    return ok && any != 0;
}

/* Reads the common reply: string K_S, string Q_S (or mpint f), string sig. */
static bool read_ecdh_reply(ssh_transport *t, kex_out *o, omt_span *q_s, uint64_t deadline,
                            dp_err *err) {
    if (expect(t, SSH_MSG_KEX_31, deadline, err) != SSH_RECV_OK) return false;
    omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1), ks, sig;
    if (!sr_string(&r, &ks) || !sr_string(&r, q_s) || !sr_string(&r, &sig) || r.len != 0) {
        dp_fail(err, "malformed key exchange reply");
        return false;
    }
    omt_buf_append(&o->k_s, ks.p, ks.len);
    omt_buf_append(&o->sig, sig.p, sig.len);
    return true;
}

static bool run_curve25519(ssh_transport *t, kex_out *o, uint64_t deadline, dp_err *err) {
    EVP_PKEY *mine = keygen("X25519");
    omt_buf q_c, msg;
    omt_buf_init(&q_c, 64);
    omt_buf_init(&msg, 128);
    bool ok = mine && raw_public(mine, &q_c, 32);
    if (ok) {
        sb_u8(&msg, SSH_MSG_KEX_30);
        sb_string(&msg, q_c.data, q_c.len);
        ok = ssh_send(t, &msg, err);
    } else {
        dp_fail(err, "X25519 key generation failed");
    }
    omt_span q_s;
    if (ok) ok = read_ecdh_reply(t, o, &q_s, deadline, err);
    uint8_t secret[32];
    if (ok && (q_s.len != 32 || !x25519_derive(mine, q_s.p, secret))) {
        dp_fail(err, "invalid curve25519 key exchange value");
        ok = false;
    }
    if (ok) {
        sb_string(&o->prefix, q_c.data, q_c.len);
        sb_string(&o->prefix, q_s.p, q_s.len);
        sb_mpint_bytes(&o->k, secret, 32);
    }
    OPENSSL_cleanse(secret, sizeof(secret));
    EVP_PKEY_free(mine);
    omt_buf_free(&q_c);
    omt_buf_free(&msg);
    return ok;
}

#define MLKEM768_PK 1184u
#define MLKEM768_CT 1088u

static bool run_mlkem(ssh_transport *t, kex_out *o, uint64_t deadline, dp_err *err) {
    EVP_PKEY *kem = keygen("ML-KEM-768");
    EVP_PKEY *ecdh = keygen("X25519");
    omt_buf q_c, msg;
    omt_buf_init(&q_c, 2048);
    omt_buf_init(&msg, 2048);
    bool ok = kem && ecdh && raw_public(kem, &q_c, MLKEM768_PK) && raw_public(ecdh, &q_c, 32);
    if (ok) {
        sb_u8(&msg, SSH_MSG_KEX_30);
        sb_string(&msg, q_c.data, q_c.len);
        ok = ssh_send(t, &msg, err);
    } else {
        dp_fail(err, "ML-KEM-768 key generation failed");
    }
    omt_span q_s;
    if (ok) ok = read_ecdh_reply(t, o, &q_s, deadline, err);
    uint8_t shared[64], hashed[64];
    size_t hashed_len = 0;
    if (ok) {
        ok = q_s.len == MLKEM768_CT + 32;
        size_t ss_len = 32;
        EVP_PKEY_CTX *ctx = ok ? EVP_PKEY_CTX_new(kem, NULL) : NULL;
        ok = ok && ctx && EVP_PKEY_decapsulate_init(ctx, NULL) == 1 &&
             EVP_PKEY_decapsulate(ctx, shared, &ss_len, q_s.p, MLKEM768_CT) == 1 && ss_len == 32;
        EVP_PKEY_CTX_free(ctx);
        ok = ok && x25519_derive(ecdh, q_s.p + MLKEM768_CT, shared + 32) &&
             digest("SHA256", shared, 64, hashed, &hashed_len);
        if (!ok) dp_fail(err, "invalid mlkem768x25519 key exchange value");
    }
    if (ok) {
        sb_string(&o->prefix, q_c.data, q_c.len);
        sb_string(&o->prefix, q_s.p, q_s.len);
        /* The hybrid methods encode K as a string, not an mpint. */
        sb_string(&o->k, hashed, hashed_len);
    }
    OPENSSL_cleanse(shared, sizeof(shared));
    OPENSSL_cleanse(hashed, sizeof(hashed));
    EVP_PKEY_free(kem);
    EVP_PKEY_free(ecdh);
    omt_buf_free(&q_c);
    omt_buf_free(&msg);
    return ok;
}

static BIGNUM *fixed_prime(int bits) {
    switch (bits) {
    case 2048: return BN_get_rfc3526_prime_2048(NULL);
    case 3072: return BN_get_rfc3526_prime_3072(NULL);
    case 4096: return BN_get_rfc3526_prime_4096(NULL);
    case 6144: return BN_get_rfc3526_prime_6144(NULL);
    case 8192: return BN_get_rfc3526_prime_8192(NULL);
    default: return NULL;
    }
}

/* 1 < v < p - 1 */
static bool dh_value_ok(const BIGNUM *v, const BIGNUM *p) {
    BIGNUM *limit = BN_dup(p);
    bool ok = limit && BN_sub_word(limit, 1) == 1 && BN_cmp(v, BN_value_one()) > 0 &&
              BN_cmp(v, limit) < 0;
    BN_free(limit);
    return ok;
}

/* Shared by both DH methods once the group is fixed: e out, f and K_S in. */
static bool run_dh(ssh_transport *t, kex_out *o, const BIGNUM *p, const BIGNUM *g, uint8_t init,
                   uint8_t reply, uint64_t deadline, dp_err *err) {
    BN_CTX *bn = BN_CTX_new();
    BIGNUM *x = BN_secure_new(), *e = BN_new(), *f = NULL, *k = BN_secure_new();
    omt_buf msg;
    omt_buf_init(&msg, 2048);
    /* A 512-bit exponent is twice the strength of the largest cipher key. */
    bool ok = bn && x && e && k &&
              BN_priv_rand_ex(x, 512, BN_RAND_TOP_ONE, BN_RAND_BOTTOM_ANY, 0, NULL) == 1;
    if (ok) {
        BN_set_flags(x, BN_FLG_CONSTTIME);
        ok = BN_mod_exp_mont_consttime(e, g, x, p, bn, NULL) == 1 && dh_value_ok(e, p);
    }
    if (ok) {
        sb_u8(&msg, init);
        ok = sb_mpint_bn(&msg, e) && ssh_send(t, &msg, err);
    } else {
        dp_fail(err, "Diffie-Hellman key generation failed");
    }
    if (ok && expect(t, reply, deadline, err) != SSH_RECV_OK) ok = false;
    if (ok) {
        omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1), ks, sig;
        ok = sr_string(&r, &ks) && sr_mpint_bn(&r, &f) && sr_string(&r, &sig) && r.len == 0 &&
             dh_value_ok(f, p);
        if (ok) {
            omt_buf_append(&o->k_s, ks.p, ks.len);
            omt_buf_append(&o->sig, sig.p, sig.len);
            ok = BN_mod_exp_mont_consttime(k, f, x, p, bn, NULL) == 1 && sb_mpint_bn(&o->k, k) &&
                 sb_mpint_bn(&o->prefix, e) && sb_mpint_bn(&o->prefix, f);
        }
        if (!ok) dp_fail(err, "invalid Diffie-Hellman key exchange value");
    }
    BN_clear_free(x);
    BN_clear_free(k);
    BN_free(e);
    BN_free(f);
    BN_CTX_free(bn);
    omt_buf_free(&msg);
    return ok;
}

static bool run_dh_group(ssh_transport *t, kex_out *o, int bits, uint64_t deadline, dp_err *err) {
    BIGNUM *p = fixed_prime(bits), *g = BN_new();
    bool ok = p && g && BN_set_word(g, 2) == 1 &&
              run_dh(t, o, p, g, SSH_MSG_KEX_30, SSH_MSG_KEX_31, deadline, err);
    BN_free(p);
    BN_free(g);
    return ok;
}

static bool run_dh_gex(ssh_transport *t, kex_out *o, uint64_t deadline, dp_err *err) {
    omt_buf msg;
    omt_buf_init(&msg, 64);
    sb_u8(&msg, SSH_MSG_KEX_DH_GEX_REQUEST);
    sb_u32(&msg, GEX_MIN);
    sb_u32(&msg, GEX_PREFERRED);
    sb_u32(&msg, GEX_MAX);
    bool ok = ssh_send(t, &msg, err);
    omt_buf_free(&msg);
    BIGNUM *p = NULL, *g = NULL;
    if (ok && expect(t, SSH_MSG_KEX_31, deadline, err) != SSH_RECV_OK) ok = false;
    if (ok) {
        omt_span r = omt_span_of(t->payload.data + 1, t->payload.len - 1);
        int bits = 0;
        ok = sr_mpint_bn(&r, &p) && sr_mpint_bn(&r, &g) && r.len == 0;
        if (ok) bits = BN_num_bits(p);
        if (!ok || bits < (int)GEX_MIN || bits > (int)GEX_MAX || !BN_is_odd(p) ||
            !dh_value_ok(g, p)) {
            dp_fail(err, "the SSH server offered an unacceptable Diffie-Hellman group");
            ok = false;
        }
    }
    if (ok) {
        sb_u32(&o->prefix, GEX_MIN);
        sb_u32(&o->prefix, GEX_PREFERRED);
        sb_u32(&o->prefix, GEX_MAX);
        ok = sb_mpint_bn(&o->prefix, p) && sb_mpint_bn(&o->prefix, g) &&
             run_dh(t, o, p, g, SSH_MSG_KEX_DH_GEX_INIT, SSH_MSG_KEX_DH_GEX_REPLY, deadline, err);
    }
    BN_free(p);
    BN_free(g);
    return ok;
}

/* ------------------------------------------------------------- the keys */

/* K1 = HASH(K || H || letter || session_id), Kn = HASH(K || H || K1..Kn-1) */
static bool derive(const char *hash, const omt_buf *k, const uint8_t *h, size_t h_len, char letter,
                   const omt_buf *session_id, uint8_t *out, size_t need) {
    uint8_t material[256];
    size_t have = 0;
    while (have < need) {
        hasher d;
        if (!hash_begin(&d, hash)) return false;
        hash_add(&d, k);
        EVP_DigestUpdate(d.md, h, h_len);
        if (have == 0) {
            EVP_DigestUpdate(d.md, &letter, 1);
            hash_add(&d, session_id);
        } else {
            EVP_DigestUpdate(d.md, material, have);
        }
        size_t n = 0;
        if (!hash_end(&d, material + have, &n) || have + n > sizeof(material)) return false;
        have += n;
    }
    memcpy(out, material, need);
    OPENSSL_cleanse(material, sizeof(material));
    return true;
}

static bool host_key_trusted(ssh_transport *t, const omt_buf *k_s, dp_err *err) {
    omt_span blob = omt_span_of(k_s->data, k_s->len);
    if (t->first_kex_done) {
        /* A re-key must present the key the session was established with. */
        if (k_s->len != t->host_key.len || memcmp(k_s->data, t->host_key.data, k_s->len) != 0) {
            dp_fail(err, "the SSH server changed its host key during a re-key");
            return false;
        }
        return true;
    }
    ssh_host_status status;
    if (!ssh_known_hosts_check(t->known_hosts, t->host, t->port, blob, &status, err)) return false;
    switch (status) {
    case SSH_HOST_KNOWN: return true;
    case SSH_HOST_UNKNOWN:
        dp_fail(err, "the host key of %s is not in %s", t->host, t->known_hosts);
        return false;
    case SSH_HOST_CHANGED:
        dp_fail(err, "the host key of %s does not match %s; it may be an impersonation", t->host,
                t->known_hosts);
        return false;
    case SSH_HOST_REVOKED:
        dp_fail(err, "the host key of %s is revoked in %s", t->host, t->known_hosts);
        return false;
    }
    return false;
}

static void kex_out_init(kex_out *o) {
    omt_buf_init(&o->k, 2048);
    omt_buf_init(&o->k_s, 64u * 1024u);
    omt_buf_init(&o->sig, 64u * 1024u);
    omt_buf_init(&o->prefix, 8192);
}

static void kex_out_free(kex_out *o) {
    omt_buf_free_secret(&o->k);
    omt_buf_free(&o->k_s);
    omt_buf_free(&o->sig);
    omt_buf_free(&o->prefix);
}

bool ssh_kex_run(ssh_transport *t, bool server_kexinit_received, dp_err *err) {
    uint64_t deadline = dp_now_ms() + t->idle_ms;
    omt_buf server_init;
    omt_buf_init(&server_init, 64u * 1024u);
    kex_out o;
    kex_out_init(&o);
    bool ok = false;
    if (server_kexinit_received) omt_buf_append(&server_init, t->payload.data, t->payload.len);
    if (!send_kexinit(t, err)) goto done;
    if (!server_kexinit_received) {
        if (expect(t, SSH_MSG_KEXINIT, deadline, err) != SSH_RECV_OK) goto done;
        omt_buf_append(&server_init, t->payload.data, t->payload.len);
    }
    omt_buf_clear(&t->i_s);
    omt_buf_append(&t->i_s, server_init.data, server_init.len);

    kexinit theirs;
    if (!parse_kexinit(&t->i_s, &theirs)) {
        dp_fail(err, "malformed KEXINIT from the SSH server");
        goto done;
    }
    if (!t->first_kex_done) {
        if (!namelist_has(theirs.lists[0], STRICT_S)) {
            dp_fail(err, "the SSH server does not support strict key exchange (the Terrapin "
                         "mitigation); update its SSH server");
            goto done;
        }
        t->strict_kex = true;
    }
    omt_buf ours;
    omt_buf_init(&ours, 2048);
    kex_list(&ours);
    char kex_name[96] = "", hostkey[64] = "", enc_cs[64] = "", enc_sc[64] = "";
    char mac_cs[64] = "", mac_sc[64] = "", comp[16];
    bool chosen = choose(omt_buf_cstr(&ours), theirs.lists[0], kex_name, sizeof(kex_name)) &&
                  strcmp(kex_name, "ext-info-c") != 0 && strcmp(kex_name, STRICT_C) != 0;
    omt_buf_free(&ours);
    if (!chosen) {
        dp_fail(err, "no common SSH key exchange algorithm");
        goto done;
    }
    if (!choose(omt_buf_cstr(&t->preferred_hostkeys), theirs.lists[1], hostkey, sizeof(hostkey))) {
        dp_fail(err, "no common SSH host key algorithm");
        goto done;
    }
    if (!choose(SSH_CIPHER_LIST, theirs.lists[2], enc_cs, sizeof(enc_cs)) ||
        !choose(SSH_CIPHER_LIST, theirs.lists[3], enc_sc, sizeof(enc_sc))) {
        dp_fail(err, "no common SSH cipher");
        goto done;
    }
    const ssh_cipher_alg *cs = ssh_cipher_find(omt_span_of(enc_cs, strlen(enc_cs)));
    const ssh_cipher_alg *sc = ssh_cipher_find(omt_span_of(enc_sc, strlen(enc_sc)));
    const ssh_mac_alg *mcs = NULL, *msc = NULL;
    if (cs->kind == SSH_CIPHER_CTR) {
        if (!choose(SSH_MAC_LIST, theirs.lists[4], mac_cs, sizeof(mac_cs))) goto no_mac;
        mcs = ssh_mac_find(omt_span_of(mac_cs, strlen(mac_cs)));
    }
    if (sc->kind == SSH_CIPHER_CTR) {
        if (!choose(SSH_MAC_LIST, theirs.lists[5], mac_sc, sizeof(mac_sc))) goto no_mac;
        msc = ssh_mac_find(omt_span_of(mac_sc, strlen(mac_sc)));
    }
    if (!choose("none", theirs.lists[6], comp, sizeof(comp)) ||
        !choose("none", theirs.lists[7], comp, sizeof(comp))) {
        dp_fail(err, "the SSH server requires compression, which this client does not offer");
        goto done;
    }
    const kex_method *method = NULL;
    for (size_t i = 0; i < OMT_ARRAY_LEN(METHODS); i++) {
        if (strcmp(METHODS[i].name, kex_name) == 0) method = &METHODS[i];
    }
    if (!method) goto done;
    /* A server that guessed wrong sends a key exchange packet to discard. */
    if (theirs.first_follows && (!span_is(first_name(theirs.lists[0]), kex_name) ||
                                 !span_is(first_name(theirs.lists[1]), hostkey))) {
        if (ssh_read_packet(t, deadline, err) != SSH_RECV_OK) goto done;
    }

    switch (method->kind) {
    case KEX_MLKEM768X25519: ok = run_mlkem(t, &o, deadline, err); break;
    case KEX_CURVE25519: ok = run_curve25519(t, &o, deadline, err); break;
    case KEX_DH_GEX: ok = run_dh_gex(t, &o, deadline, err); break;
    case KEX_DH_GROUP: ok = run_dh_group(t, &o, method->group_bits, deadline, err); break;
    }
    if (!ok) goto done;
    ok = false;

    /* H = HASH(V_C || V_S || I_C || I_S || K_S || method fields || K) */
    omt_buf fields;
    omt_buf_init(&fields, 256u * 1024u);
    sb_string(&fields, t->v_c.data, t->v_c.len);
    sb_string(&fields, t->v_s.data, t->v_s.len);
    sb_string(&fields, t->i_c.data, t->i_c.len);
    sb_string(&fields, t->i_s.data, t->i_s.len);
    sb_string(&fields, o.k_s.data, o.k_s.len);
    omt_buf_append(&fields, o.prefix.data, o.prefix.len);
    hasher hh;
    uint8_t h[64];
    size_t h_len = 0;
    bool hashed = !fields.failed && hash_begin(&hh, method->hash);
    if (hashed) {
        hash_add(&hh, &fields);
        hash_add(&hh, &o.k);
        hashed = hash_end(&hh, h, &h_len);
    } else {
        EVP_MD_CTX_free(hh.md);
    }
    omt_buf_free(&fields);
    if (!hashed) {
        dp_fail(err, "exchange hash failed");
        goto done;
    }

    ssh_pubkey key;
    if (!ssh_pubkey_parse(omt_span_of(o.k_s.data, o.k_s.len), &key, err)) goto done;
    bool verified =
        ssh_hostkey_alg_matches(hostkey, key.name) &&
        ssh_pubkey_verify(&key, hostkey, omt_span_of(o.sig.data, o.sig.len), h, h_len, err);
    ssh_pubkey_free(&key);
    if (!verified) {
        if (err && err->msg.len == 0) dp_fail(err, "host key signature verification failed");
        goto done;
    }
    if (!host_key_trusted(t, &o.k_s, err)) goto done;
    if (!t->first_kex_done) {
        omt_buf_append(&t->session_id, h, h_len);
        omt_buf_append(&t->host_key, o.k_s.data, o.k_s.len);
    }

    /* Keys for both directions, then NEWKEYS each way. */
    uint8_t iv_cs[16], iv_sc[16], key_cs[64], key_sc[64], mk_cs[64], mk_sc[64];
    bool derived =
        derive(method->hash, &o.k, h, h_len, 'A', &t->session_id, iv_cs, cs->iv_len) &&
        derive(method->hash, &o.k, h, h_len, 'B', &t->session_id, iv_sc, sc->iv_len) &&
        derive(method->hash, &o.k, h, h_len, 'C', &t->session_id, key_cs, cs->key_len) &&
        derive(method->hash, &o.k, h, h_len, 'D', &t->session_id, key_sc, sc->key_len) &&
        (!mcs || derive(method->hash, &o.k, h, h_len, 'E', &t->session_id, mk_cs, mcs->key_len)) &&
        (!msc || derive(method->hash, &o.k, h, h_len, 'F', &t->session_id, mk_sc, msc->key_len));
    omt_buf newkeys;
    omt_buf_init(&newkeys, 8);
    sb_u8(&newkeys, SSH_MSG_NEWKEYS);
    if (derived && ssh_send(t, &newkeys, err)) {
        if (t->strict_kex) t->send_seq = 0;
        if (!ssh_cipher_init(&t->send, cs, mcs, key_cs, iv_cs, mk_cs, true)) {
            dp_fail(err, "cipher initialization failed");
        } else if (expect(t, SSH_MSG_NEWKEYS, deadline, err) == SSH_RECV_OK) {
            if (t->strict_kex) t->recv_seq = 0;
            if (ssh_cipher_init(&t->recv, sc, msc, key_sc, iv_sc, mk_sc, false)) {
                ok = true;
            } else {
                dp_fail(err, "cipher initialization failed");
            }
        }
    } else if (!derived) {
        dp_fail(err, "key derivation failed");
    }
    omt_buf_free(&newkeys);
    OPENSSL_cleanse(iv_cs, sizeof(iv_cs));
    OPENSSL_cleanse(iv_sc, sizeof(iv_sc));
    OPENSSL_cleanse(key_cs, sizeof(key_cs));
    OPENSSL_cleanse(key_sc, sizeof(key_sc));
    OPENSSL_cleanse(mk_cs, sizeof(mk_cs));
    OPENSSL_cleanse(mk_sc, sizeof(mk_sc));
    OPENSSL_cleanse(h, sizeof(h));
    if (ok) t->first_kex_done = true;
    goto done;

no_mac:
    dp_fail(err, "no common SSH MAC algorithm");
done:
    kex_out_free(&o);
    omt_buf_free(&server_init);
    return ok;
}
