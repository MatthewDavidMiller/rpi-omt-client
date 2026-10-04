/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "web/app.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common/fsio.h"
#include "common/json.h"
#include "common/rand.h"
#include "web/network.h"
#include "web/templates.h"
#include "web/tv.h"

#define SESSION_COOKIE "__Host-omt_session"
#define LOGIN_COOKIE "__Host-omt_login"
#define FLASH_COOKIE "__Host-omt_flash"
#define COOKIE_ATTRIBUTES "Path=/; Secure; HttpOnly; SameSite=Lax"
#define CSP "default-src 'self'; style-src 'self'; script-src 'none'; form-action 'self'"

/* ------------------------------------------------------------------ form */

static int hexval(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

/* Percent-decoding as the WHATWG urlencoded parser does it: '+' is a space,
 * a well-formed %XX is a byte, and a malformed escape is kept literally. */
static char *decode(const uint8_t *s, size_t len, bool *valid) {
    char *out = malloc(len + 1);
    if (!out) return NULL;
    size_t n = 0;
    for (size_t i = 0; i < len; i++) {
        if (s[i] == '+') {
            out[n++] = ' ';
        } else if (s[i] == '%' && i + 2 < len && hexval((char)s[i + 1]) >= 0 &&
                   hexval((char)s[i + 2]) >= 0) {
            out[n++] = (char)(hexval((char)s[i + 1]) << 4 | hexval((char)s[i + 2]));
            i += 2;
        } else {
            out[n++] = (char)s[i];
        }
    }
    out[n] = 0;
    if (!omt_utf8_valid(out, n) || memchr(out, 0, n)) *valid = false;
    return out;
}

void omt_form_free(omt_form *f) {
    for (size_t i = 0; i < f->count; i++) {
        free(f->keys[i]);
        free(f->values[i]);
    }
    f->count = 0;
}

void omt_form_parse(const uint8_t *body, size_t len, omt_form *out) {
    out->count = 0;
    bool valid = true;
    size_t pos = 0;
    while (pos < len && valid) {
        const uint8_t *amp = memchr(body + pos, '&', len - pos);
        size_t end = amp ? (size_t)(amp - body) : len;
        if (end > pos) {
            const uint8_t *eq = memchr(body + pos, '=', end - pos);
            size_t key_end = eq ? (size_t)(eq - body) : end;
            char *key = decode(body + pos, key_end - pos, &valid);
            char *value = eq ? decode(eq + 1, end - key_end - 1, &valid)
                             : decode((const uint8_t *)"", 0, &valid);
            if (!key || !value) {
                free(key);
                free(value);
                valid = false;
                break;
            }
            size_t i = 0;
            while (i < out->count && strcmp(out->keys[i], key) != 0) i++;
            if (i < out->count) {
                free(out->values[i]);
                out->values[i] = value;
                free(key);
            } else if (out->count < OMT_FORM_MAX) {
                out->keys[out->count] = key;
                out->values[out->count++] = value;
            } else {
                free(key);
                free(value);
            }
        }
        pos = end + 1;
    }
    if (!valid) omt_form_free(out);
}

const char *omt_form_get(const omt_form *f, const char *key) {
    for (size_t i = 0; i < f->count; i++)
        if (strcmp(f->keys[i], key) == 0) return f->values[i];
    return NULL;
}

/* --------------------------------------------------------------- cookies */

bool omt_cookie_value(const char *header, const char *name, char *out, size_t size) {
    size_t name_len = strlen(name);
    const char *p = header;
    while (*p) {
        const char *semi = strchr(p, ';');
        const char *end = semi ? semi : p + strlen(p);
        omt_span part = omt_utf8_trim(p, (size_t)(end - p));
        const char *eq = memchr(part.p, '=', part.len);
        if (eq) {
            size_t key_len = (size_t)(eq - (const char *)part.p);
            size_t value_len = part.len - key_len - 1;
            if (key_len == name_len && memcmp(part.p, name, name_len) == 0 && value_len > 0) {
                if (value_len >= size) return false;
                memcpy(out, eq + 1, value_len);
                out[value_len] = 0;
                return true;
            }
        }
        if (!semi) break;
        p = semi + 1;
    }
    return false;
}

static bool request_cookie(const omt_http_request *req, const char *name, char *out, size_t size) {
    char header[8192];
    return omt_http_header_value(req, "cookie", header, sizeof(header)) &&
           omt_cookie_value(header, name, out, size);
}

static void set_cookie(omt_http_response *res, const char *name, const char *value,
                       uint64_t max_age) {
    char text[512];
    snprintf(text, sizeof(text), "%s=%s; Path=/; Max-Age=%llu; Secure; HttpOnly; SameSite=Lax",
             name, value, (unsigned long long)max_age);
    omt_http_add_header(res, "set-cookie", text);
}

static void clear_cookie(omt_http_response *res, const char *name) {
    char text[256];
    snprintf(text, sizeof(text), "%s=; Path=/; Max-Age=0; " COOKIE_ATTRIBUTES, name);
    omt_http_add_header(res, "set-cookie", text);
}

/* ------------------------------------------------------------ rate limit */

bool omt_app_allow(omt_app *app, const char *scope, const char *peer, omt_rate_limit limit) {
    uint64_t now = omt_now_ms();
    /* Expire across every key, and forget keys with nothing left. */
    size_t kept = 0;
    for (size_t i = 0; i < app->rate_count; i++) {
        omt_rate_entry *e = &app->rates[i];
        size_t live = 0;
        for (size_t k = 0; k < e->count; k++)
            if (e->expiries[k] > now) e->expiries[live++] = e->expiries[k];
        e->count = live;
        if (live) {
            app->rates[kept++] = *e;
        } else {
            free(e->expiries);
        }
    }
    app->rate_count = kept;
    omt_rate_entry *entry = NULL;
    for (size_t i = 0; i < app->rate_count && !entry; i++)
        if (strcmp(app->rates[i].scope, scope) == 0 && strcmp(app->rates[i].peer, peer) == 0)
            entry = &app->rates[i];
    if (!entry) {
        /* Fail closed when the table is full: an attacker spraying addresses
         * cannot buy an unthrottled attempt by evicting someone. */
        if (app->rate_count >= OMT_RATE_KEYS) return false;
        entry = &app->rates[app->rate_count];
        memset(entry, 0, sizeof(*entry));
        omt_strlcpy(entry->scope, scope, sizeof(entry->scope));
        omt_strlcpy(entry->peer, peer, sizeof(entry->peer));
        app->rate_count++;
    }
    if (entry->count >= limit.count) return false;
    if (entry->count == entry->cap) {
        size_t cap = entry->cap ? entry->cap * 2 : 4;
        uint64_t *grown = realloc(entry->expiries, cap * sizeof(uint64_t));
        if (!grown) return false;
        entry->expiries = grown;
        entry->cap = cap;
    }
    entry->expiries[entry->count++] = now + limit.window_ms;
    return true;
}

/* ------------------------------------------------------------- lifecycle */

bool omt_app_init(omt_app *app, const omt_web_settings *s, omt_err *err) {
    memset(app, 0, sizeof(*app));
    app->settings = s;
    omt_playback_init(&app->playback, s);
    app->diagnostics.settings = s;
    app->diagnostics.playback = &app->playback;
    app->rates = calloc(OMT_RATE_KEYS, sizeof(omt_rate_entry));
    if (!app->rates) {
        omt_err_set(err, "out of memory");
        return false;
    }
    omt_buf host;
    omt_err ignored;
    omt_strlcpy(app->hostname, "omt-client", sizeof(app->hostname));
    if (omt_read_text("/etc/hostname", 255, &host, &ignored) == OMT_READ_OK) {
        omt_span t = omt_utf8_trim(omt_buf_cstr(&host), host.len);
        memcpy(app->hostname, t.p, t.len);
        app->hostname[t.len] = 0;
        omt_buf_free(&host);
    }
    return omt_auth_load(&app->auth, s, err);
}

void omt_app_free(omt_app *app) {
    for (size_t i = 0; i < app->rate_count; i++) free(app->rates[i].expiries);
    free(app->rates);
    omt_auth_free(&app->auth);
}

/* ------------------------------------------------------------- responses */

static void redirect(omt_http_response *res, const char *to) {
    res->status = 303;
    omt_http_add_header(res, "location", to);
}

typedef struct {
    omt_app *app;
    const omt_http_request *req;
    omt_http_response *res;
    tv_arena arena;
    char session[OMT_SESSION_ID_HEX + 1];
    bool has_flash_cookie;
} request_ctx;

static tv *common(request_ctx *r, bool authenticated, const char *csrf, const char *endpoint) {
    tv_arena *a = &r->arena;
    tv *ctx = tv_map(a);
    tv *flashes = tv_list(a);
    char key[64];
    if (request_cookie(r->req, FLASH_COOKIE, key, sizeof(key))) {
        omt_app *app = r->app;
        for (size_t i = 0; i < app->flash_count; i++) {
            if (strcmp(app->flashes[i].key, key) != 0) continue;
            tv *pair = tv_list(a);
            tv_push(a, pair, tv_str(a, app->flashes[i].category));
            tv_push(a, pair, tv_str(a, app->flashes[i].message));
            tv_push(a, flashes, pair);
            app->flashes[i] = app->flashes[--app->flash_count];
            break;
        }
    }
    tv_set_str(a, ctx, "hostname", r->app->hostname);
    tv_set_bool(a, ctx, "authenticated", authenticated);
    tv_set_str(a, ctx, "csrf_token", csrf);
    tv_set_str(a, ctx, "endpoint", endpoint);
    tv_set(a, ctx, "flashes", flashes);
    return ctx;
}

static void render(request_ctx *r, const char *template, tv *context, int status) {
    omt_http_response *res = r->res;
    if (r->arena.failed || !omt_template_render(template, context, &res->body)) {
        fprintf(stderr, "template error: unable to render %s\n", template);
        omt_buf_clear(&res->body);
        res->status = 500;
        omt_http_add_header(res, "content-type", "text/plain; charset=utf-8");
        omt_buf_puts(&res->body, "Internal Server Error");
        return;
    }
    res->status = status;
    omt_http_add_header(res, "content-type", "text/html; charset=utf-8");
    if (r->has_flash_cookie) clear_cookie(res, FLASH_COOKIE);
}

static void session_token(request_ctx *r, char out[OMT_SESSION_ID_HEX + 1]) {
    if (!omt_auth_csrf_token(&r->app->auth, "session", r->session, out)) out[0] = 0;
}

static void flash_redirect(request_ctx *r, const char *to, const omt_action_result *result) {
    omt_app *app = r->app;
    omt_buf key;
    omt_buf_init(&key, 64);
    omt_err err;
    if (!omt_random_hex(&key, 16, &err)) {
        omt_buf_free(&key);
        redirect(r->res, to);
        return;
    }
    /* Bounded: past the cap the lexically first key gives way, as the Rust
     * BTreeMap did. */
    while (app->flash_count >= OMT_MAX_FLASHES) {
        size_t first = 0;
        for (size_t i = 1; i < app->flash_count; i++)
            if (strcmp(app->flashes[i].key, app->flashes[first].key) < 0) first = i;
        app->flashes[first] = app->flashes[--app->flash_count];
    }
    omt_flash *f = &app->flashes[app->flash_count++];
    omt_strlcpy(f->key, omt_buf_cstr(&key), sizeof(f->key));
    omt_strlcpy(f->category, result->ok ? "success" : "error", sizeof(f->category));
    omt_strlcpy(f->message, result->ok ? result->message : result->error, sizeof(f->message));
    redirect(r->res, to);
    set_cookie(r->res, FLASH_COOKIE, f->key, 60);
    omt_buf_free(&key);
}

static void login_page(request_ctx *r, const char *error, int status) {
    omt_buf nonce;
    omt_buf_init(&nonce, 64);
    omt_err err;
    if (!omt_random_hex(&nonce, 24, &err)) omt_buf_clear(&nonce);
    char token[OMT_SESSION_ID_HEX + 1] = "";
    if (!omt_auth_csrf_token(&r->app->auth, "login", omt_buf_cstr(&nonce), token)) token[0] = 0;
    tv *ctx = common(r, false, token, "auth.login");
    tv_set_str(&r->arena, ctx, "error", error);
    render(r, "login.html", ctx, status);
    set_cookie(r->res, LOGIN_COOKIE, omt_buf_cstr(&nonce), 600);
    omt_buf_free(&nonce);
}

static void error_page(request_ctx *r, const char *title, const char *message, int status) {
    char token[OMT_SESSION_ID_HEX + 1];
    session_token(r, token);
    tv *ctx = common(r, true, token, "error");
    tv_set_str(&r->arena, ctx, "title", title);
    tv_set_str(&r->arena, ctx, "message", message);
    render(r, "error.html", ctx, status);
}

static void too_many(request_ctx *r) {
    error_page(r, "Too many requests", "Too many requests. Please wait and try again.", 429);
}

/* ------------------------------------------------------------- sessions */

static bool current_session(request_ctx *r) {
    char id[128];
    if (!request_cookie(r->req, SESSION_COOKIE, id, sizeof(id))) return false;
    if (!omt_auth_is_current(&r->app->auth, id)) return false;
    omt_strlcpy(r->session, id, sizeof(r->session));
    return strlen(id) == OMT_SESSION_ID_HEX;
}

static bool require_session(request_ctx *r) {
    if (current_session(r)) return true;
    redirect(r->res, "/login");
    return false;
}

/* The authenticated-POST preamble: a current session and its CSRF token. */
static bool authenticated_post(request_ctx *r, omt_form *form) {
    omt_form_parse(r->req->body, r->req->body_len, form);
    if (!require_session(r)) return false;
    const char *token = omt_form_get(form, "csrf_token");
    if (!token || !omt_auth_verify_csrf(&r->app->auth, "session", r->session, token)) {
        char csrf[OMT_SESSION_ID_HEX + 1];
        session_token(r, csrf);
        tv *ctx = common(r, true, csrf, "error");
        tv_set_str(&r->arena, ctx, "title", "Session expired");
        tv_set_str(&r->arena, ctx, "message", "Session expired. Please try again.");
        render(r, "error.html", ctx, 400);
        return false;
    }
    return true;
}

/* ------------------------------------------------------------- contexts */

static tv *video_limit_tv(tv_arena *a, const omt_video_limit *v) {
    tv *m = tv_map(a);
    tv_set_str(a, m, "board_label", v->board_label);
    tv_set_str(a, m, "effective", v->effective);
    tv_set_str(a, m, "board_default", v->board_default);
    tv_set_str(a, m, "error", v->error);
    tv_set_str(a, m, "effective_description", v->effective_description);
    tv_set_str(a, m, "board_default_description", v->board_default_description);
    tv_set_bool(a, m, "overridden", v->overridden);
    tv_set_bool(a, m, "above_board_default", v->above_board_default);
    return m;
}

static tv *command_tv(tv_arena *a, const omt_diagnostic_result *d) {
    const omt_proc_result *c = &d->command;
    tv *m = tv_map(a);
    tv_set_str(a, m, "command", c->command ? c->command : "");
    tv_set(a, m, "returncode", c->has_returncode ? tv_int(a, c->returncode) : tv_none(a));
    tv_set(a, m, "stdout", tv_strn(a, omt_buf_cstr(&c->out), c->out.len));
    tv_set(a, m, "stderr", tv_strn(a, omt_buf_cstr(&c->err), c->err.len));
    tv_set(a, m, "duration_seconds", tv_float(a, c->duration_seconds));
    tv_set_bool(a, m, "timed_out", c->timed_out);
    tv_set_str(a, m, "error", c->error);
    tv_set_bool(a, m, "skipped", d->skipped);
    tv_set_bool(a, m, "stdout_truncated", c->stdout_truncated);
    tv_set_bool(a, m, "stderr_truncated", c->stderr_truncated);
    tv *sources = tv_list(a);
    for (size_t i = 0; i < d->source_count; i++) tv_push(a, sources, tv_str(a, d->sources[i]));
    tv_set(a, m, "sources", sources);
    return m;
}

/* -------------------------------------------------------------- handlers */

static void dashboard(request_ctx *r) {
    if (!require_session(r)) return;
    char token[OMT_SESSION_ID_HEX + 1];
    session_token(r, token);
    tv_arena *a = &r->arena;
    omt_playback_summary *summary = malloc(sizeof(*summary));
    omt_source_choices *choices = malloc(sizeof(*choices));
    omt_video_limit limit;
    if (!summary || !choices) {
        free(summary);
        free(choices);
        error_page(r, "Something went wrong", "The appliance could not complete that request.",
                   500);
        return;
    }
    omt_playback_summary_read(&r->app->playback, summary);
    tv *ctx = common(r, true, token, "dashboard.dashboard");
    omt_playback_sources(&r->app->playback, choices);
    tv *sources = tv_list(a);
    for (size_t i = 0; i < choices->count; i++) {
        tv *s = tv_map(a);
        char buf[256];
        tv_set_str(a, s, "name", choices->names[i]);
        tv_set_str(a, s, "backend", "OMT discovery");
        snprintf(buf, sizeof(buf), "discovered|%s", choices->names[i]);
        tv_set_str(a, s, "selection_value", buf);
        snprintf(buf, sizeof(buf), "%s \xe2\x80\x94 OMT discovery", choices->names[i]);
        tv_set_str(a, s, "display_label", buf);
        tv_push(a, sources, s);
    }
    tv_set(a, ctx, "sources", sources);
    tv_set_str(a, ctx, "current_source", summary->source);
    tv_set_str(a, ctx, "current_direct_target", summary->direct_address);
    tv *pb = tv_map(a);
    tv_set_str(a, pb, "state", summary->state);
    tv_set_str(a, pb, "label", summary->label);
    tv_set_str(a, pb, "detail", summary->detail);
    tv_set_str(a, pb, "tone", summary->tone);
    tv_set_str(a, pb, "source", summary->source);
    tv_set_str(a, pb, "direct_address", summary->direct_address);
    tv_set(a, ctx, "playback", pb);
    omt_playback_video_limit(&r->app->playback, &limit);
    tv_set(a, ctx, "video_limit", video_limit_tv(a, &limit));
    render(r, "dashboard.html", ctx, 200);
    free(summary);
    free(choices);
}

static void login_get(request_ctx *r) {
    if (current_session(r)) {
        redirect(r->res, "/");
        return;
    }
    login_page(r, "", 200);
}

static void login_post(request_ctx *r) {
    omt_app *app = r->app;
    if (!omt_app_allow(app, "login", r->req->peer, app->settings->login_limit)) {
        login_page(r, "Too many login attempts. Please wait.", 429);
        return;
    }
    omt_form form;
    omt_form_parse(r->req->body, r->req->body_len, &form);
    char nonce[128] = "";
    const char *token = omt_form_get(&form, "csrf_token");
    if (!request_cookie(r->req, LOGIN_COOKIE, nonce, sizeof(nonce)) || !token ||
        !omt_auth_verify_csrf(&app->auth, "login", nonce, token)) {
        login_page(r, "Session expired. Please try again.", 400);
        omt_form_free(&form);
        return;
    }
    const char *password = omt_form_get(&form, "password");
    char previous[128];
    bool has_previous = request_cookie(r->req, SESSION_COOKIE, previous, sizeof(previous));
    char session[OMT_SESSION_ID_HEX + 1];
    omt_err err;
    int ok =
        omt_auth_authenticate(&app->auth, password ? password : "", password ? strlen(password) : 0,
                              has_previous ? previous : NULL, session, &err);
    if (ok > 0) {
        redirect(r->res, "/");
        set_cookie(r->res, SESSION_COOKIE, session, app->settings->session_lifetime_s);
        clear_cookie(r->res, LOGIN_COOKIE);
    } else if (ok == 0) {
        login_page(r, "Invalid password", 200);
    } else {
        fprintf(stderr, "unable to create persistent session: %s\n", err.msg);
        login_page(r, "Unable to create a persistent session. Check configuration storage.", 503);
    }
    omt_form_free(&form);
}

static void logout(request_ctx *r) {
    omt_form form;
    if (!authenticated_post(r, &form)) {
        omt_form_free(&form);
        return;
    }
    omt_err err;
    if (!omt_auth_revoke(&r->app->auth, r->session, &err))
        fprintf(stderr, "unable to revoke session: %s\n", err.msg);
    redirect(r->res, "/login");
    clear_cookie(r->res, SESSION_COOKIE);
    omt_form_free(&form);
}

typedef void (*action_fn)(request_ctx *r, const omt_form *form, omt_action_result *out);

static void post_action(request_ctx *r, const char *to, action_fn fn) {
    omt_form form;
    if (authenticated_post(r, &form)) {
        omt_action_result result;
        memset(&result, 0, sizeof(result));
        fn(r, &form, &result);
        flash_redirect(r, to, &result);
    }
    omt_form_free(&form);
}

static const char *field(const omt_form *f, const char *key) {
    const char *v = omt_form_get(f, key);
    return v ? v : "";
}

static void do_select(request_ctx *r, const omt_form *f, omt_action_result *out) {
    omt_playback_select(&r->app->playback, field(f, "source"), out);
}
static void do_restart(request_ctx *r, const omt_form *f, omt_action_result *out) {
    (void)f;
    omt_playback_restart(&r->app->playback, out);
}
static void do_clear(request_ctx *r, const omt_form *f, omt_action_result *out) {
    (void)f;
    omt_playback_clear(&r->app->playback, out);
}
static void do_direct(request_ctx *r, const omt_form *f, omt_action_result *out) {
    omt_span t = omt_utf8_trim(field(f, "direct_address"), strlen(field(f, "direct_address")));
    char address[OMT_TARGET_MAX_BYTES * 2];
    if (t.len >= sizeof(address)) t.len = sizeof(address) - 1;
    memcpy(address, t.p, t.len);
    address[t.len] = 0;
    omt_playback_save_direct(&r->app->playback, address, out);
}
static void do_video_limit(request_ctx *r, const omt_form *f, omt_action_result *out) {
    omt_playback_save_video_limit(&r->app->playback, field(f, "video_limit"), out);
}
static void do_playout_delay(request_ctx *r, const omt_form *f, omt_action_result *out) {
    omt_playback_save_playout_delay(&r->app->playback, field(f, "playout_delay"), out);
}

static void refresh_sources(request_ctx *r) {
    omt_form form;
    if (authenticated_post(r, &form)) {
        omt_playback_refresh(&r->app->playback);
        redirect(r->res, "/");
    }
    omt_form_free(&form);
}

static void network_page(request_ctx *r, const char *submitted, const char *error_override) {
    char token[OMT_SESSION_ID_HEX + 1];
    session_token(r, token);
    tv_arena *a = &r->arena;
    omt_network_configuration network;
    omt_read_network_configuration(r->app->settings->runtime_config_file, &network);
    if (submitted)
        omt_strlcpy(network.discovery_server, submitted, sizeof(network.discovery_server));
    if (error_override) omt_strlcpy(network.error, error_override, sizeof(network.error));
    omt_source_configuration configuration;
    omt_playback_configuration(&r->app->playback, &configuration);
    tv *ctx = common(r, true, token, "network.network_settings");
    tv *n = tv_map(a);
    tv_set_str(a, n, "discovery_server", network.discovery_server);
    tv_set_str(a, n, "discovery_server_text", network.discovery_server);
    tv_set_str(a, n, "error", network.error);
    tv_set(a, ctx, "network", n);
    tv_set_str(a, ctx, "current_source", configuration.source);
    tv_set_str(a, ctx, "current_direct_target", configuration.direct_address);
    tv_set_str(a, ctx, "configuration_error", configuration.error);
    render(r, "network.html", ctx, 200);
}

static void network_get(request_ctx *r) {
    if (require_session(r)) network_page(r, NULL, NULL);
}

static void network_post(request_ctx *r) {
    omt_form form;
    if (!authenticated_post(r, &form)) {
        omt_form_free(&form);
        return;
    }
    const char *submitted = field(&form, "discovery_server");
    omt_err err;
    int changed =
        omt_save_network_configuration(r->app->settings->runtime_config_file, submitted, &err);
    omt_action_result result;
    memset(&result, 0, sizeof(result));
    if (changed == 0) {
        result.ok = true;
        omt_strlcpy(result.message, "OMT discovery settings are already up to date.",
                    sizeof(result.message));
        flash_redirect(r, "/settings/network", &result);
    } else if (changed > 0) {
        omt_playback_refresh(&r->app->playback);
        omt_source_configuration configuration;
        omt_playback_configuration(&r->app->playback, &configuration);
        if (omt_configuration_configured(&configuration)) {
            omt_playback_restart(&r->app->playback, &result);
        } else {
            result.ok = true;
            omt_strlcpy(result.message, "OMT discovery settings saved.", sizeof(result.message));
        }
        flash_redirect(r, "/settings/network", &result);
    } else {
        /* A refused value re-renders the form with what was typed, rather than
         * redirecting the operator's input away. */
        network_page(r, submitted, err.msg);
        if (!r->has_flash_cookie) clear_cookie(r->res, FLASH_COOKIE);
        fprintf(stderr, "network setting rejected: %s\n", err.msg);
    }
    omt_form_free(&form);
}

static void diagnostics_page(request_ctx *r, const omt_diagnostic_result *result,
                             const char *observed_status) {
    char token[OMT_SESSION_ID_HEX + 1];
    session_token(r, token);
    tv_arena *a = &r->arena;
    omt_source_configuration configuration;
    omt_playback_configuration(&r->app->playback, &configuration);
    tv *ctx = common(r, true, token, "diagnostics.diagnostics");
    omt_buf version, status;
    omt_buf_init(&version, 512);
    omt_buf_init(&status, 1024 * 1024);
    omt_app_version(r->app->settings, &version);
    if (observed_status)
        omt_buf_puts(&status, observed_status);
    else
        omt_diagnostics_status(&r->app->diagnostics, &status);
    tv_set_str(a, ctx, "app_version", omt_buf_cstr(&version));
    tv_set_str(a, ctx, "current_source", configuration.source);
    tv_set_str(a, ctx, "current_direct_target", configuration.direct_address);
    tv_set_str(a, ctx, "configuration_error", configuration.error);
    tv_set(a, ctx, "omt_status", tv_strn(a, omt_buf_cstr(&status), status.len));
    if (result) {
        tv *m = tv_map(a);
        tv_set_str(a, m, "title", result->title);
        tv_set(a, m, "command", command_tv(a, result));
        tv_set(a, ctx, "result", m);
    } else {
        tv_set(a, ctx, "result", tv_none(a));
    }
    render(r, "diagnostics.html", ctx, 200);
    omt_buf_free(&version);
    omt_buf_free(&status);
}

static void diagnostics_get(request_ctx *r) {
    if (require_session(r)) diagnostics_page(r, NULL, NULL);
}

typedef enum { DIAG_DISCOVERY, DIAG_RUNTIME, DIAG_DIRECT } diag_kind;

static void diagnostics_action(request_ctx *r, diag_kind kind) {
    omt_form form;
    if (!authenticated_post(r, &form)) {
        omt_form_free(&form);
        return;
    }
    if (!omt_app_allow(r->app, "diagnostics", r->req->peer,
                       r->app->settings->diagnostic_action_limit)) {
        too_many(r);
        omt_form_free(&form);
        return;
    }
    omt_diagnostic_result *result = malloc(sizeof(*result));
    if (!result) {
        error_page(r, "Something went wrong", "The appliance could not complete that request.",
                   500);
        omt_form_free(&form);
        return;
    }
    omt_buf status;
    omt_buf_init(&status, 1024 * 1024);
    if (kind == DIAG_DISCOVERY) {
        omt_playback_refresh(&r->app->playback);
        omt_diagnostics_discovery(&r->app->diagnostics, result);
        diagnostics_page(r, result, NULL);
    } else if (kind == DIAG_RUNTIME) {
        omt_diagnostics_runtime(&r->app->diagnostics, result, &status);
        diagnostics_page(r, result, omt_buf_cstr(&status));
    } else {
        const char *raw = field(&form, "direct_address");
        omt_span t = omt_utf8_trim(raw, strlen(raw));
        char address[OMT_TARGET_MAX_BYTES * 2];
        if (t.len >= sizeof(address)) t.len = sizeof(address) - 1;
        memcpy(address, t.p, t.len);
        address[t.len] = 0;
        omt_diagnostics_direct(&r->app->diagnostics, address, result);
        diagnostics_page(r, result, NULL);
    }
    omt_diagnostic_result_free(result);
    free(result);
    omt_buf_free(&status);
    omt_form_free(&form);
}

static void diagnostics_download(request_ctx *r) {
    omt_form form;
    if (!authenticated_post(r, &form)) {
        omt_form_free(&form);
        return;
    }
    omt_app *app = r->app;
    if (!omt_app_allow(app, "diagnostic-download", r->req->peer,
                       app->settings->diagnostic_download_limit)) {
        too_many(r);
        omt_form_free(&form);
        return;
    }
    const char *include = omt_form_get(&form, "include_packet_capture");
    omt_buf version;
    omt_buf_init(&version, 512);
    omt_app_version(app->settings, &version);
    char filename[64];
    omt_err err;
    omt_buf_clear(&r->res->body);
    if (omt_diagnostics_bundle(&app->diagnostics, include && strcmp(include, "1") == 0,
                               omt_buf_cstr(&version), &r->res->body, filename, &err)) {
        char disposition[128];
        r->res->status = 200;
        omt_http_add_header(r->res, "content-type", "application/zip");
        snprintf(disposition, sizeof(disposition), "attachment; filename=\"%s\"", filename);
        omt_http_add_header(r->res, "content-disposition", disposition);
    } else {
        fprintf(stderr, "support bundle failed: %s\n", err.msg);
        omt_buf_clear(&r->res->body);
        error_page(r, "Something went wrong",
                   "The appliance could not complete that request. Check the container logs.", 500);
    }
    omt_buf_free(&version);
    omt_form_free(&form);
}

static void system_get(request_ctx *r) {
    if (!require_session(r)) return;
    char token[OMT_SESSION_ID_HEX + 1];
    session_token(r, token);
    tv_arena *a = &r->arena;
    tv *ctx = common(r, true, token, "system.system");
    omt_video_limit limit;
    omt_playback_video_limit(&r->app->playback, &limit);
    tv_set(a, ctx, "video_limit", video_limit_tv(a, &limit));
    omt_playout_delay_view delay;
    omt_playback_playout_delay(&r->app->playback, &delay);
    tv *d = tv_map(a);
    tv_set_int(a, d, "milliseconds", (int64_t)delay.milliseconds);
    tv_set_int(a, d, "default_milliseconds", (int64_t)delay.default_milliseconds);
    tv_set_bool(a, d, "overridden", delay.overridden);
    tv_set_str(a, d, "error", delay.error);
    tv_set(a, ctx, "playout_delay", d);
    render(r, "system.html", ctx, 200);
}

static void reboot_get(request_ctx *r) {
    if (!require_session(r)) return;
    char token[OMT_SESSION_ID_HEX + 1];
    session_token(r, token);
    render(r, "reboot_confirm.html", common(r, true, token, "system.confirm_reboot"), 200);
}

static void reboot_post(request_ctx *r) {
    omt_form form;
    if (!authenticated_post(r, &form)) {
        omt_form_free(&form);
        return;
    }
    if (!omt_app_allow(r->app, "reboot", r->req->peer, r->app->settings->reboot_limit)) {
        too_many(r);
        omt_form_free(&form);
        return;
    }
    omt_action_result result;
    omt_diagnostics_request_reboot(&r->app->diagnostics, &result);
    if (result.ok) {
        char token[OMT_SESSION_ID_HEX + 1];
        session_token(r, token);
        tv *ctx = common(r, true, token, "system.reboot");
        tv_set_str(&r->arena, ctx, "message", result.message);
        render(r, "reboot_scheduled.html", ctx, 202);
    } else {
        flash_redirect(r, "/system", &result);
    }
    omt_form_free(&form);
}

static void about(request_ctx *r) {
    if (!require_session(r)) return;
    char token[OMT_SESSION_ID_HEX + 1];
    session_token(r, token);
    tv_arena *a = &r->arena;
    omt_buf version, license, notices;
    omt_buf_init(&version, 512);
    omt_buf_init(&license, 1024 * 1024 + 64);
    omt_buf_init(&notices, 8u * 1024 * 1024 + 64);
    omt_app_version(r->app->settings, &version);
    omt_legal_texts(r->app->settings, &license, &notices);
    tv *ctx = common(r, true, token, "about.about");
    tv_set_str(a, ctx, "app_version", omt_buf_cstr(&version));
    tv_set(a, ctx, "project_license", tv_strn(a, omt_buf_cstr(&license), license.len));
    tv_set(a, ctx, "third_party_notices", tv_strn(a, omt_buf_cstr(&notices), notices.len));
    render(r, "about.html", ctx, 200);
    omt_buf_free(&version);
    omt_buf_free(&license);
    omt_buf_free(&notices);
}

static void not_found(request_ctx *r) {
    if (current_session(r))
        error_page(r, "Page not found", "That page does not exist.", 404);
    else
        login_page(r, "That page does not exist.", 404);
}

static void select_source(request_ctx *r) { post_action(r, "/", do_select); }
static void restart_playback(request_ctx *r) { post_action(r, "/", do_restart); }
static void clear_playback(request_ctx *r) { post_action(r, "/", do_clear); }
static void direct_source(request_ctx *r) { post_action(r, "/settings/network", do_direct); }
static void video_limit_post(request_ctx *r) { post_action(r, "/system", do_video_limit); }
static void playout_delay_post(request_ctx *r) { post_action(r, "/system", do_playout_delay); }
static void diagnostics_discovery(request_ctx *r) { diagnostics_action(r, DIAG_DISCOVERY); }
static void diagnostics_runtime(request_ctx *r) { diagnostics_action(r, DIAG_RUNTIME); }
static void diagnostics_direct(request_ctx *r) { diagnostics_action(r, DIAG_DIRECT); }

typedef void (*handler_fn)(request_ctx *r);

/* The route table: one row per path, with its GET and POST handlers. Every
 * path here is documented in docs/CODEBASE_REFERENCE.md or OPERATIONS.md. */
static const struct {
    const char *path;
    handler_fn get;
    handler_fn post;
} ROUTES[] = {
    {"/login", login_get, login_post},
    {"/logout", NULL, logout},
    {"/", dashboard, NULL},
    {"/sources/select", NULL, select_source},
    {"/sources/refresh", NULL, refresh_sources},
    {"/playback/restart", NULL, restart_playback},
    {"/playback/clear", NULL, clear_playback},
    {"/settings/network", network_get, network_post},
    {"/settings/direct-source", NULL, direct_source},
    {"/diagnostics", diagnostics_get, NULL},
    {"/diagnostics/discovery", NULL, diagnostics_discovery},
    {"/diagnostics/runtime", NULL, diagnostics_runtime},
    {"/diagnostics/direct", NULL, diagnostics_direct},
    {"/diagnostics/download", NULL, diagnostics_download},
    {"/system", system_get, NULL},
    {"/system/video-limit", NULL, video_limit_post},
    {"/system/playout-delay", NULL, playout_delay_post},
    {"/system/reboot", reboot_get, reboot_post},
    {"/about", about, NULL},
};

static void security_headers(omt_http_response *res, bool is_static) {
    omt_http_add_header(res, "strict-transport-security", "max-age=31536000; includeSubDomains");
    omt_http_add_header(res, "x-frame-options", "DENY");
    omt_http_add_header(res, "x-content-type-options", "nosniff");
    omt_http_add_header(res, "referrer-policy", "strict-origin-when-cross-origin");
    omt_http_add_header(res, "content-security-policy", CSP);
    if (is_static) {
        omt_http_add_header(res, "cache-control", "public, max-age=86400");
    } else {
        omt_http_add_header(res, "cache-control", "no-store");
        omt_http_add_header(res, "pragma", "no-cache");
    }
}

void omt_app_handle(void *context, const omt_http_request *req, omt_http_response *res) {
    request_ctx r;
    memset(&r, 0, sizeof(r));
    r.app = context;
    r.req = req;
    r.res = res;
    tv_arena_init(&r.arena);
    char flash[64];
    r.has_flash_cookie = request_cookie(req, FLASH_COOKIE, flash, sizeof(flash));
    bool get = strcmp(req->method, "GET") == 0 || strcmp(req->method, "HEAD") == 0;
    bool post = strcmp(req->method, "POST") == 0;
    bool is_static = omt_has_prefix(req->path, "/static/");
    if (is_static && get &&
        (strcmp(req->path, "/static/style.css") == 0 ||
         strcmp(req->path, "/static/favicon.svg") == 0)) {
        const char *data, *type;
        size_t len;
        if (omt_static_asset(req->path + 8, &data, &len, &type)) {
            res->status = 200;
            omt_http_add_header(res, "content-type", type);
            omt_buf_append(&res->body, data, len);
        }
    } else {
        bool matched = false;
        for (size_t i = 0; i < OMT_ARRAY_LEN(ROUTES) && !matched; i++) {
            if (strcmp(ROUTES[i].path, req->path) != 0) continue;
            matched = true;
            handler_fn fn = get ? ROUTES[i].get : post ? ROUTES[i].post : NULL;
            if (fn) {
                fn(&r);
            } else {
                /* A known path with the wrong method, as axum answered it. */
                res->status = 405;
                char allow[32];
                snprintf(allow, sizeof(allow), "%s%s%s", ROUTES[i].get ? "GET,HEAD" : "",
                         ROUTES[i].get && ROUTES[i].post ? "," : "", ROUTES[i].post ? "POST" : "");
                omt_http_add_header(res, "allow", allow);
            }
        }
        if (!matched) not_found(&r);
    }
    security_headers(res, is_static);
    tv_arena_free(&r.arena);
}
