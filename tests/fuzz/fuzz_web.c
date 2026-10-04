/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Everything an unauthenticated client or a damaged file can hand the Web
 * frontend: HTTP request heads, form bodies, cookie headers, the OMT
 * settings XML, saved-state records, and stored password hashes.
 */
#include <stdlib.h>
#include <string.h>

#include "web/app.h"
#include "web/auth.h"
#include "web/diagnostics.h"
#include "web/http.h"
#include "web/network.h"
#include "web/playback.h"

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size);

int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
    omt_http_request req;
    size_t head, length;
    bool keep;
    (void)omt_http_parse_head(data, size, &req, &head, &length, &keep);
    omt_form form;
    omt_form_parse(data, size, &form);
    omt_form_free(&form);
    char *text = malloc(size + 1);
    if (!text) return 0;
    memcpy(text, data, size);
    text[size] = 0;
    char value[128], server[600];
    (void)omt_cookie_value(text, "__Host-omt_session", value, sizeof(value));
    omt_err err;
    (void)omt_parse_server(data, size, server, &err);
    omt_buf updated;
    omt_buf_init(&updated, 65536);
    bool changed;
    (void)omt_update_settings_xml(data, size, "omt://example.com:6399", &updated, &changed, &err);
    omt_buf_free(&updated);
    (void)omt_normalize_server(text, server, &err);
    omt_source_choices *choices = malloc(sizeof(*choices));
    if (choices) {
        omt_parse_discovered(text, size, choices);
        free(choices);
    }
    char state[32], detail[2100];
    (void)omt_status_record_valid(text, size, "Camera", 5, state, sizeof(state), detail,
                                  sizeof(detail));
    static const char *const fields[] = {"version", "request_id", "status"};
    const char *v[3];
    size_t l[3];
    (void)omt_parse_record(text, size, fields, 3, true, v, l);
    /* Password parsing only: verification would spend the fuzzer's time in
     * PBKDF2 and scrypt rather than in parsing. */
    omt_password p;
    if (omt_password_parse(text, size, &p, &err)) omt_password_free(&p);
    free(text);
    return 0;
}
