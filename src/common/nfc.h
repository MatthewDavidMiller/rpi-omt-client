/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Unicode Normalization Form C, for the one question the validators ask:
 * is this string already in NFC?
 */
#ifndef OMT_NFC_H
#define OMT_NFC_H

#include "common/base.h"
#include "common/buf.h"

/* Normalizes valid UTF-8 `s` to NFC into `out` (appended). Returns false when
 * the input is not valid UTF-8 or the output would pass out->limit. */
bool omt_nfc_normalize(const char *s, size_t len, omt_buf *out);
/* True when valid UTF-8 `s` is unchanged by NFC. Inputs longer than
 * `limit` bytes are refused (false), keeping the work bounded. */
bool omt_nfc_is_normalized(const char *s, size_t len, size_t limit);
uint8_t omt_nfc_combining_class(uint32_t cp);

#endif
