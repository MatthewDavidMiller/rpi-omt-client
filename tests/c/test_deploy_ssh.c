/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The SSH client's parts that need no server: wire encoding, the packet
 * ciphers, known_hosts, bcrypt_pbkdf, and user-key signing. The protocol as a
 * whole is held to OpenSSH by tests/integration/test_ssh_client.sh.
 */
#include <openssl/evp.h>
#include <openssl/pem.h>
#include <openssl/rand.h>

#include "deploy/ssh/blowfish.h"
#include "deploy/ssh/ssh_internal.h"
#include "test.h"

/* ------------------------------------------------------------------- wire */

static void mpints_are_minimal_and_non_negative(void) {
    omt_buf b;
    omt_buf_init(&b, 256);
    const uint8_t zero[] = {0, 0};
    sb_mpint_bytes(&b, zero, 2);
    const uint8_t high[] = {0x00, 0x80, 0x01};
    sb_mpint_bytes(&b, high, 3);
    const uint8_t low[] = {0x7f};
    sb_mpint_bytes(&b, low, 1);
    const uint8_t expect[] = {0, 0, 0, 0, 0, 0, 0, 3, 0, 0x80, 0x01, 0, 0, 0, 1, 0x7f};
    CHECK_INT(b.len, sizeof(expect));
    CHECK(memcmp(b.data, expect, sizeof(expect)) == 0);

    omt_span r = omt_span_of(b.data, b.len), mag;
    CHECK(sr_mpint_bytes(&r, &mag) && mag.len == 0);
    CHECK(sr_mpint_bytes(&r, &mag) && mag.len == 2 && mag.p[0] == 0x80);
    CHECK(sr_mpint_bytes(&r, &mag) && mag.len == 1 && mag.p[0] == 0x7f);
    CHECK(!sr_mpint_bytes(&r, &mag));
    const uint8_t negative[] = {0, 0, 0, 1, 0x80};
    r = omt_span_of(negative, sizeof(negative));
    CHECK(!sr_mpint_bytes(&r, &mag));
    omt_buf_free(&b);
}

static void strings_and_name_lists_are_bounded(void) {
    const uint8_t truncated[] = {0, 0, 0, 9, 'a', 'b'};
    omt_span r = omt_span_of(truncated, sizeof(truncated)), out;
    CHECK(!sr_string(&r, &out));
    const char list[] = "curve25519-sha256,ext-info-s,kex-strict-s-v00@openssh.com";
    omt_span l = omt_span_of(list, sizeof(list) - 1);
    CHECK(namelist_has(l, "ext-info-s"));
    CHECK(namelist_has(l, "kex-strict-s-v00@openssh.com"));
    CHECK(!namelist_has(l, "curve25519"));
    CHECK(!namelist_has(omt_span_of("", 0), "x"));
}

static void base64_is_strict(void) {
    omt_buf b;
    omt_buf_init(&b, 256);
    CHECK(ssh_base64_decode("aGVsbG8=", 8, false, &b));
    CHECK_STR(omt_buf_cstr(&b), "hello");
    omt_buf_clear(&b);
    CHECK(ssh_base64_decode("aGVs\nbG8=\n", 10, true, &b));
    CHECK_STR(omt_buf_cstr(&b), "hello");
    omt_buf_clear(&b);
    CHECK(!ssh_base64_decode("aGVsbG8", 7, false, &b));       /* unpadded */
    CHECK(!ssh_base64_decode("aGVs bG8=", 9, false, &b));     /* space not allowed */
    CHECK(!ssh_base64_decode("aGVsbG8=aa==", 12, false, &b)); /* data after padding */
    CHECK(!ssh_base64_decode("aGVsbG9=", 8, false, &b));      /* non-zero trailing bits */
    omt_buf_free(&b);
}

/* ---------------------------------------------------------------- ciphers */

/* Seals with one direction's state and opens with an identically keyed one,
 * as client and server would, for several packets in a row. */
static void cipher_round_trip(const char *cipher_name, const char *mac_name) {
    const ssh_cipher_alg *alg = ssh_cipher_find(omt_span_of(cipher_name, strlen(cipher_name)));
    const ssh_mac_alg *mac =
        mac_name ? ssh_mac_find(omt_span_of(mac_name, strlen(mac_name))) : NULL;
    CHECK(alg != NULL);
    if (!alg) return;
    uint8_t key[64], iv[16], mk[64];
    CHECK(RAND_bytes(key, sizeof(key)) == 1 && RAND_bytes(iv, sizeof(iv)) == 1 &&
          RAND_bytes(mk, sizeof(mk)) == 1);
    ssh_cipher tx = {0}, rx = {0};
    CHECK(ssh_cipher_init(&tx, alg, mac, key, iv, mk, true));
    CHECK(ssh_cipher_init(&rx, alg, mac, key, iv, mk, false));
    size_t block = ssh_cipher_block(&tx);
    for (uint32_t seq = 7; seq < 12; seq++) {
        uint8_t packet[256 + 64], tag[64], plain[256];
        size_t payload = 20 + seq;
        size_t aad = ssh_cipher_aead(&tx) ? 4 : 0;
        size_t padding = block - (5 + payload - aad) % block;
        if (padding < 4) padding += block;
        size_t len = 4 + 1 + payload + padding;
        omt_put_be32(packet, (uint32_t)(len - 4));
        packet[4] = (uint8_t)padding;
        for (size_t i = 5; i < len; i++) packet[i] = (uint8_t)(i * seq);
        memcpy(plain, packet, len);
        CHECK(ssh_cipher_seal(&tx, seq, packet, len, tag));
        CHECK(memcmp(packet + 5, plain + 5, payload) != 0);
        uint8_t first[16];
        memcpy(first, packet, ssh_cipher_first_len(&rx));
        uint32_t got_len = 0;
        CHECK(ssh_cipher_length(&rx, seq, packet, &got_len));
        CHECK_INT(got_len, len - 4);
        uint8_t tampered[sizeof(packet)];
        memcpy(tampered, packet, len);
        CHECK(ssh_cipher_open(&rx, seq, packet, len, tag));
        CHECK(memcmp(packet + 4, plain + 4, len - 4) == 0);
        (void)tampered;
    }
    /* A flipped ciphertext bit is refused. */
    uint8_t packet[64], tag[64];
    memset(packet, 0, sizeof(packet));
    omt_put_be32(packet, 60);
    packet[4] = 10;
    CHECK(ssh_cipher_seal(&tx, 99, packet, 64, tag));
    packet[20] ^= 1;
    uint32_t len = 0;
    CHECK(ssh_cipher_length(&rx, 99, packet, &len));
    CHECK(!ssh_cipher_open(&rx, 99, packet, 64, tag));
    ssh_cipher_free(&tx);
    ssh_cipher_free(&rx);
}

static void every_cipher_and_mac_round_trips(void) {
    cipher_round_trip("chacha20-poly1305@openssh.com", NULL);
    cipher_round_trip("aes256-gcm@openssh.com", NULL);
    cipher_round_trip("aes128-gcm@openssh.com", NULL);
    cipher_round_trip("aes256-ctr", "hmac-sha2-512-etm@openssh.com");
    cipher_round_trip("aes192-ctr", "hmac-sha2-256-etm@openssh.com");
    cipher_round_trip("aes128-ctr", "hmac-sha2-512");
    cipher_round_trip("aes128-ctr", "hmac-sha2-256");
}

static void legacy_algorithms_are_not_offered(void) {
    CHECK(strstr(SSH_CIPHER_LIST, "cbc") == NULL);
    CHECK(strstr(SSH_CIPHER_LIST, "3des") == NULL);
    CHECK(strstr(SSH_MAC_LIST, "sha1") == NULL);
    CHECK(strstr(SSH_HOSTKEY_LIST, "ssh-rsa") == NULL);
    CHECK(strstr(SSH_HOSTKEY_LIST, "ssh-dss") == NULL);
    CHECK(ssh_cipher_find(omt_span_of("aes128-cbc", 10)) == NULL);
    CHECK(ssh_mac_find(omt_span_of("hmac-sha1", 9)) == NULL);
}

/* ------------------------------------------------------------ known_hosts */

static void blob_for(const char *type, uint8_t fill, omt_buf *out) {
    uint8_t key[32];
    memset(key, fill, sizeof(key));
    sb_cstring(out, type);
    sb_string(out, key, sizeof(key));
}

static void base64_of(const omt_buf *blob, char *out, size_t size) {
    int n = EVP_EncodeBlock((unsigned char *)out, blob->data, (int)blob->len);
    CHECK(n > 0 && (size_t)n < size);
}

static ssh_host_status check(const char *file, const char *host, uint16_t port,
                             const omt_buf *key) {
    ssh_host_status st = SSH_HOST_UNKNOWN;
    CHECK(ssh_known_hosts_parse(file, strlen(file), host, port, omt_span_of(key->data, key->len),
                                &st, NULL));
    return st;
}

static void known_hosts_matching_is_strict(void) {
    omt_buf key, other, ecdsa;
    omt_buf_init(&key, 256);
    omt_buf_init(&other, 256);
    omt_buf_init(&ecdsa, 256);
    blob_for("ssh-ed25519", 0x11, &key);
    blob_for("ssh-ed25519", 0x22, &other);
    blob_for("ecdsa-sha2-nistp256", 0x33, &ecdsa);
    char k64[128], o64[128], e64[128], file[2048];
    base64_of(&key, k64, sizeof(k64));
    base64_of(&other, o64, sizeof(o64));
    base64_of(&ecdsa, e64, sizeof(e64));

    omt_snprintf(file, sizeof(file), "# comment\n\npi.local ssh-ed25519 %s operator@desk\n", k64);
    CHECK_INT(check(file, "pi.local", 22, &key), SSH_HOST_KNOWN);
    CHECK_INT(check(file, "PI.LOCAL", 22, &key), SSH_HOST_KNOWN);
    CHECK_INT(check(file, "pi.local", 22, &other), SSH_HOST_CHANGED);
    CHECK_INT(check(file, "pi.local", 22, &ecdsa), SSH_HOST_UNKNOWN);
    CHECK_INT(check(file, "other.local", 22, &key), SSH_HOST_UNKNOWN);
    /* A non-default port is only ever `[host]:port`. */
    CHECK_INT(check(file, "pi.local", 2222, &key), SSH_HOST_UNKNOWN);
    omt_snprintf(file, sizeof(file), "[pi.local]:2222 ssh-ed25519 %s\r\n", k64);
    CHECK_INT(check(file, "pi.local", 2222, &key), SSH_HOST_KNOWN);

    /* Patterns, and a negation that excludes a host the wildcard admits. */
    omt_snprintf(file, sizeof(file), "*.local,!evil.local ssh-ed25519 %s\n", k64);
    CHECK_INT(check(file, "pi.local", 22, &key), SSH_HOST_KNOWN);
    CHECK_INT(check(file, "evil.local", 22, &key), SSH_HOST_UNKNOWN);
    omt_snprintf(file, sizeof(file), "10.0.0.? ssh-ed25519 %s\n", k64);
    CHECK_INT(check(file, "10.0.0.7", 22, &key), SSH_HOST_KNOWN);
    CHECK_INT(check(file, "10.0.0.17", 22, &key), SSH_HOST_UNKNOWN);

    /* Revocation wins over a match, for every host. */
    omt_snprintf(file, sizeof(file), "pi.local ssh-ed25519 %s\n@revoked * ssh-ed25519 %s\n", k64,
                 k64);
    CHECK_INT(check(file, "pi.local", 22, &key), SSH_HOST_REVOKED);
    /* CA lines are not trust for a plain key. */
    omt_snprintf(file, sizeof(file), "@cert-authority pi.local ssh-ed25519 %s\n", k64);
    CHECK_INT(check(file, "pi.local", 22, &key), SSH_HOST_UNKNOWN);
    /* Garbage lines are skipped, not fatal. */
    omt_snprintf(file, sizeof(file),
                 "pi.local\npi.local ssh-ed25519 !!!!\npi.local ssh-ed25519 %s\n", k64);
    CHECK_INT(check(file, "pi.local", 22, &key), SSH_HOST_KNOWN);

    /* The types a host is known by, for negotiation order. */
    omt_snprintf(file, sizeof(file), "pi.local ecdsa-sha2-nistp256 %s\npi.local ssh-ed25519 %s\n",
                 e64, o64);
    omt_buf types;
    omt_buf_init(&types, 256);
    ssh_host_status st;
    CHECK(ssh_known_hosts_parse(file, strlen(file), "pi.local", 22, omt_span_of(NULL, 0), &st,
                                &types));
    CHECK_STR(omt_buf_cstr(&types), "ecdsa-sha2-nistp256,ssh-ed25519");
    omt_buf_free(&types);
    omt_buf_free(&key);
    omt_buf_free(&other);
    omt_buf_free(&ecdsa);
}

static void hashed_entries_match_by_hmac(void) {
    /* |1|salt|HMAC-SHA1(salt, "pi.local"), computed here with OpenSSL. */
    uint8_t salt[20];
    for (int i = 0; i < 20; i++) salt[i] = (uint8_t)(i + 1);
    uint8_t mac[20];
    unsigned int mac_len = 0;
    EVP_MAC *impl = EVP_MAC_fetch(NULL, "HMAC", NULL);
    EVP_MAC_CTX *ctx = EVP_MAC_CTX_new(impl);
    char digest[] = "SHA1";
    OSSL_PARAM params[] = {OSSL_PARAM_utf8_string("digest", digest, 0), OSSL_PARAM_END};
    size_t out_len = 0;
    CHECK(EVP_MAC_init(ctx, salt, 20, params) == 1 &&
          EVP_MAC_update(ctx, (const uint8_t *)"pi.local", 8) == 1 &&
          EVP_MAC_final(ctx, mac, &out_len, 20) == 1);
    (void)mac_len;
    EVP_MAC_CTX_free(ctx);
    EVP_MAC_free(impl);
    char s64[64], m64[64], k64[128], file[1024];
    EVP_EncodeBlock((unsigned char *)s64, salt, 20);
    EVP_EncodeBlock((unsigned char *)m64, mac, 20);
    omt_buf key;
    omt_buf_init(&key, 256);
    blob_for("ssh-ed25519", 0x44, &key);
    base64_of(&key, k64, sizeof(k64));
    omt_snprintf(file, sizeof(file), "|1|%s|%s ssh-ed25519 %s\n", s64, m64, k64);
    CHECK_INT(check(file, "pi.local", 22, &key), SSH_HOST_KNOWN);
    CHECK_INT(check(file, "pi.locak", 22, &key), SSH_HOST_UNKNOWN);
    omt_buf_free(&key);
}

/* ---------------------------------------------------------------- bcrypt */

static void bcrypt_pbkdf_matches_the_published_vector(void) {
    /* pyca/bcrypt's kdf test vector: password "password", salt "salt", 4
     * rounds, 32 bytes. */
    static const uint8_t expect[32] = {
        0x5b, 0xbf, 0x0c, 0xc2, 0x93, 0x58, 0x7f, 0x1c, 0x36, 0x35, 0x55,
        0x5c, 0x27, 0x79, 0x65, 0x98, 0xd4, 0x7e, 0x57, 0x90, 0x71, 0xbf,
        0x42, 0x7e, 0x9d, 0x8f, 0xbe, 0x84, 0x2a, 0xba, 0x34, 0xd9,
    };
    uint8_t key[32];
    CHECK(ssh_bcrypt_pbkdf((const uint8_t *)"password", 8, (const uint8_t *)"salt", 4, key, 32, 4));
    CHECK(memcmp(key, expect, 32) == 0);
    CHECK(!ssh_bcrypt_pbkdf((const uint8_t *)"", 0, (const uint8_t *)"salt", 4, key, 32, 4));
    CHECK(!ssh_bcrypt_pbkdf((const uint8_t *)"pw", 2, (const uint8_t *)"salt", 4, key, 32, 0));
}

static void blowfish_tables_are_pi(void) {
    CHECK_INT(ssh_blowfish_init_p[0], 0x243f6a88u);
    CHECK_INT(ssh_blowfish_init_s[3][255], 0x3ac372e6u);
}

/* ----------------------------------------------------------------- keys */

static void sign_and_verify(const char *keygen, const char *alg) {
    EVP_PKEY *pkey = strcmp(keygen, "RSA") == 0 ? EVP_PKEY_Q_keygen(NULL, NULL, "RSA", (size_t)2048)
                     : strcmp(keygen, "EC") == 0 ? EVP_PKEY_Q_keygen(NULL, NULL, "EC", "P-384")
                                                 : EVP_PKEY_Q_keygen(NULL, NULL, keygen);
    CHECK(pkey != NULL);
    if (!pkey) return;
    BIO *bio = BIO_new(BIO_s_mem());
    CHECK(PEM_write_bio_PrivateKey(bio, pkey, NULL, NULL, 0, NULL, NULL) == 1);
    char *pem = NULL;
    long pem_len = BIO_get_mem_data(bio, &pem);
    ssh_privkey key;
    dp_err err;
    dp_err_init(&err);
    bool loaded = ssh_privkey_parse((const uint8_t *)pem, (size_t)pem_len, "", 0, &key, &err);
    CHECK_MSG(loaded, "%s: %s", keygen, dp_err_text(&err));
    if (loaded) {
        const uint8_t data[] = "exchange hash stand-in";
        omt_buf sig;
        omt_buf_init(&sig, 4096);
        CHECK(ssh_privkey_sign(&key, alg, data, sizeof(data), &sig, &err));
        ssh_pubkey pub;
        CHECK(ssh_pubkey_parse(omt_span_of(key.blob.data, key.blob.len), &pub, &err));
        CHECK_MSG(
            ssh_pubkey_verify(&pub, alg, omt_span_of(sig.data, sig.len), data, sizeof(data), &err),
            "%s", alg);
        uint8_t wrong[] = "exchange hash stand-iN";
        CHECK(!ssh_pubkey_verify(&pub, alg, omt_span_of(sig.data, sig.len), wrong, sizeof(wrong),
                                 &err));
        ssh_pubkey_free(&pub);
        omt_buf_free(&sig);
        ssh_privkey_free(&key);
    }
    dp_err_free(&err);
    BIO_free(bio);
    EVP_PKEY_free(pkey);
}

static void every_user_key_type_signs_what_its_public_half_verifies(void) {
    sign_and_verify("ED25519", "ssh-ed25519");
    sign_and_verify("EC", "ecdsa-sha2-nistp384");
    sign_and_verify("RSA", "rsa-sha2-512");
    sign_and_verify("RSA", "rsa-sha2-256");
}

static void short_rsa_keys_are_refused(void) {
    EVP_PKEY *pkey = EVP_PKEY_Q_keygen(NULL, NULL, "RSA", (size_t)1024);
    BIO *bio = BIO_new(BIO_s_mem());
    CHECK(pkey && PEM_write_bio_PrivateKey(bio, pkey, NULL, NULL, 0, NULL, NULL) == 1);
    char *pem = NULL;
    long pem_len = BIO_get_mem_data(bio, &pem);
    ssh_privkey key;
    dp_err err;
    dp_err_init(&err);
    CHECK(!ssh_privkey_parse((const uint8_t *)pem, (size_t)pem_len, "", 0, &key, &err));
    CHECK(strstr(dp_err_text(&err), "2048") != NULL);
    dp_err_free(&err);
    BIO_free(bio);
    EVP_PKEY_free(pkey);
}

static void malformed_openssh_keys_are_refused(void) {
/* The markers are split so a secret scanner reading this file does not take
 * the malformed fixtures for keys. */
#define BEGIN_KEY                                                                                  \
    "-----BEGIN OPENSSH PRIVATE KEY"                                                               \
    "-----\n"
#define END_KEY                                                                                    \
    "-----END OPENSSH PRIVATE KEY"                                                                 \
    "-----\n"
    const char *cases[] = {
        BEGIN_KEY END_KEY,          BEGIN_KEY "b3BlbnNzaC1rZXktdjEA\n" END_KEY,
        BEGIN_KEY "!!!!\n" END_KEY, BEGIN_KEY,
        "not a key at all",
    };
    for (size_t i = 0; i < OMT_ARRAY_LEN(cases); i++) {
        ssh_privkey key;
        dp_err err;
        dp_err_init(&err);
        CHECK_MSG(
            !ssh_privkey_parse((const uint8_t *)cases[i], strlen(cases[i]), "", 0, &key, &err),
            "case %zu", i);
        dp_err_free(&err);
    }
}

static void signatures_for_another_algorithm_are_refused(void) {
    EVP_PKEY *pkey = EVP_PKEY_Q_keygen(NULL, NULL, "ED25519");
    BIO *bio = BIO_new(BIO_s_mem());
    CHECK(pkey && PEM_write_bio_PrivateKey(bio, pkey, NULL, NULL, 0, NULL, NULL) == 1);
    char *pem = NULL;
    long pem_len = BIO_get_mem_data(bio, &pem);
    ssh_privkey key;
    dp_err err;
    dp_err_init(&err);
    if (ssh_privkey_parse((const uint8_t *)pem, (size_t)pem_len, "", 0, &key, &err)) {
        omt_buf sig;
        omt_buf_init(&sig, 512);
        const uint8_t data[] = "h";
        CHECK(ssh_privkey_sign(&key, "ssh-ed25519", data, 1, &sig, &err));
        ssh_pubkey pub;
        CHECK(ssh_pubkey_parse(omt_span_of(key.blob.data, key.blob.len), &pub, &err));
        /* The negotiated algorithm and the signature's own name must agree. */
        CHECK(!ssh_pubkey_verify(&pub, "rsa-sha2-512", omt_span_of(sig.data, sig.len), data, 1,
                                 &err));
        ssh_pubkey_free(&pub);
        omt_buf_free(&sig);
        ssh_privkey_free(&key);
    }
    dp_err_free(&err);
    BIO_free(bio);
    EVP_PKEY_free(pkey);
}

int main(void) {
    RUN(mpints_are_minimal_and_non_negative);
    RUN(strings_and_name_lists_are_bounded);
    RUN(base64_is_strict);
    RUN(every_cipher_and_mac_round_trips);
    RUN(legacy_algorithms_are_not_offered);
    RUN(known_hosts_matching_is_strict);
    RUN(hashed_entries_match_by_hmac);
    RUN(bcrypt_pbkdf_matches_the_published_vector);
    RUN(blowfish_tables_are_pi);
    RUN(every_user_key_type_signs_what_its_public_half_verifies);
    RUN(short_rsa_keys_are_refused);
    RUN(malformed_openssh_keys_are_refused);
    RUN(signatures_for_another_algorithm_are_refused);
    return TEST_EXIT();
}
