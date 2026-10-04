/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The Web frontend's routes. Every page but the login and the static assets
 * needs a current session; every POST also needs the session-scoped CSRF
 * token. Security headers are applied to every response.
 */
#ifndef OMT_WEB_APP_H
#define OMT_WEB_APP_H

#include "web/auth.h"
#include "web/diagnostics.h"
#include "web/http.h"
#include "web/playback.h"
#include "web/settings.h"

#define OMT_RATE_KEYS 4096
#define OMT_MAX_FLASHES 64

typedef struct {
    char scope[24];
    char peer[64];
    uint64_t *expiries; /* ring of expiry times, oldest first */
    size_t count;
    size_t cap;
} omt_rate_entry;

typedef struct {
    char key[33];
    char category[8];
    char message[OMT_ACTION_TEXT];
} omt_flash;

typedef struct {
    const omt_web_settings *settings;
    omt_auth auth;
    omt_playback playback;
    omt_diagnostics diagnostics;
    char hostname[256];
    omt_rate_entry *rates;
    size_t rate_count;
    omt_flash flashes[OMT_MAX_FLASHES];
    size_t flash_count;
} omt_app;

OMT_NODISCARD bool omt_app_init(omt_app *app, const omt_web_settings *s, omt_err *err);
void omt_app_free(omt_app *app);
/* The http server's handler. */
void omt_app_handle(void *context, const omt_http_request *req, omt_http_response *res);

/* Exposed for tests. */
bool omt_app_allow(omt_app *app, const char *scope, const char *peer, omt_rate_limit limit);
bool omt_cookie_value(const char *header, const char *name, char *out, size_t size);

/* application/x-www-form-urlencoded, decoded. Duplicate keys keep the last
 * value; any pair that decodes to invalid UTF-8 empties the whole form, as
 * serde_urlencoded's error did. */
#define OMT_FORM_MAX 32
typedef struct {
    char *keys[OMT_FORM_MAX];
    char *values[OMT_FORM_MAX];
    size_t count;
} omt_form;
void omt_form_parse(const uint8_t *body, size_t len, omt_form *out);
const char *omt_form_get(const omt_form *f, const char *key);
void omt_form_free(omt_form *f);

#endif
