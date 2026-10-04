/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * State and input handling for the terminal deployer. Rendering lives in
 * ui.c; this never draws.
 */
#ifndef DP_TUI_APP_H
#define DP_TUI_APP_H

#include "common/buf.h"
#include "deploy/core/deploy.h"
#include "deploy/tui/term.h"

/* How many progress lines the activity log keeps: a deployment emits far more
 * than a screen's worth, and the operator scrolling back wants the recent
 * end. */
#define LOG_CAPACITY 2000u
#define FIELD_LIMIT 4096u

typedef enum {
    VIEW_CONNECTION,
    VIEW_SD_CARD,
    VIEW_ALPINE,
    VIEW_DEPLOY,
    VIEW_MANAGE,
    VIEW_WIFI,
    VIEW_ACTIVITY,
    VIEW_ABOUT,
    VIEW_COUNT,
} view;

/* One focusable row, flat across every view so focus is a single index and
 * the render and input paths cannot disagree about what is selected. */
typedef enum {
    SLOT_HOST,
    SLOT_USER,
    SLOT_PASSWORD,
    SLOT_SUDO_PASSWORD,
    SLOT_KNOWN_HOSTS,
    SLOT_TEST_BUTTON,
    SLOT_BOOT_DIRECTORY,
    SLOT_SD_WIFI_COUNTRY,
    SLOT_SD_WIFI_SSID,
    SLOT_SD_WIFI_PASSWORD,
    SLOT_PREPARE_SD_BUTTON,
    SLOT_ALPINE_HOSTNAME,
    SLOT_ALPINE_ROOT_PASSWORD,
    SLOT_ALPINE_ROOT_CONFIRM,
    SLOT_ALPINE_PI_PASSWORD,
    SLOT_ALPINE_PI_CONFIRM,
    SLOT_ALPINE_WIFI_SSID,
    SLOT_ALPINE_WIFI_PASSWORD,
    SLOT_ALPINE_APPLY_LOGIN,
    SLOT_ALPINE_BUTTON,
    SLOT_REMOTE_DIRECTORY,
    SLOT_ROTATE_WEB_PASSWORD,
    SLOT_WEB_PASSWORD,
    SLOT_WEB_CONFIRM,
    SLOT_DEPLOY_BUTTON,
    SLOT_STATUS_BUTTON,
    SLOT_LOGS_BUTTON,
    SLOT_RESTART_BUTTON,
    SLOT_REBOOT_BUTTON,
    SLOT_MANAGE_HOSTNAME,
    SLOT_HOSTNAME_BUTTON,
    SLOT_MANAGE_WEB_PASSWORD,
    SLOT_MANAGE_WEB_CONFIRM,
    SLOT_WEB_PASSWORD_BUTTON,
    SLOT_WIFI_SSID,
    SLOT_WIFI_PASSWORD,
    SLOT_WIFI_CONNECT,
    SLOT_WIFI_PRESERVE,
    SLOT_WIFI_BUTTON,
    SLOT_COUNT,
} slot;

typedef enum { KIND_TEXT, KIND_SECRET, KIND_TOGGLE, KIND_BUTTON } kind;

/* The editable values. Several slots share one: the Wi-Fi SSID and password
 * are the same fields on the SD-card, Alpine, and Wi-Fi views. */
typedef enum {
    FIELD_HOST,
    FIELD_USER,
    FIELD_PASSWORD,
    FIELD_SUDO_PASSWORD,
    FIELD_KNOWN_HOSTS,
    FIELD_BOOT_DIRECTORY,
    FIELD_WIFI_COUNTRY,
    FIELD_HOSTNAME,
    FIELD_OS_ROOT_PASSWORD,
    FIELD_OS_ROOT_CONFIRM,
    FIELD_OS_PI_PASSWORD,
    FIELD_OS_PI_CONFIRM,
    FIELD_REMOTE_DIRECTORY,
    FIELD_WEB_PASSWORD,
    FIELD_WEB_CONFIRM,
    FIELD_MANAGE_HOSTNAME,
    FIELD_MANAGE_WEB_PASSWORD,
    FIELD_MANAGE_WEB_CONFIRM,
    FIELD_WIFI_SSID,
    FIELD_WIFI_PASSWORD,
    FIELD_COUNT,
    FIELD_NONE = -1,
} field;

/* What answering a confirmation yes will do. */
typedef struct {
    bool active;
    bool quit; /* otherwise start `job` */
    dp_job_kind job;
    dp_action action;
    char prompt[256];
} pending;

typedef struct worker worker;

typedef struct {
    view current;
    size_t focus;
    size_t cursor; /* in characters, so a multi-byte value never splits */
    bool reveal;
    bool should_quit;
    omt_buf status;

    omt_buf fields[FIELD_COUNT];
    bool apply_alpine_login;
    bool rotate_web_password;
    bool wifi_connect;
    bool wifi_preserve;

    char *log[LOG_CAPACITY];
    size_t log_len;
    size_t log_scroll;
    bool follow_log;
    /* First visible line of the About document. */
    size_t about_scroll;
    pending confirm;

    worker *job;
    bool switch_to_pi_after_job;
} app;

const char *view_title(view v);
/* The focusable rows a view shows, in tab order. */
const slot *view_slots(view v, size_t *count);
const char *slot_label(slot s);
kind slot_kind(slot s);
field slot_field(slot s);

void app_init(app *a);
void app_free(app *a);
bool app_busy(const app *a);
bool app_selected(const app *a, slot *out);
const char *app_value(const app *a, slot s);
bool app_toggle(const app *a, slot s);
void app_set_status(app *a, const char *fmt, ...) OMT_PRINTF(2, 3);
void app_select_view(app *a, view v);
void app_move_focus(app *a, int delta);
void app_scroll_about(app *a, long delta);
void app_push_log(app *a, const char *line);
/* Drains whatever the worker has produced since the last redraw. */
void app_poll_worker(app *a);
void app_cancel_job(app *a);
void app_activate(app *a);
void app_press(app *a, slot s);
void app_request_quit(app *a);
void app_confirm_pending(app *a, bool accepted);
void app_insert(app *a, uint32_t ch);
void app_backspace(app *a);
void app_delete(app *a);
void app_move_cursor(app *a, int delta);
void app_cursor_home(app *a);
void app_cursor_end(app *a);
void app_handle_key(app *a, const key_event *key);
void app_scroll_log(app *a, int direction);
/* Validates what the operator can still fix; NULL when the job may start. */
const char *app_precheck(const app *a, dp_job_kind job, dp_err *err);
/* The request a job would run with. */
const char *app_request(const app *a, dp_job_kind job, dp_action action, dp_job_request *out);
/* Starts a job on a worker thread (exposed for the tests' fake workers). */
void app_start(app *a, dp_job_kind job, dp_action action);

/* For the tests: a worker that has already finished with the given result. */
worker *worker_finished_for_test(bool ok, const char *error);
worker *worker_running_for_test(void);

#endif
