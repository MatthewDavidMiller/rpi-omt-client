/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * Unicode tables for NFC, generated into nfc_tables.c. Each is sorted by its
 * first column for binary search.
 */
#ifndef OMT_NFC_TABLES_H
#define OMT_NFC_TABLES_H

#include <stddef.h>
#include <stdint.h>

typedef struct {
    uint32_t cp;
    uint8_t ccc;
} omt_nfc_ccc;

typedef struct {
    uint32_t cp;
    uint32_t first;
    uint32_t second; /* 0 for a singleton mapping */
} omt_nfc_decomp;

typedef struct {
    uint32_t first;
    uint32_t second;
    uint32_t composite;
} omt_nfc_comp;

extern const omt_nfc_ccc omt_nfc_ccc_table[];
extern const omt_nfc_decomp omt_nfc_decomp_table[];
extern const omt_nfc_comp omt_nfc_comp_table[];
extern const size_t omt_nfc_ccc_count;
extern const size_t omt_nfc_decomp_count;
extern const size_t omt_nfc_comp_count;

#endif
