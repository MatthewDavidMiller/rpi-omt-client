/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Internals of the first-party SSH client: wire encoding, the binary packet
 * transport, key exchange, host keys, user keys, authentication, channels, and
 * SFTP. Every primitive comes from OpenSSL's libcrypto; this code only speaks
 * the protocol around them (RFC 4250-4254, 4419, 5656, 8308, 8332, 8709, 8731,
 * the OpenSSH PROTOCOL extensions, and the strict key exchange that closes the
 * Terrapin prefix-truncation attack).
 */
#ifndef DP_SSH_INTERNAL_H
#define DP_SSH_INTERNAL_H

#include <openssl/bn.h>
#include <openssl/evp.h>

#include "common/buf.h"
#include "deploy/core/deploy.h"
#include "deploy/ssh/ssh.h"

/* Message numbers. */
enum {
    SSH_MSG_DISCONNECT = 1,
    SSH_MSG_IGNORE = 2,
    SSH_MSG_UNIMPLEMENTED = 3,
    SSH_MSG_DEBUG = 4,
    SSH_MSG_SERVICE_REQUEST = 5,
    SSH_MSG_SERVICE_ACCEPT = 6,
    SSH_MSG_EXT_INFO = 7,
    SSH_MSG_KEXINIT = 20,
    SSH_MSG_NEWKEYS = 21,
    SSH_MSG_KEX_30 = 30, /* KEXDH_INIT, KEX_ECDH_INIT, KEX_HYBRID_INIT */
    SSH_MSG_KEX_31 = 31, /* KEXDH_REPLY, KEX_ECDH_REPLY, KEX_DH_GEX_GROUP */
    SSH_MSG_KEX_DH_GEX_INIT = 32,
    SSH_MSG_KEX_DH_GEX_REPLY = 33,
    SSH_MSG_KEX_DH_GEX_REQUEST = 34,
    SSH_MSG_USERAUTH_REQUEST = 50,
    SSH_MSG_USERAUTH_FAILURE = 51,
    SSH_MSG_USERAUTH_SUCCESS = 52,
    SSH_MSG_USERAUTH_BANNER = 53,
    SSH_MSG_USERAUTH_60 = 60, /* PK_OK, PASSWD_CHANGEREQ, INFO_REQUEST */
    SSH_MSG_USERAUTH_INFO_RESPONSE = 61,
    SSH_MSG_GLOBAL_REQUEST = 80,
    SSH_MSG_REQUEST_SUCCESS = 81,
    SSH_MSG_REQUEST_FAILURE = 82,
    SSH_MSG_CHANNEL_OPEN = 90,
    SSH_MSG_CHANNEL_OPEN_CONFIRMATION = 91,
    SSH_MSG_CHANNEL_OPEN_FAILURE = 92,
    SSH_MSG_CHANNEL_WINDOW_ADJUST = 93,
    SSH_MSG_CHANNEL_DATA = 94,
    SSH_MSG_CHANNEL_EXTENDED_DATA = 95,
    SSH_MSG_CHANNEL_EOF = 96,
    SSH_MSG_CHANNEL_CLOSE = 97,
    SSH_MSG_CHANNEL_REQUEST = 98,
    SSH_MSG_CHANNEL_SUCCESS = 99,
    SSH_MSG_CHANNEL_FAILURE = 100,
};

/* The largest packet either side may send: OpenSSH's own ceiling. */
#define SSH_MAX_PACKET (256u * 1024u)
#define SSH_CONNECT_TIMEOUT_MS 15000u
#define SSH_IDLE_TIMEOUT_MS 60000u
#define SSH_COMMAND_TIMEOUT_MS (30u * 60u * 1000u)
#define SSH_UPLOAD_TIMEOUT_MS (30u * 60u * 1000u)

/* ----------------------------------------------------------------- wire */
void sb_u8(omt_buf *b, uint8_t v);
void sb_bool(omt_buf *b, bool v);
void sb_u32(omt_buf *b, uint32_t v);
void sb_u64(omt_buf *b, uint64_t v);
void sb_string(omt_buf *b, const void *data, size_t len);
void sb_cstring(omt_buf *b, const char *s);
/* An mpint from an unsigned big-endian magnitude. */
void sb_mpint_bytes(omt_buf *b, const uint8_t *be, size_t len);
bool sb_mpint_bn(omt_buf *b, const BIGNUM *bn);

OMT_NODISCARD bool sr_u8(omt_span *s, uint8_t *v);
OMT_NODISCARD bool sr_bool(omt_span *s, bool *v);
OMT_NODISCARD bool sr_u32(omt_span *s, uint32_t *v);
OMT_NODISCARD bool sr_u64(omt_span *s, uint64_t *v);
OMT_NODISCARD bool sr_string(omt_span *s, omt_span *out);
/* A non-negative mpint as a BIGNUM (caller frees). */
OMT_NODISCARD bool sr_mpint_bn(omt_span *s, BIGNUM **out);
/* A non-negative mpint as its magnitude, leading zero bytes removed. */
OMT_NODISCARD bool sr_mpint_bytes(omt_span *s, omt_span *out);
bool span_is(omt_span s, const char *text);
/* Strict base64 (RFC 4648 with padding); whitespace is skipped on request. */
bool ssh_base64_decode(const char *text, size_t len, bool skip_space, omt_buf *out);
/* Whether `name` is an entry of a comma-separated name-list. */
bool namelist_has(omt_span list, const char *name);

/* ------------------------------------------------------------- ciphers */
typedef enum { SSH_CIPHER_NONE, SSH_CIPHER_CHACHA, SSH_CIPHER_GCM, SSH_CIPHER_CTR } ssh_cipher_kind;

typedef struct {
    const char *name;
    ssh_cipher_kind kind;
    size_t key_len;
    size_t iv_len;
    size_t block;
} ssh_cipher_alg;

typedef struct {
    const char *name;
    const char *digest; /* OpenSSL digest name */
    size_t key_len;
    size_t mac_len;
    bool etm;
} ssh_mac_alg;

typedef struct {
    const ssh_cipher_alg *alg; /* NULL before the first NEWKEYS */
    const ssh_mac_alg *mac;    /* NULL for the AEAD ciphers */
    EVP_CIPHER_CTX *ctx;       /* CTR keystream, or GCM/ChaCha context */
    EVP_MAC *mac_impl;
    uint8_t key[64];
    uint8_t iv[16];
    uint8_t mac_key[64];
} ssh_cipher;

const ssh_cipher_alg *ssh_cipher_find(omt_span name);
const ssh_mac_alg *ssh_mac_find(omt_span name);
extern const char SSH_CIPHER_LIST[];
extern const char SSH_MAC_LIST[];
bool ssh_cipher_init(ssh_cipher *c, const ssh_cipher_alg *alg, const ssh_mac_alg *mac,
                     const uint8_t *key, const uint8_t *iv, const uint8_t *mac_key, bool encrypt);
void ssh_cipher_free(ssh_cipher *c);
size_t ssh_cipher_block(const ssh_cipher *c);
size_t ssh_cipher_tag_len(const ssh_cipher *c);
bool ssh_cipher_aead(const ssh_cipher *c);
/* Seals packet[0..len) in place (length field included) and appends the tag
 * or MAC to `tag`. */
bool ssh_cipher_seal(ssh_cipher *c, uint32_t seq, uint8_t *packet, size_t len, uint8_t *tag);
/* The packet length from the first bytes received; `first` holds at least
 * ssh_cipher_first_len() bytes, and for CTR without ETM is decrypted in place. */
size_t ssh_cipher_first_len(const ssh_cipher *c);
bool ssh_cipher_length(ssh_cipher *c, uint32_t seq, uint8_t *first, uint32_t *len);
/* Verifies and decrypts a whole packet (length field, body, tag). After this
 * the body starting at packet+4 is plaintext. */
bool ssh_cipher_open(ssh_cipher *c, uint32_t seq, uint8_t *packet, size_t len, const uint8_t *tag);

/* ------------------------------------------------------------ host keys */
typedef struct {
    int type; /* EVP_PKEY_ED25519, EVP_PKEY_EC, EVP_PKEY_RSA */
    EVP_PKEY *pkey;
    char name[32]; /* key type as named in the blob */
    const char *digest;
} ssh_pubkey;

/* Parses a public key blob of a supported type. */
bool ssh_pubkey_parse(omt_span blob, ssh_pubkey *out, dp_err *err);
void ssh_pubkey_free(ssh_pubkey *k);
/* Verifies an SSH signature blob made with `alg` over data. */
bool ssh_pubkey_verify(const ssh_pubkey *k, const char *alg, omt_span sig, const uint8_t *data,
                       size_t len, dp_err *err);
/* The host-key algorithms that verify keys of a given blob type. */
bool ssh_hostkey_alg_matches(const char *alg, const char *key_type);
extern const char SSH_HOSTKEY_LIST[];

/* ------------------------------------------------------------ known_hosts */
typedef enum {
    SSH_HOST_KNOWN,
    SSH_HOST_UNKNOWN,
    SSH_HOST_CHANGED,
    SSH_HOST_REVOKED,
} ssh_host_status;

/* Looks up `host` (port 22) or `[host]:port` in an OpenSSH known_hosts file. */
bool ssh_known_hosts_check(const char *path, const char *host, uint16_t port, omt_span key_blob,
                           ssh_host_status *status, dp_err *err);
/* The key types known_hosts lists for the host, comma-separated in file
 * order, so negotiation can prefer an algorithm whose key is already trusted. */
bool ssh_known_hosts_types(const char *path, const char *host, uint16_t port, omt_buf *types,
                           dp_err *err);
/* Exposed for the unit tests and the fuzzer. */
bool ssh_known_hosts_parse(const char *text, size_t len, const char *host, uint16_t port,
                           omt_span key_blob, ssh_host_status *status, omt_buf *types);

/* ------------------------------------------------------------ user keys */
typedef struct {
    EVP_PKEY *pkey;
    char type[32]; /* ssh-ed25519, ecdsa-sha2-nistp256, ssh-rsa, ... */
    omt_buf blob;  /* public key blob */
    const char *ec_digest;
} ssh_privkey;

/* Loads an OpenSSH (openssh-key-v1) or PEM private key. */
bool ssh_privkey_load(const char *path, const char *passphrase, size_t passphrase_len,
                      ssh_privkey *out, dp_err *err);
bool ssh_privkey_parse(const uint8_t *data, size_t len, const char *passphrase,
                       size_t passphrase_len, ssh_privkey *out, dp_err *err);
void ssh_privkey_free(ssh_privkey *k);
/* Appends an SSH signature blob for `alg` over data. */
bool ssh_privkey_sign(const ssh_privkey *k, const char *alg, const uint8_t *data, size_t len,
                      omt_buf *sig, dp_err *err);

/* bcrypt_pbkdf as OpenBSD defines it, for encrypted OpenSSH keys. */
bool ssh_bcrypt_pbkdf(const uint8_t *pass, size_t pass_len, const uint8_t *salt, size_t salt_len,
                      uint8_t *key, size_t key_len, uint32_t rounds);

/* ------------------------------------------------------------ transport */
typedef struct {
    dp_sock sock;
    const dp_cancel *cancel;
    char *host;
    uint16_t port;
    char *known_hosts;

    omt_buf v_c, v_s; /* identification strings, without CR LF */
    omt_buf i_c, i_s; /* KEXINIT payloads */
    omt_buf session_id;
    omt_buf host_key; /* the host key blob verified at the first exchange */
    omt_buf preferred_hostkeys;

    uint32_t send_seq, recv_seq;
    ssh_cipher send, recv;
    bool strict_kex;
    bool first_kex_done;
    bool ext_info;
    omt_buf server_sig_algs;

    omt_buf rbuf;  /* bytes received and not yet consumed */
    bool have_len; /* the current packet's length is known */
    uint32_t pkt_len;
    omt_buf payload; /* the most recent packet's payload */
    omt_buf sbuf;    /* scratch for building packets */

    uint64_t idle_ms; /* how long a read may wait for the peer */
} ssh_transport;

typedef enum { SSH_RECV_OK, SSH_RECV_TIMEOUT, SSH_RECV_ERROR } ssh_recv_status;

void ssh_transport_init(ssh_transport *t);
void ssh_transport_free(ssh_transport *t);
/* TCP connect, version exchange, and the first key exchange with host-key
 * verification, all within the connect timeout. */
bool ssh_transport_open(ssh_transport *t, const dp_connection *c, const dp_cancel *cancel,
                        dp_err *err);
/* Sends one packet with the given payload. */
bool ssh_send(ssh_transport *t, const omt_buf *payload, dp_err *err);
/* Waits up to `wait_ms` for the next packet the caller should see. Transport
 * messages (ignore, debug, a re-key, a global request) are handled here. */
ssh_recv_status ssh_recv(ssh_transport *t, uint32_t wait_ms, dp_err *err);
/* Like ssh_recv, but waits out the idle timeout and fails on silence. */
bool ssh_recv_wait(ssh_transport *t, dp_err *err);
void ssh_disconnect(ssh_transport *t, uint32_t reason, const char *text);
/* The next packet as received, with no transport handling, by an absolute
 * dp_now_ms deadline. The payload is in t->payload. */
ssh_recv_status ssh_read_packet(ssh_transport *t, uint64_t deadline_ms, dp_err *err);
/* Records the server's EXT_INFO (RFC 8308). */
bool ssh_handle_ext_info(ssh_transport *t, dp_err *err);

/* key exchange (kex.c) */
bool ssh_kex_run(ssh_transport *t, bool server_kexinit_received, dp_err *err);

/* -------------------------------------------------------------- session */
struct ssh_session {
    ssh_transport t;
    uint32_t next_channel;
};

bool ssh_authenticate(ssh_transport *t, const dp_connection *c, dp_err *err);

/* ------------------------------------------------------------- channels */
#define SSH_LOCAL_WINDOW (2u * 1024u * 1024u)
#define SSH_LOCAL_MAX_PACKET 32768u

typedef struct {
    ssh_transport *t;
    uint32_t local_id, remote_id;
    uint64_t remote_window;
    uint32_t remote_max_packet;
    uint32_t consumed; /* received bytes not yet returned to the window */
    bool opened, open_failed;
    bool eof_received, close_received, close_sent, eof_sent;
    int reply; /* 0 none pending, 1 success, -1 failure */
    bool has_exit;
    int exit_code;
    omt_buf *out, *err; /* where data and extended data go */
    size_t out_limit;   /* per stream; beyond it the channel fails */
    bool overflow;
} ssh_channel;

/* The bytes an upload sends: a file or memory, rewindable for the fallback. */
typedef struct {
    const uint8_t *mem;
    size_t len, pos;
    dp_file *file;
} ssh_source;

long ssh_source_read(ssh_source *s, uint8_t *buf, size_t len, dp_err *err);
bool ssh_source_rewind(ssh_source *s, dp_err *err);

bool ssh_channel_open(ssh_channel *ch, ssh_session *s, omt_buf *out, omt_buf *err_sink,
                      size_t limit, dp_err *err);
/* Applies the packet in t->payload to the channel. Returns false on a
 * protocol error. */
bool ssh_channel_dispatch(ssh_channel *ch, dp_err *err);
/* Sends a channel request and, when asked, waits for its reply. */
bool ssh_channel_request(ssh_channel *ch, const char *type, bool want_reply, const omt_buf *extra,
                         uint32_t timeout_ms, bool *accepted, dp_err *err);
/* Sends data within the peer's window, servicing incoming packets while it
 * waits for the window to open. */
bool ssh_channel_write(ssh_channel *ch, const uint8_t *data, size_t len, uint64_t deadline_ms,
                       const dp_cancel *cancel, dp_err *err);
bool ssh_channel_eof(ssh_channel *ch, dp_err *err);
void ssh_channel_close(ssh_channel *ch);
/* Waits up to wait_ms for one packet and dispatches it. Returns 1 when a
 * packet arrived, 0 on timeout, -1 on error. */
int ssh_channel_poll(ssh_channel *ch, uint32_t wait_ms, dp_err *err);

bool ssh_sftp_upload(ssh_session *s, ssh_source *src, const char *remote, const dp_cancel *cancel,
                     dp_err *err);
bool ssh_shell_upload(ssh_session *s, ssh_source *src, const char *remote, const dp_cancel *cancel,
                      dp_err *err);

#endif
