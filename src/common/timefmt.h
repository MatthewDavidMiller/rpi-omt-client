/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Calendar arithmetic for the timestamps the status contract carries. The
 * conversion is the civil-from-days algorithm, so no time zone database or
 * locale is consulted: every instant is UTC.
 */
#ifndef OMT_TIMEFMT_H
#define OMT_TIMEFMT_H

#include "common/base.h"

/* Wall-clock time since the Unix epoch. */
void omt_wall_clock(int64_t *seconds, uint32_t *nanos);
/* Formats "YYYY-MM-DDTHH:MM:SS.mmmZ" (24 bytes) into out[32]. */
void omt_format_rfc3339_millis(int64_t seconds, uint32_t millis, char out[32]);
/* Formats "YYYYMMDDTHHMMSSZ" into out[32]. */
void omt_format_compact_utc(int64_t seconds, char out[32]);
/* Converts a proleptic Gregorian civil date to days since 1970-01-01. */
int64_t omt_days_from_civil(int64_t year, unsigned month, unsigned day);
void omt_civil_from_days(int64_t days, int64_t *year, unsigned *month, unsigned *day);

/* Parses an RFC 3339 date-time (the form the time crate's Rfc3339 accepts):
 * YYYY-MM-DDTHH:MM:SS[.fraction](Z|+HH:MM|-HH:MM), case-insensitive T and Z.
 * Returns the instant as seconds and nanoseconds since the epoch. */
OMT_NODISCARD bool omt_parse_rfc3339(const char *s, int64_t *seconds, uint32_t *nanos);

#endif
