/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/timefmt.h"

#include <stdio.h>
#include <time.h>

void omt_wall_clock(int64_t *seconds, uint32_t *nanos) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    *seconds = (int64_t)ts.tv_sec;
    *nanos = (uint32_t)ts.tv_nsec;
}

void omt_civil_from_days(int64_t days, int64_t *year, unsigned *month, unsigned *day) {
    int64_t shifted = days + 719468;
    int64_t era = (shifted >= 0 ? shifted : shifted - 146096) / 146097;
    int64_t day_of_era = shifted - era * 146097;
    int64_t year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    int64_t y = year_of_era + era * 400;
    int64_t day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    int64_t month_prime = (5 * day_of_year + 2) / 153;
    int64_t d = day_of_year - (153 * month_prime + 2) / 5 + 1;
    int64_t m = month_prime + (month_prime < 10 ? 3 : -9);
    if (m <= 2) y += 1;
    *year = y;
    *month = (unsigned)m;
    *day = (unsigned)d;
}

int64_t omt_days_from_civil(int64_t year, unsigned month, unsigned day) {
    int64_t y = year - (month <= 2 ? 1 : 0);
    int64_t era = (y >= 0 ? y : y - 399) / 400;
    int64_t year_of_era = y - era * 400;
    int64_t m = month;
    int64_t day_of_year = (153 * (m > 2 ? m - 3 : m + 9) + 2) / 5 + (int64_t)day - 1;
    int64_t day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    return era * 146097 + day_of_era - 719468;
}

static void split(int64_t seconds, int64_t *year, unsigned *month, unsigned *day, unsigned *hour,
                  unsigned *minute, unsigned *second) {
    int64_t days = seconds / 86400;
    int64_t rest = seconds % 86400;
    if (rest < 0) {
        rest += 86400;
        days -= 1;
    }
    omt_civil_from_days(days, year, month, day);
    *hour = (unsigned)(rest / 3600);
    *minute = (unsigned)(rest % 3600 / 60);
    *second = (unsigned)(rest % 60);
}

void omt_format_rfc3339_millis(int64_t seconds, uint32_t millis, char out[32]) {
    int64_t year;
    unsigned month, day, hour, minute, second;
    split(seconds, &year, &month, &day, &hour, &minute, &second);
    snprintf(out, 32, "%04lld-%02u-%02uT%02u:%02u:%02u.%03uZ", (long long)year, month, day, hour,
             minute, second, (unsigned)(millis % 1000));
}

void omt_format_compact_utc(int64_t seconds, char out[32]) {
    int64_t year;
    unsigned month, day, hour, minute, second;
    split(seconds, &year, &month, &day, &hour, &minute, &second);
    snprintf(out, 32, "%04lld%02u%02uT%02u%02u%02uZ", (long long)year, month, day, hour, minute,
             second);
}

static bool digits(const char *s, int n, unsigned *out) {
    unsigned v = 0;
    for (int i = 0; i < n; i++) {
        if (s[i] < '0' || s[i] > '9') return false;
        v = v * 10 + (unsigned)(s[i] - '0');
    }
    *out = v;
    return true;
}

static bool leap(int64_t y) { return (y % 4 == 0 && y % 100 != 0) || y % 400 == 0; }

bool omt_parse_rfc3339(const char *s, int64_t *seconds, uint32_t *nanos) {
    unsigned year, month, day, hour, minute, second;
    if (!digits(s, 4, &year) || s[4] != '-' || !digits(s + 5, 2, &month) || s[7] != '-' ||
        !digits(s + 8, 2, &day) || (s[10] != 'T' && s[10] != 't') || !digits(s + 11, 2, &hour) ||
        s[13] != ':' || !digits(s + 14, 2, &minute) || s[16] != ':' || !digits(s + 17, 2, &second))
        return false;
    static const unsigned days_in[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
    if (month < 1 || month > 12 || day < 1) return false;
    unsigned dim = days_in[month - 1] + (month == 2 && leap(year) ? 1u : 0u);
    /* A leap second (:60) is accepted and folded into the next second, as
     * the time crate does for RFC 3339 input. */
    if (day > dim || hour > 23 || minute > 59 || second > 60) return false;
    const char *p = s + 19;
    uint32_t frac = 0;
    if (*p == '.') {
        p++;
        int n = 0;
        while (*p >= '0' && *p <= '9') {
            if (n < 9) frac = frac * 10 + (uint32_t)(*p - '0');
            n++;
            p++;
        }
        if (n == 0) return false;
        for (int i = n; i < 9; i++) frac *= 10;
    }
    int64_t offset = 0;
    if (*p == 'Z' || *p == 'z') {
        p++;
    } else if (*p == '+' || *p == '-') {
        unsigned oh, om;
        if (!digits(p + 1, 2, &oh) || p[3] != ':' || !digits(p + 4, 2, &om) || oh > 23 || om > 59)
            return false;
        offset = (int64_t)(oh * 3600 + om * 60) * (*p == '-' ? -1 : 1);
        p += 6;
    } else {
        return false;
    }
    if (*p) return false;
    int64_t days = omt_days_from_civil(year, month, day);
    *seconds = days * 86400 + hour * 3600 + minute * 60 + second - offset;
    *nanos = frac;
    return true;
}
