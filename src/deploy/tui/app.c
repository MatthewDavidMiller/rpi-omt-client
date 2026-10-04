/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The terminal deployer's state and input handling, ported from the Rust
 * terminal application. The field set is the one docs/SETUP.md describes.
 */
#include "deploy/tui/app.h"

#include <limits.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

#include "common/json.h"

/* ------------------------------------------------------------------ tables */

static const slot CONNECTION_SLOTS[] = {SLOT_HOST,          SLOT_USER,        SLOT_PASSWORD,
                                        SLOT_SUDO_PASSWORD, SLOT_KNOWN_HOSTS, SLOT_TEST_BUTTON};
static const slot SD_SLOTS[] = {SLOT_BOOT_DIRECTORY, SLOT_SD_WIFI_COUNTRY, SLOT_SD_WIFI_SSID,
                                SLOT_SD_WIFI_PASSWORD, SLOT_PREPARE_SD_BUTTON};
static const slot ALPINE_SLOTS[] = {
    SLOT_ALPINE_HOSTNAME,      SLOT_ALPINE_ROOT_PASSWORD, SLOT_ALPINE_ROOT_CONFIRM,
    SLOT_ALPINE_PI_PASSWORD,   SLOT_ALPINE_PI_CONFIRM,    SLOT_ALPINE_WIFI_SSID,
    SLOT_ALPINE_WIFI_PASSWORD, SLOT_ALPINE_APPLY_LOGIN,   SLOT_ALPINE_BUTTON};
static const slot DEPLOY_SLOTS[] = {SLOT_REMOTE_DIRECTORY, SLOT_ROTATE_WEB_PASSWORD,
                                    SLOT_WEB_PASSWORD, SLOT_WEB_CONFIRM, SLOT_DEPLOY_BUTTON};
static const slot MANAGE_SLOTS[] = {
    SLOT_STATUS_BUTTON,       SLOT_LOGS_BUTTON,        SLOT_RESTART_BUTTON,
    SLOT_REBOOT_BUTTON,       SLOT_MANAGE_HOSTNAME,    SLOT_HOSTNAME_BUTTON,
    SLOT_MANAGE_WEB_PASSWORD, SLOT_MANAGE_WEB_CONFIRM, SLOT_WEB_PASSWORD_BUTTON};
static const slot WIFI_SLOTS[] = {SLOT_WIFI_SSID, SLOT_WIFI_PASSWORD, SLOT_WIFI_CONNECT,
                                  SLOT_WIFI_PRESERVE, SLOT_WIFI_BUTTON};

const char *view_title(view v) {
    static const char *const titles[] = {"Connection", "SD card", "Alpine",   "Deploy",
                                         "Manage",     "Wi-Fi",   "Activity", "About"};
    return v < VIEW_COUNT ? titles[v] : "";
}

const slot *view_slots(view v, size_t *count) {
    switch (v) {
    case VIEW_CONNECTION: *count = OMT_ARRAY_LEN(CONNECTION_SLOTS); return CONNECTION_SLOTS;
    case VIEW_SD_CARD: *count = OMT_ARRAY_LEN(SD_SLOTS); return SD_SLOTS;
    case VIEW_ALPINE: *count = OMT_ARRAY_LEN(ALPINE_SLOTS); return ALPINE_SLOTS;
    case VIEW_DEPLOY: *count = OMT_ARRAY_LEN(DEPLOY_SLOTS); return DEPLOY_SLOTS;
    case VIEW_MANAGE: *count = OMT_ARRAY_LEN(MANAGE_SLOTS); return MANAGE_SLOTS;
    case VIEW_WIFI: *count = OMT_ARRAY_LEN(WIFI_SLOTS); return WIFI_SLOTS;
    default: *count = 0; return NULL;
    }
}

typedef struct {
    const char *label;
    kind k;
    field f;
} slot_info;

static const slot_info SLOTS[SLOT_COUNT] = {
    [SLOT_HOST] = {"Host", KIND_TEXT, FIELD_HOST},
    [SLOT_USER] = {"Username", KIND_TEXT, FIELD_USER},
    [SLOT_PASSWORD] = {"SSH password", KIND_SECRET, FIELD_PASSWORD},
    [SLOT_SUDO_PASSWORD] = {"sudo password (optional)", KIND_SECRET, FIELD_SUDO_PASSWORD},
    [SLOT_KNOWN_HOSTS] = {"known_hosts path (optional)", KIND_TEXT, FIELD_KNOWN_HOSTS},
    [SLOT_TEST_BUTTON] = {"Test connection", KIND_BUTTON, FIELD_NONE},
    [SLOT_BOOT_DIRECTORY] = {"Alpine boot partition path", KIND_TEXT, FIELD_BOOT_DIRECTORY},
    [SLOT_SD_WIFI_COUNTRY] = {"Wi-Fi country", KIND_TEXT, FIELD_WIFI_COUNTRY},
    [SLOT_SD_WIFI_SSID] = {"Initial Wi-Fi SSID", KIND_TEXT, FIELD_WIFI_SSID},
    [SLOT_SD_WIFI_PASSWORD] = {"Initial Wi-Fi password", KIND_SECRET, FIELD_WIFI_PASSWORD},
    [SLOT_PREPARE_SD_BUTTON] = {"Prepare SD card", KIND_BUTTON, FIELD_NONE},
    [SLOT_ALPINE_HOSTNAME] = {"Appliance hostname", KIND_TEXT, FIELD_HOSTNAME},
    [SLOT_ALPINE_ROOT_PASSWORD] = {"New root password", KIND_SECRET, FIELD_OS_ROOT_PASSWORD},
    [SLOT_ALPINE_ROOT_CONFIRM] = {"Confirm root password", KIND_SECRET, FIELD_OS_ROOT_CONFIRM},
    [SLOT_ALPINE_PI_PASSWORD] = {"New pi password", KIND_SECRET, FIELD_OS_PI_PASSWORD},
    [SLOT_ALPINE_PI_CONFIRM] = {"Confirm pi password", KIND_SECRET, FIELD_OS_PI_CONFIRM},
    [SLOT_ALPINE_WIFI_SSID] = {"Wi-Fi SSID (optional)", KIND_TEXT, FIELD_WIFI_SSID},
    [SLOT_ALPINE_WIFI_PASSWORD] = {"Wi-Fi password", KIND_SECRET, FIELD_WIFI_PASSWORD},
    [SLOT_ALPINE_APPLY_LOGIN] = {"Use the new pi login after setup", KIND_TOGGLE, FIELD_NONE},
    [SLOT_ALPINE_BUTTON] = {"Run Alpine setup", KIND_BUTTON, FIELD_NONE},
    [SLOT_REMOTE_DIRECTORY] = {"Remote directory", KIND_TEXT, FIELD_REMOTE_DIRECTORY},
    [SLOT_ROTATE_WEB_PASSWORD] = {"Also set the Web GUI password", KIND_TOGGLE, FIELD_NONE},
    [SLOT_WEB_PASSWORD] = {"Web GUI password", KIND_SECRET, FIELD_WEB_PASSWORD},
    [SLOT_WEB_CONFIRM] = {"Confirm Web GUI password", KIND_SECRET, FIELD_WEB_CONFIRM},
    [SLOT_DEPLOY_BUTTON] = {"Deploy", KIND_BUTTON, FIELD_NONE},
    [SLOT_STATUS_BUTTON] = {"Status", KIND_BUTTON, FIELD_NONE},
    [SLOT_LOGS_BUTTON] = {"Logs", KIND_BUTTON, FIELD_NONE},
    [SLOT_RESTART_BUTTON] = {"Restart the appliance", KIND_BUTTON, FIELD_NONE},
    [SLOT_REBOOT_BUTTON] = {"Reboot the Raspberry Pi", KIND_BUTTON, FIELD_NONE},
    [SLOT_MANAGE_HOSTNAME] = {"Rename appliance to", KIND_TEXT, FIELD_MANAGE_HOSTNAME},
    [SLOT_HOSTNAME_BUTTON] = {"Apply hostname", KIND_BUTTON, FIELD_NONE},
    [SLOT_MANAGE_WEB_PASSWORD] = {"New Web GUI password", KIND_SECRET, FIELD_MANAGE_WEB_PASSWORD},
    [SLOT_MANAGE_WEB_CONFIRM] = {"Confirm Web GUI password", KIND_SECRET, FIELD_MANAGE_WEB_CONFIRM},
    [SLOT_WEB_PASSWORD_BUTTON] = {"Change Web GUI password", KIND_BUTTON, FIELD_NONE},
    [SLOT_WIFI_SSID] = {"SSID", KIND_TEXT, FIELD_WIFI_SSID},
    [SLOT_WIFI_PASSWORD] = {"Wi-Fi password", KIND_SECRET, FIELD_WIFI_PASSWORD},
    [SLOT_WIFI_CONNECT] = {"Connect now", KIND_TOGGLE, FIELD_NONE},
    [SLOT_WIFI_PRESERVE] = {"Keep other saved profiles", KIND_TOGGLE, FIELD_NONE},
    [SLOT_WIFI_BUTTON] = {"Apply Wi-Fi settings", KIND_BUTTON, FIELD_NONE},
};

const char *slot_label(slot s) { return SLOTS[s].label; }
kind slot_kind(slot s) { return SLOTS[s].k; }
field slot_field(slot s) { return SLOTS[s].f; }

static bool field_is_secret(field f) {
    return f == FIELD_PASSWORD || f == FIELD_SUDO_PASSWORD || f == FIELD_OS_ROOT_PASSWORD ||
           f == FIELD_OS_ROOT_CONFIRM || f == FIELD_OS_PI_PASSWORD || f == FIELD_OS_PI_CONFIRM ||
           f == FIELD_WEB_PASSWORD || f == FIELD_WEB_CONFIRM || f == FIELD_MANAGE_WEB_PASSWORD ||
           f == FIELD_MANAGE_WEB_CONFIRM || f == FIELD_WIFI_PASSWORD;
}

/* ----------------------------------------------------------------- worker */

/* Shared between the frontend and one job's thread. Whichever of the two lets
 * go last frees it, so a quit while a job runs cannot free what the worker
 * still writes to. */
struct worker {
    dp_mutex *lock;
    dp_cancel cancel;
    int refs;
    char **lines;
    size_t count, cap;
    bool finished;
    bool ok;
    char *error;
    dp_job_request request;
};

static worker *worker_new(void) {
    worker *w = calloc(1, sizeof(*w));
    if (!w) return NULL;
    w->lock = dp_mutex_new();
    if (!w->lock) {
        free(w);
        return NULL;
    }
    atomic_init(&w->cancel, false);
    w->refs = 2;
    dp_job_request_init(&w->request);
    return w;
}

static void worker_release(worker *w) {
    dp_mutex_lock(w->lock);
    bool last = --w->refs == 0;
    dp_mutex_unlock(w->lock);
    if (!last) return;
    for (size_t i = 0; i < w->count; i++) free(w->lines[i]);
    free(w->lines);
    free(w->error);
    dp_job_request_free(&w->request);
    dp_mutex_free(w->lock);
    free(w);
}

static void worker_line(void *ctx, const char *text) {
    worker *w = ctx;
    char *copy = dp_strdup(text);
    if (!copy) return;
    dp_mutex_lock(w->lock);
    if (w->count == w->cap) {
        size_t cap = w->cap ? w->cap * 2 : 64;
        char **grown = realloc(w->lines, cap * sizeof(*grown));
        if (!grown) {
            dp_mutex_unlock(w->lock);
            free(copy);
            return;
        }
        w->lines = grown;
        w->cap = cap;
    }
    w->lines[w->count++] = copy;
    dp_mutex_unlock(w->lock);
}

static void worker_main(void *raw) {
    worker *w = raw;
    dp_progress p = {worker_line, w};
    dp_err err;
    dp_err_init(&err);
    bool ok = dp_run_job(&w->request, &w->cancel, &p, &err);
    /* The request holds secrets; wipe them as soon as the job is done. */
    dp_job_request_free(&w->request);
    dp_job_request_init(&w->request);
    char *error = ok ? NULL : dp_strdup(dp_err_text(&err));
    dp_err_free(&err);
    dp_mutex_lock(w->lock);
    w->ok = ok;
    w->error = error;
    w->finished = true;
    dp_mutex_unlock(w->lock);
    worker_release(w);
}

worker *worker_finished_for_test(bool ok, const char *error) {
    worker *w = worker_new();
    if (!w) return NULL;
    w->refs = 1;
    w->finished = true;
    w->ok = ok;
    w->error = error ? dp_strdup(error) : NULL;
    return w;
}

worker *worker_running_for_test(void) {
    worker *w = worker_new();
    if (w) w->refs = 1;
    return w;
}

/* -------------------------------------------------------------------- app */

static void set_field(app *a, field f, const char *text) {
    omt_buf_clear(&a->fields[f]);
    omt_buf_puts(&a->fields[f], text);
}

void app_init(app *a) {
    memset(a, 0, sizeof(*a));
    omt_buf_init(&a->status, 64u * 1024u);
    omt_buf_puts(&a->status, "Ready.");
    for (int f = 0; f < FIELD_COUNT; f++) omt_buf_init(&a->fields[f], FIELD_LIMIT + 1);
    set_field(a, FIELD_HOST, "raspberrypi.local");
    set_field(a, FIELD_USER, "root");
    set_field(a, FIELD_WIFI_COUNTRY, "US");
    set_field(a, FIELD_HOSTNAME, "omt-client");
    set_field(a, FIELD_REMOTE_DIRECTORY, "/opt/omt-client");
    a->wifi_connect = true;
    a->wifi_preserve = true;
    a->follow_log = true;
    a->current = VIEW_CONNECTION;
    app_cursor_end(a);
}

void app_free(app *a) {
    for (int f = 0; f < FIELD_COUNT; f++) {
        if (field_is_secret((field)f)) {
            omt_buf_free_secret(&a->fields[f]);
        } else {
            omt_buf_free(&a->fields[f]);
        }
    }
    for (size_t i = 0; i < a->log_len; i++) free(a->log[i]);
    a->log_len = 0;
    omt_buf_free(&a->status);
    if (a->job) {
        worker_release(a->job);
        a->job = NULL;
    }
}

bool app_busy(const app *a) { return a->job != NULL; }

bool app_selected(const app *a, slot *out) {
    size_t n;
    const slot *slots = view_slots(a->current, &n);
    if (a->focus >= n) return false;
    *out = slots[a->focus];
    return true;
}

const char *app_value(const app *a, slot s) {
    field f = slot_field(s);
    return f == FIELD_NONE ? "" : omt_buf_cstr(&a->fields[f]);
}

static bool *toggle_ptr(app *a, slot s) {
    switch (s) {
    case SLOT_ALPINE_APPLY_LOGIN: return &a->apply_alpine_login;
    case SLOT_ROTATE_WEB_PASSWORD: return &a->rotate_web_password;
    case SLOT_WIFI_CONNECT: return &a->wifi_connect;
    case SLOT_WIFI_PRESERVE: return &a->wifi_preserve;
    default: return NULL;
    }
}

bool app_toggle(const app *a, slot s) {
    switch (s) {
    case SLOT_ALPINE_APPLY_LOGIN: return a->apply_alpine_login;
    case SLOT_ROTATE_WEB_PASSWORD: return a->rotate_web_password;
    case SLOT_WIFI_CONNECT: return a->wifi_connect;
    case SLOT_WIFI_PRESERVE: return a->wifi_preserve;
    default: return false;
    }
}

void app_set_status(app *a, const char *fmt, ...) {
    omt_buf_clear(&a->status);
    va_list args;
    va_start(args, fmt);
    omt_buf_vprintf(&a->status, fmt, args);
    va_end(args);
}

static size_t value_chars(const app *a, slot s) {
    const char *v = app_value(a, s);
    return dp_utf8_count(v, strlen(v));
}

void app_cursor_end(app *a) {
    slot s;
    a->cursor = app_selected(a, &s) ? value_chars(a, s) : 0;
}

void app_cursor_home(app *a) { a->cursor = 0; }

void app_select_view(app *a, view v) {
    a->current = v;
    a->focus = 0;
    /* A prefilled first field -- the default host, above all -- is edited
     * from its end, so Backspace clears it and typing does not land in front
     * of it. */
    app_cursor_end(a);
}

void app_move_focus(app *a, int delta) {
    size_t n;
    view_slots(a->current, &n);
    if (n == 0) return;
    long next = ((long)a->focus + delta) % (long)n;
    if (next < 0) next += (long)n;
    a->focus = (size_t)next;
    /* Land at the end of the newly focused text so typing continues it. */
    app_cursor_end(a);
}

void app_scroll_about(app *a, long delta) {
    long next = (long)(a->about_scroll > (size_t)LONG_MAX ? LONG_MAX : (long)a->about_scroll);
    if (delta > 0 && next > LONG_MAX - delta) {
        next = LONG_MAX;
    } else {
        next += delta;
    }
    a->about_scroll = next < 0 ? 0 : (size_t)next;
}

void app_push_log(app *a, const char *line) {
    char *copy = dp_strdup(line);
    if (!copy) return;
    if (a->log_len >= LOG_CAPACITY) {
        free(a->log[0]);
        memmove(a->log, a->log + 1, (a->log_len - 1) * sizeof(char *));
        a->log_len--;
        if (a->log_scroll > 0) a->log_scroll--;
    }
    a->log[a->log_len++] = copy;
}

void app_poll_worker(app *a) {
    worker *w = a->job;
    if (!w) return;
    dp_mutex_lock(w->lock);
    char **lines = w->lines;
    size_t count = w->count;
    w->lines = NULL;
    w->count = w->cap = 0;
    bool finished = w->finished, ok = w->ok;
    char *error = finished ? w->error : NULL;
    if (finished) w->error = NULL;
    dp_mutex_unlock(w->lock);
    for (size_t i = 0; i < count; i++) {
        app_push_log(a, lines[i]);
        free(lines[i]);
    }
    free(lines);
    if (!finished) return;
    worker_release(w);
    a->job = NULL;
    if (ok) {
        app_set_status(a, "Finished successfully.");
        app_push_log(a, "-- finished successfully --");
        if (a->switch_to_pi_after_job) {
            set_field(a, FIELD_USER, "pi");
            omt_buf_clear(&a->fields[FIELD_PASSWORD]);
            omt_buf_append(&a->fields[FIELD_PASSWORD], a->fields[FIELD_OS_PI_PASSWORD].data,
                           a->fields[FIELD_OS_PI_PASSWORD].len);
            omt_buf_clear(&a->fields[FIELD_SUDO_PASSWORD]);
            omt_buf_append(&a->fields[FIELD_SUDO_PASSWORD], a->fields[FIELD_OS_PI_PASSWORD].data,
                           a->fields[FIELD_OS_PI_PASSWORD].len);
            app_push_log(a, "Connection updated to user pi. Deploy next; the Alpine root password "
                            "installs sudo on first deploy.");
        }
    } else {
        const char *text = error ? error : "the worker stopped without reporting";
        app_set_status(a, "Failed: %s", text);
        omt_buf line;
        omt_buf_init(&line, DP_ERR_LIMIT);
        omt_buf_printf(&line, "-- failed: %s --", text);
        app_push_log(a, omt_buf_cstr(&line));
        omt_buf_free(&line);
    }
    free(error);
    a->switch_to_pi_after_job = false;
}

void app_cancel_job(app *a) {
    if (!a->job) return;
    atomic_store(&a->job->cancel, true);
    app_set_status(a, "Cancelling...");
    app_push_log(a, "-- cancellation requested --");
}

static const char *text(const app *a, field f) { return omt_buf_cstr(&a->fields[f]); }

static bool same_value(const app *a, field x, field y) {
    return a->fields[x].len == a->fields[y].len &&
           memcmp(omt_buf_cstr(&a->fields[x]), omt_buf_cstr(&a->fields[y]), a->fields[x].len) == 0;
}

/* The connection the remote jobs share, built by the same rules the Rust
 * frontends used: an always-present SSH password (factory Alpine answers root
 * with an empty one), optional sudo and known_hosts, and the Alpine view's
 * root password as the bootstrap secret. */
static const char *build_connection(const app *a, dp_connection *out) {
    dp_connection_fields f = {
        text(a, FIELD_HOST),          text(a, FIELD_USER),        text(a, FIELD_PASSWORD),
        text(a, FIELD_SUDO_PASSWORD), text(a, FIELD_KNOWN_HOSTS), text(a, FIELD_OS_ROOT_PASSWORD),
    };
    return dp_connection_from_fields(&f, out);
}

static char *trimmed(const app *a, field f) { return dp_trim_dup(text(a, f)); }

const char *app_precheck(const app *a, dp_job_kind job, dp_err *err) {
    switch (job) {
    case DP_JOB_PREPARE_SD: {
        dp_sd_settings s = {trimmed(a, FIELD_BOOT_DIRECTORY),
                            trimmed(a, FIELD_WIFI_COUNTRY),
                            trimmed(a, FIELD_WIFI_SSID),
                            {false, {0}}};
        dp_secret_init(&s.wifi_password);
        const char *problem = dp_secret_assign(&s.wifi_password, text(a, FIELD_WIFI_PASSWORD),
                                               a->fields[FIELD_WIFI_PASSWORD].len);
        if (!problem) problem = dp_validate_sd_settings(&s, err);
        dp_sd_settings_free(&s);
        return problem;
    }
    case DP_JOB_ALPINE: {
        char *hostname = trimmed(a, FIELD_HOSTNAME);
        bool valid = hostname && dp_valid_appliance_hostname(hostname);
        free(hostname);
        if (!valid) return "Appliance hostname must be one DNS label of 1-63 characters";
        if (!same_value(a, FIELD_OS_ROOT_PASSWORD, FIELD_OS_ROOT_CONFIRM)) {
            return "Root password confirmation does not match";
        }
        if (!same_value(a, FIELD_OS_PI_PASSWORD, FIELD_OS_PI_CONFIRM)) {
            return "pi password confirmation does not match";
        }
        static const field passwords[] = {FIELD_OS_ROOT_PASSWORD, FIELD_OS_PI_PASSWORD};
        for (size_t i = 0; i < 2; i++) {
            dp_secret s;
            dp_secret_init(&s);
            const char *problem =
                dp_secret_assign(&s, text(a, passwords[i]), a->fields[passwords[i]].len);
            if (!problem) problem = dp_validate_os_password(&s);
            dp_secret_clear(&s);
            if (problem) return problem;
        }
        char *ssid = trimmed(a, FIELD_WIFI_SSID);
        bool needs_password = ssid && ssid[0] && a->fields[FIELD_WIFI_PASSWORD].len == 0;
        free(ssid);
        if (needs_password) return "A Wi-Fi SSID needs a Wi-Fi password";
        return NULL;
    }
    case DP_JOB_DEPLOY: {
        char *dir = trimmed(a, FIELD_REMOTE_DIRECTORY);
        bool valid = dir && dp_valid_remote_directory(dir);
        free(dir);
        if (!valid) {
            return "Remote directory must be a normalized absolute path, such as /opt/omt-client";
        }
        if (a->rotate_web_password && !same_value(a, FIELD_WEB_PASSWORD, FIELD_WEB_CONFIRM)) {
            return "Web GUI password confirmation does not match";
        }
        return NULL;
    }
    case DP_JOB_WEB_PASSWORD:
        if (!same_value(a, FIELD_MANAGE_WEB_PASSWORD, FIELD_MANAGE_WEB_CONFIRM)) {
            return "Web GUI password confirmation does not match";
        }
        return NULL;
    case DP_JOB_HOSTNAME: {
        char *name = trimmed(a, FIELD_MANAGE_HOSTNAME);
        bool valid = name && dp_valid_appliance_hostname(name);
        free(name);
        return valid ? NULL : "Hostname must be one DNS label of 1-63 characters";
    }
    case DP_JOB_TEST:
    case DP_JOB_MANAGE:
    case DP_JOB_WIFI: return NULL;
    }
    return NULL;
}

static const char *assign(dp_secret *s, const app *a, field f) {
    return dp_secret_assign(s, text(a, f), a->fields[f].len);
}

const char *app_request(const app *a, dp_job_kind job, dp_action action, dp_job_request *r) {
    dp_job_request_init(r);
    r->job = job;
    r->action = action;
    r->options.project_root = NULL;
    r->options.remote_directory = trimmed(a, FIELD_REMOTE_DIRECTORY);
    r->options.rebuild_image = false;
    if (job != DP_JOB_PREPARE_SD) {
        dp_connection_free(&r->connection);
        const char *problem = build_connection(a, &r->connection);
        if (problem) return problem;
        r->has_connection = true;
    }
    free(r->wifi_country);
    r->boot_directory = trimmed(a, FIELD_BOOT_DIRECTORY);
    r->wifi_country = trimmed(a, FIELD_WIFI_COUNTRY);
    r->wifi_ssid = trimmed(a, FIELD_WIFI_SSID);
    r->wifi_connect = a->wifi_connect;
    r->wifi_preserve_existing_profiles = a->wifi_preserve;
    r->hostname = trimmed(a, FIELD_HOSTNAME);
    r->manage_hostname = trimmed(a, FIELD_MANAGE_HOSTNAME);
    r->rotate_web_password = a->rotate_web_password;
    /* Manage's Web password fields are separate from Deploy's, so a rotation
     * typed on one view cannot be submitted from the other. */
    field web = job == DP_JOB_WEB_PASSWORD ? FIELD_MANAGE_WEB_PASSWORD : FIELD_WEB_PASSWORD;
    const char *problem = assign(&r->wifi_password, a, FIELD_WIFI_PASSWORD);
    if (!problem) problem = assign(&r->os_root_password, a, FIELD_OS_ROOT_PASSWORD);
    if (!problem) problem = assign(&r->os_pi_password, a, FIELD_OS_PI_PASSWORD);
    if (!problem) problem = assign(&r->web_password, a, web);
    return problem;
}

void app_start(app *a, dp_job_kind job, dp_action action) {
    if (app_busy(a)) {
        app_set_status(a, "A job is already running.");
        return;
    }
    dp_err err;
    dp_err_init(&err);
    const char *problem = app_precheck(a, job, &err);
    if (problem) {
        app_set_status(a, "Cannot start: %s", problem);
        dp_err_free(&err);
        return;
    }
    dp_err_free(&err);
    worker *w = worker_new();
    if (!w) {
        app_set_status(a, "Cannot start: out of memory");
        return;
    }
    dp_job_request_free(&w->request);
    problem = app_request(a, job, action, &w->request);
    if (problem) {
        app_set_status(a, "Cannot start: %s", problem);
        w->refs = 1;
        worker_release(w);
        return;
    }
    dp_err spawn;
    dp_err_init(&spawn);
    if (!dp_thread_spawn(worker_main, w, &spawn)) {
        app_set_status(a, "Cannot start: %s", dp_err_text(&spawn));
        dp_err_free(&spawn);
        w->refs = 1;
        worker_release(w);
        return;
    }
    dp_err_free(&spawn);
    a->job = w;
    a->switch_to_pi_after_job = job == DP_JOB_ALPINE && a->apply_alpine_login;
    app_set_status(a, "Running...");
    a->follow_log = true;
    /* Progress belongs in front of the operator, not behind a tab. */
    app_select_view(a, VIEW_ACTIVITY);
}

static void confirm(app *a, bool quit, dp_job_kind job, dp_action action, const char *prompt) {
    a->confirm.active = true;
    a->confirm.quit = quit;
    a->confirm.job = job;
    a->confirm.action = action;
    omt_strlcpy(a->confirm.prompt, prompt, sizeof(a->confirm.prompt));
}

void app_press(app *a, slot s) {
    switch (s) {
    case SLOT_TEST_BUTTON: app_start(a, DP_JOB_TEST, DP_ACTION_STATUS); break;
    case SLOT_PREPARE_SD_BUTTON: app_start(a, DP_JOB_PREPARE_SD, DP_ACTION_STATUS); break;
    /* This erases the disk the Pi boots from. */
    case SLOT_ALPINE_BUTTON:
        confirm(a, false, DP_JOB_ALPINE, DP_ACTION_STATUS,
                "Erase the boot disk and install Alpine in persistent sys mode? The Pi reboots "
                "when it finishes.");
        break;
    case SLOT_DEPLOY_BUTTON: app_start(a, DP_JOB_DEPLOY, DP_ACTION_STATUS); break;
    case SLOT_STATUS_BUTTON: app_start(a, DP_JOB_MANAGE, DP_ACTION_STATUS); break;
    case SLOT_LOGS_BUTTON: app_start(a, DP_JOB_MANAGE, DP_ACTION_LOGS); break;
    case SLOT_RESTART_BUTTON:
        confirm(a, false, DP_JOB_MANAGE, DP_ACTION_RESTART, "Restart the appliance container now?");
        break;
    case SLOT_REBOOT_BUTTON:
        confirm(a, false, DP_JOB_MANAGE, DP_ACTION_REBOOT, "Reboot the Raspberry Pi now?");
        break;
    case SLOT_HOSTNAME_BUTTON: app_start(a, DP_JOB_HOSTNAME, DP_ACTION_STATUS); break;
    case SLOT_WEB_PASSWORD_BUTTON: app_start(a, DP_JOB_WEB_PASSWORD, DP_ACTION_STATUS); break;
    case SLOT_WIFI_BUTTON: app_start(a, DP_JOB_WIFI, DP_ACTION_STATUS); break;
    default: break;
    }
}

void app_activate(app *a) {
    slot s;
    if (!app_selected(a, &s)) return;
    switch (slot_kind(s)) {
    case KIND_TOGGLE: {
        bool *flag = toggle_ptr(a, s);
        if (flag) *flag = !*flag;
        break;
    }
    case KIND_TEXT:
    case KIND_SECRET: app_move_focus(a, 1); break;
    case KIND_BUTTON: app_press(a, s); break;
    }
}

/* Quitting abandons a worker that may be mid-transaction, so it asks first
 * while one runs. */
void app_request_quit(app *a) {
    if (app_busy(a)) {
        confirm(a, true, DP_JOB_TEST, DP_ACTION_STATUS,
                "A job is still running. Quit and stop watching it?");
    } else {
        a->should_quit = true;
    }
}

void app_confirm_pending(app *a, bool accepted) {
    if (!a->confirm.active) return;
    pending p = a->confirm;
    a->confirm.active = false;
    if (!accepted) {
        app_set_status(a, "Cancelled.");
        return;
    }
    if (p.quit) {
        a->should_quit = true;
    } else {
        app_start(a, p.job, p.action);
    }
}

/* ------------------------------------------------------------- editing */

/* The byte offset of character `index` in UTF-8 text. */
static size_t byte_offset(const omt_buf *b, size_t index) {
    size_t chars = 0;
    for (size_t i = 0; i < b->len; i++) {
        if ((b->data[i] & 0xC0) != 0x80) {
            if (chars == index) return i;
            chars++;
        }
    }
    return b->len;
}

static omt_buf *focused_text(app *a) {
    slot s;
    if (!app_selected(a, &s)) return NULL;
    kind k = slot_kind(s);
    if (k != KIND_TEXT && k != KIND_SECRET) return NULL;
    return &a->fields[slot_field(s)];
}

/* Replaces bytes [at, at + remove) of a value with `insert`, building the
 * result in a fresh buffer and wiping the old one, so neither a secret's
 * characters nor a stale terminator survive the edit. */
static void splice(omt_buf *b, size_t at, size_t remove, const uint8_t *insert, size_t insert_len) {
    omt_buf next;
    omt_buf_init(&next, FIELD_LIMIT + 1);
    if (!omt_buf_reserve(&next, b->len + insert_len + 1)) return;
    omt_buf_append(&next, b->data, at);
    if (insert_len) omt_buf_append(&next, insert, insert_len);
    omt_buf_append(&next, b->data + at + remove, b->len - at - remove);
    if (next.failed) {
        omt_buf_free_secret(&next);
        return;
    }
    omt_buf_free_secret(b);
    *b = next;
}

void app_insert(app *a, uint32_t ch) {
    omt_buf *b = focused_text(a);
    if (!b || ch < 0x20 || (ch >= 0x7F && ch < 0xA0)) return;
    omt_buf encoded;
    omt_buf_init(&encoded, 8);
    omt_utf8_put(&encoded, ch);
    if (b->len + encoded.len <= FIELD_LIMIT) {
        splice(b, byte_offset(b, a->cursor), 0, encoded.data, encoded.len);
        a->cursor++;
    }
    omt_buf_free(&encoded);
}

static void remove_char(omt_buf *b, size_t at) {
    size_t end = at + 1;
    while (end < b->len && (b->data[end] & 0xC0) == 0x80) end++;
    splice(b, at, end - at, NULL, 0);
}

void app_backspace(app *a) {
    omt_buf *b = focused_text(a);
    if (!b || a->cursor == 0) return;
    size_t at = byte_offset(b, a->cursor - 1);
    if (at < b->len) {
        remove_char(b, at);
        a->cursor--;
    }
}

void app_delete(app *a) {
    omt_buf *b = focused_text(a);
    if (!b) return;
    size_t at = byte_offset(b, a->cursor);
    if (at < b->len) remove_char(b, at);
}

void app_move_cursor(app *a, int delta) {
    slot s;
    if (!app_selected(a, &s)) return;
    long length = (long)value_chars(a, s);
    long next = (long)a->cursor + delta;
    if (next < 0) next = 0;
    if (next > length) next = length;
    a->cursor = (size_t)next;
}

/* ------------------------------------------------------------------ keys */

static void cycle_view(app *a, int delta) {
    int next = ((int)a->current + delta) % VIEW_COUNT;
    if (next < 0) next += VIEW_COUNT;
    app_select_view(a, (view)next);
}

void app_scroll_log(app *a, int direction) {
    if (direction < 0) {
        /* Leaving the tail starts from the tail: paging up from a following
         * view used to land on the oldest line of the run. */
        if (a->follow_log) a->log_scroll = a->log_len > 0 ? a->log_len - 1 : 0;
        a->follow_log = false;
        a->log_scroll = a->log_scroll > 10 ? a->log_scroll - 10 : 0;
    } else {
        a->log_scroll += 10;
        /* Back at the tail resumes following. */
        if (a->log_scroll >= a->log_len) a->follow_log = true;
    }
}

static void scroll_page(app *a, int direction) {
    if (a->current == VIEW_ABOUT) {
        app_scroll_about(a, direction * 10);
    } else {
        app_scroll_log(a, direction);
    }
}

void app_handle_key(app *a, const key_event *key) {
    /* A pending confirmation owns the keyboard until it is answered, so a
     * reboot cannot be triggered by a keystroke meant for the form behind. */
    if (a->confirm.active) {
        if (key->code == KEY_CHAR && !key->ctrl && (key->ch == 'y' || key->ch == 'Y')) {
            app_confirm_pending(a, true);
        } else if ((key->code == KEY_CHAR && !key->ctrl && (key->ch == 'n' || key->ch == 'N')) ||
                   key->code == KEY_ESC) {
            app_confirm_pending(a, false);
        }
        return;
    }
    bool about = a->current == VIEW_ABOUT;
    switch (key->code) {
    case KEY_CHAR:
        if (key->ctrl) {
            if (key->ch == 'q') {
                app_request_quit(a);
            } else if (key->ch == 'c') {
                /* Ctrl+C stops the job rather than the program while one runs. */
                if (app_busy(a)) {
                    app_cancel_job(a);
                } else {
                    a->should_quit = true;
                }
            } else if (key->ch == 'r') {
                a->reveal = !a->reveal;
            }
        } else {
            app_insert(a, key->ch);
        }
        break;
    case KEY_ESC: app_cancel_job(a); break;
    case KEY_RIGHT:
        if (key->ctrl) {
            cycle_view(a, 1);
        } else {
            app_move_cursor(a, 1);
        }
        break;
    case KEY_LEFT:
        if (key->ctrl) {
            cycle_view(a, -1);
        } else {
            app_move_cursor(a, -1);
        }
        break;
    case KEY_F:
        if (key->f >= 1 && key->f <= VIEW_COUNT) app_select_view(a, (view)(key->f - 1));
        break;
    case KEY_DOWN:
        if (about) {
            app_scroll_about(a, 1);
        } else {
            app_move_focus(a, 1);
        }
        break;
    case KEY_UP:
        if (about) {
            app_scroll_about(a, -1);
        } else {
            app_move_focus(a, -1);
        }
        break;
    case KEY_HOME:
        if (about) {
            a->about_scroll = 0;
        } else {
            app_cursor_home(a);
        }
        break;
    case KEY_END:
        if (about) {
            /* Overshoots on purpose; clamped where the wrapped length is known. */
            a->about_scroll = SIZE_MAX;
        } else {
            app_cursor_end(a);
        }
        break;
    case KEY_TAB: app_move_focus(a, 1); break;
    case KEY_BACKTAB: app_move_focus(a, -1); break;
    case KEY_ENTER: app_activate(a); break;
    case KEY_BACKSPACE: app_backspace(a); break;
    case KEY_DELETE: app_delete(a); break;
    case KEY_PAGE_UP: scroll_page(a, -1); break;
    case KEY_PAGE_DOWN: scroll_page(a, 1); break;
    case KEY_NONE: break;
    }
}
