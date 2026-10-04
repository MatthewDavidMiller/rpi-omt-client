/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include <stdio.h>
#include <string.h>

#include "receiver_core/core.h"

#define CEILING_MAX_WIDTH 1920
#define CEILING_MAX_HEIGHT 1080
#define CEILING_MAX_FPS 60
#define CEILING_MIN_DIMENSION 16

/* Digits only, so a leading '+', a leading zero, or surrounding space is a
 * rejection rather than something a lenient parser would quietly accept. */
static bool number(const char *s, size_t len, int32_t *out) {
    if (len == 0 || len > 4 || s[0] == '0') return false;
    uint64_t v;
    if (!omt_parse_u64(s, len, 9999, &v)) return false;
    *out = (int32_t)v;
    return true;
}

static bool shape(const char *field, size_t len, omt_shape *out, omt_err *err) {
    const char *at = memchr(field, '@', len);
    const char *x = at ? memchr(field, 'x', (size_t)(at - field)) : NULL;
    if (!at || !x || !number(field, (size_t)(x - field), &out->width) ||
        !number(x + 1, (size_t)(at - x - 1), &out->height) ||
        !number(at + 1, len - (size_t)(at - field) - 1, &out->fps)) {
        omt_err_set(err, "Invalid video ceiling: %.*s. Expected WIDTHxHEIGHT@FPS.", (int)len,
                    field);
        return false;
    }
    if (out->width < CEILING_MIN_DIMENSION || out->width > CEILING_MAX_WIDTH ||
        out->height < CEILING_MIN_DIMENSION || out->height > CEILING_MAX_HEIGHT || out->fps < 1 ||
        out->fps > CEILING_MAX_FPS) {
        omt_err_set(err, "Video ceiling %.*s is outside the supported %dx%d at %d fps maximum.",
                    (int)len, field, CEILING_MAX_WIDTH, CEILING_MAX_HEIGHT, CEILING_MAX_FPS);
        return false;
    }
    return true;
}

bool omt_ceiling_parse(const char *text, omt_video_ceiling *out, omt_err *err) {
    memset(out, 0, sizeof(*out));
    const char *p = text;
    for (;;) {
        const char *comma = strchr(p, ',');
        size_t len = comma ? (size_t)(comma - p) : strlen(p);
        if (out->count == OMT_CEILING_MAX_SHAPES) {
            omt_err_set(err, "A video ceiling may list at most %d resolutions.",
                        OMT_CEILING_MAX_SHAPES);
            return false;
        }
        if (!shape(p, len, &out->shapes[out->count], err)) return false;
        out->count++;
        if (!comma) break;
        p = comma + 1;
    }
    return true;
}

void omt_ceiling_describe(const omt_video_ceiling *c, omt_buf *out) {
    for (size_t i = 0; i < c->count; i++) {
        if (i) omt_buf_puts(out, ", or ");
        omt_buf_printf(out, "%dx%d at %d fps", c->shapes[i].width, c->shapes[i].height,
                       c->shapes[i].fps);
    }
}

bool omt_ceiling_admits(const omt_video_ceiling *c, int32_t width, int32_t height, double rate,
                        omt_err *err) {
    /* The rate is compared with a tolerance because 59.94 arrives as
     * 60000/1001 and must not be refused by a 60 fps ceiling. */
    for (size_t i = 0; i < c->count; i++)
        if (width <= c->shapes[i].width && height <= c->shapes[i].height &&
            rate <= (double)c->shapes[i].fps + 0.01)
            return true;
    omt_buf d;
    omt_buf_init(&d, 512);
    omt_ceiling_describe(c, &d);
    omt_err_set(err, "%dx%d at %.2f fps exceeds this appliance's limit of %s.", width, height, rate,
                omt_buf_cstr(&d));
    omt_buf_free(&d);
    return false;
}
