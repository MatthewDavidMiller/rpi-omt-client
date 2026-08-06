// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#ifndef OMT_RECEIVER_JSON_TEXT_H
#define OMT_RECEIVER_JSON_TEXT_H

#include <stdbool.h>
#include <stddef.h>

typedef struct {
    char *data;
    size_t length;
    size_t capacity;
    bool failed;
} omt_text_buffer;

void omt_text_init(omt_text_buffer *buffer, char *storage, size_t capacity);
bool omt_text_append(omt_text_buffer *buffer, const char *text);
bool omt_text_append_n(omt_text_buffer *buffer, const char *text, size_t length);
bool omt_text_append_char(omt_text_buffer *buffer, char value);
bool omt_json_append_string(omt_text_buffer *buffer, const char *value);
bool omt_json_string(char *output, size_t capacity, const char *value);

#endif
