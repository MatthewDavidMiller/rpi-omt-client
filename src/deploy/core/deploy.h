/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The deployer core: validation, the embedded capsule, and the operations both
 * frontends (the CLI and the terminal application) run. Nothing here draws or
 * prompts; progress is reported through a callback and failures through a
 * dp_err.
 *
 * Validation functions return NULL when a value is acceptable and the message
 * to show the operator when it is not.
 */
#ifndef DP_DEPLOY_H
#define DP_DEPLOY_H

#include "common/base.h"
#include "common/buf.h"
#include "deploy/core/sys.h"

#define DP_MAX_SECRET_BYTES 4096u
#define DP_MAX_MANIFEST_MEMBERS 128u
#define DP_MAX_MANIFEST_MEMBER_BYTES 240u
#define DP_MIN_WEB_PASSWORD_BYTES 12u
#define DP_MAX_WEB_PASSWORD_BYTES 128u
#define DP_MIN_OS_PASSWORD_BYTES 8u
#define DP_MAX_OS_PASSWORD_BYTES 128u
#define DP_IMAGE_MEMBER "omt-client-arm64.tar.gz"

/* ---------------------------------------------------------------- secrets
 * A credential, wiped when cleared. `set` distinguishes an absent secret
 * from an explicitly empty one: factory Alpine accepts root with an empty SSH
 * password, so "" is a real credential there. */
typedef struct {
    bool set;
    omt_buf value;
} dp_secret;

void dp_secret_init(dp_secret *s);
void dp_secret_clear(dp_secret *s);
/* Validates (at most 4096 bytes of UTF-8 with no control characters) and
 * stores a copy. Returns the validation message on failure. */
const char *dp_secret_assign(dp_secret *s, const char *text, size_t len);
const char *dp_secret_assign_cstr(dp_secret *s, const char *text);
static inline const char *dp_secret_text(const dp_secret *s) {
    return s->set ? omt_buf_cstr(&s->value) : "";
}
static inline size_t dp_secret_len(const dp_secret *s) { return s->set ? s->value.len : 0; }
static inline bool dp_secret_nonempty(const dp_secret *s) { return s->set && s->value.len > 0; }
/* Copies another secret (absent stays absent). */
void dp_secret_copy(dp_secret *dst, const dp_secret *src);

/* Whether text contains a Unicode control character (Cc) or is not UTF-8. */
bool dp_has_control(const char *text, size_t len);
/* Number of code points, for cursor arithmetic on UTF-8 text. */
size_t dp_utf8_count(const char *text, size_t len);

/* ------------------------------------------------------------- settings */
typedef enum { DP_AUTH_PASSWORD, DP_AUTH_KEY } dp_auth;

typedef enum {
    DP_ACTION_STATUS,
    DP_ACTION_LOGS,
    DP_ACTION_RESTART,
    DP_ACTION_REBOOT,
} dp_action;

/* The fixed argv each management action runs, NULL-terminated. */
const char *const *dp_action_argv(dp_action action);

typedef struct {
    char *host;
    char *username;
    uint16_t port;
    dp_auth auth;
    dp_secret password;
    char *key_path; /* NULL when absent */
    dp_secret key_passphrase;
    char *known_hosts_path; /* NULL uses ~/.ssh/known_hosts */
    dp_secret sudo_password;
    /* Root password used only to bootstrap untouched Alpine through `su`. */
    dp_secret bootstrap_root_password;
} dp_connection;

void dp_connection_init(dp_connection *c);
void dp_connection_free(dp_connection *c);
bool dp_connection_copy(dp_connection *dst, const dp_connection *src);

typedef struct {
    char *project_root; /* NULL deploys the embedded capsule */
    char *remote_directory;
    bool rebuild_image;
} dp_deploy_options;

void dp_deploy_options_free(dp_deploy_options *o);

typedef struct {
    char *ssid;
    dp_secret password;
    bool connect;
    bool preserve_existing_profiles;
} dp_wifi_settings;

void dp_wifi_settings_free(dp_wifi_settings *w);

typedef struct {
    char *hostname;
    bool has_wifi;
    dp_wifi_settings wifi;
    dp_secret root_password;
    dp_secret pi_password;
} dp_alpine_settings;

void dp_alpine_settings_free(dp_alpine_settings *a);

typedef struct {
    char *boot_directory;
    char *country;
    char *wifi_ssid;
    dp_secret wifi_password;
} dp_sd_settings;

void dp_sd_settings_free(dp_sd_settings *s);

/* ------------------------------------------------------------ validation */
bool dp_valid_host(const char *value);
bool dp_valid_username(const char *value);
bool dp_valid_appliance_hostname(const char *value);
bool dp_valid_remote_directory(const char *value);
bool dp_valid_manifest_name(const char *value);
const char *dp_validate_connection(const dp_connection *c);
const char *dp_validate_options(const dp_deploy_options *o);
const char *dp_validate_wifi(const dp_wifi_settings *w);
const char *dp_validate_os_password(const dp_secret *s);
const char *dp_validate_alpine_setup(const dp_alpine_settings *a);
const char *dp_validate_web_password(const dp_secret *s);

/* --------------------------------------------------------------- helpers */
/* The 64-hex-digit WPA PSK for a passphrase, stored in `out`. */
const char *dp_derive_wpa_psk(const char *ssid, const dp_secret *passphrase, dp_secret *out);
/* Appends value in single quotes, safe for a POSIX shell. */
void dp_shell_quote(omt_buf *out, const char *value);
void dp_hex_encode(omt_buf *out, const void *bytes, size_t len);
bool dp_random_token(omt_buf *out, size_t bytes, dp_err *err);
void dp_sha256_hex(const void *data, size_t len, char out[65]);
bool dp_sha256_file(const char *path, char out[65], dp_err *err);

/* A manifest's members, in order. */
typedef struct {
    char **names;
    size_t count;
} dp_manifest;

void dp_manifest_free(dp_manifest *m);
bool dp_parse_manifest(const char *text, size_t len, dp_manifest *out, dp_err *err);
bool dp_load_manifest(const char *path, dp_manifest *out, dp_err *err);
/* root joined with a member, after re-checking the member is safe. */
bool dp_secure_relative(const char *root, const char *member, omt_buf *out, dp_err *err);

/* ---------------------------------------------------------------- capsule */
typedef struct {
    const char *name;
    const uint8_t *bytes;
    size_t size;
} dp_capsule_member;

const dp_capsule_member *dp_capsule_members(size_t *count);
const dp_capsule_member *dp_capsule_member_named(const char *name);
const dp_capsule_member *dp_capsule_image(void);
/* The LICENSE and THIRD_PARTY_NOTICES.txt texts the release ships. */
const char *dp_license_text(size_t *len);
const char *dp_notices_text(size_t *len);
/* "Embedded capsule: N members, omt-client-arm64.tar.gz M MiB, sha256 H." */
bool dp_capsule_report(omt_buf *out, dp_err *err);

/* -------------------------------------------------------------- progress */
typedef struct {
    void (*line)(void *ctx, const char *text);
    void *ctx;
} dp_progress;

/* Reports text, which may span several lines. */
void dp_report(const dp_progress *p, const char *text);
void dp_reportf(const dp_progress *p, const char *fmt, ...) OMT_PRINTF(2, 3);

/* ------------------------------------------------------------- operations */
bool dp_test_connection(const dp_connection *c, const dp_cancel *cancel, const dp_progress *p,
                        dp_err *err);
bool dp_alpine_setup(const dp_connection *c, const dp_alpine_settings *settings,
                     const char *project_root, const dp_cancel *cancel, const dp_progress *p,
                     dp_err *err);
bool dp_manage(const dp_connection *c, dp_action action, const dp_cancel *cancel,
               const dp_progress *p, dp_err *err);
bool dp_set_hostname(const dp_connection *c, const char *hostname, const char *project_root,
                     const dp_cancel *cancel, const dp_progress *p, dp_err *err);
bool dp_change_web_password(const dp_connection *c, const dp_secret *password,
                            const dp_cancel *cancel, const dp_progress *p, dp_err *err);
bool dp_apply_wifi(const dp_connection *c, const dp_wifi_settings *settings,
                   const dp_cancel *cancel, const dp_progress *p, dp_err *err);
bool dp_deploy(const dp_connection *c, const dp_deploy_options *options, const dp_cancel *cancel,
               const dp_progress *p, dp_err *err);

/* ---------------------------------------------------------------- SD card */
#define DP_HEADLESS_FILE_NAME "headless.apkovl.tar.gz"
#define DP_HEADLESS_VERSION "v1.9"
const char *dp_validate_sd_settings(const dp_sd_settings *s, dp_err *err);
bool dp_wpa_supplicant_config(const dp_sd_settings *s, omt_buf *out, dp_err *err);
bool dp_prepare_sd_card(const dp_sd_settings *s, const dp_cancel *cancel, const dp_progress *p,
                        dp_err *err);

/* ------------------------------------------------------------------ tools */
typedef struct {
    const char *id;
    const char *name;
} dp_package;

extern const dp_package DP_GIT_FOR_WINDOWS;
extern const dp_package DP_DOCKER_DESKTOP;
extern const dp_package DP_PYTHON;

typedef struct {
    const char *name;
    const char *purpose;
    bool required;
    bool satisfied;
    char *detail;
    char *remedy;
    const dp_package *package; /* NULL when no winget package supplies it */
} dp_prerequisite;

static inline bool dp_prerequisite_blocking(const dp_prerequisite *row) {
    return row->required && !row->satisfied;
}

typedef struct {
    dp_prerequisite *rows;
    size_t count;
} dp_prerequisites;

void dp_prerequisites_free(dp_prerequisites *rows);
bool dp_probe_prerequisites(const char *project_root, const dp_cancel *cancel,
                            dp_prerequisites *out);
/* The distinct packages that would satisfy the unsatisfied rows. */
size_t dp_missing_packages(const dp_prerequisites *rows, const dp_package **out, size_t max);
bool dp_install_packages(const dp_package *const *packages, size_t count, const dp_cancel *cancel,
                         const dp_progress *p, dp_err *err);
bool dp_ensure_arm64_emulation(const dp_cancel *cancel, const dp_progress *p, dp_err *err);
/* Locates an executable on PATH, or checks a path that names one. */
bool dp_find_executable(const char *program, omt_buf *out);

typedef struct {
    char *program;
    char *args[4];
    char *env[2];
} dp_build_plan;

void dp_build_plan_free(dp_build_plan *plan);
bool dp_image_build_plan(dp_build_plan *plan, dp_err *err);
void dp_build_plan_summary(const dp_build_plan *plan, omt_buf *out);

/* ------------------------------------------------------------------- jobs */
typedef enum {
    DP_JOB_PREPARE_SD,
    DP_JOB_TEST,
    DP_JOB_ALPINE,
    DP_JOB_DEPLOY,
    DP_JOB_MANAGE,
    DP_JOB_WEB_PASSWORD,
    DP_JOB_HOSTNAME,
    DP_JOB_WIFI,
} dp_job_kind;

/* Everything a job needs, captured when it starts so later edits to the form
 * cannot change what it does. */
typedef struct {
    dp_job_kind job;
    dp_action action; /* for DP_JOB_MANAGE */
    bool has_connection;
    dp_connection connection;
    dp_deploy_options options;
    char *boot_directory;
    char *wifi_country;
    char *wifi_ssid;
    dp_secret wifi_password;
    bool wifi_connect;
    bool wifi_preserve_existing_profiles;
    char *hostname;
    char *manage_hostname;
    dp_secret os_root_password;
    dp_secret os_pi_password;
    bool rotate_web_password;
    dp_secret web_password;
} dp_job_request;

void dp_job_request_init(dp_job_request *r);
void dp_job_request_free(dp_job_request *r);

/* The connection fields the terminal application collects, as typed. */
typedef struct {
    const char *host;
    const char *username;
    const char *password;
    const char *sudo_password;
    const char *known_hosts;
    const char *bootstrap_root_password;
} dp_connection_fields;

/* Builds and validates a connection; returns the validation message or NULL. */
const char *dp_connection_from_fields(const dp_connection_fields *f, dp_connection *out);
bool dp_run_job(dp_job_request *r, const dp_cancel *cancel, const dp_progress *p, dp_err *err);

/* ----------------------------------------------------------- string utils */
char *dp_strdup(const char *s);
char *dp_strndup(const char *s, size_t len);
/* A copy of s with Unicode whitespace trimmed from both ends. */
char *dp_trim_dup(const char *s);

#endif
