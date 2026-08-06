// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
#include "json_text.h"

#include <string.h>

void omt_text_init(omt_text_buffer *buffer, char *storage, size_t capacity)
{
    buffer->data = storage;
    buffer->length = 0u;
    buffer->capacity = capacity;
    buffer->failed = capacity == 0u;
    if (capacity != 0u) {
        storage[0] = '\0';
    }
}

bool omt_text_append_n(omt_text_buffer *buffer, const char *text, size_t length)
{
    if (buffer->failed || length >= buffer->capacity - buffer->length) {
        buffer->failed = true;
        return false;
    }
    memcpy(buffer->data + buffer->length, text, length);
    buffer->length += length;
    buffer->data[buffer->length] = '\0';
    return true;
}

bool omt_text_append(omt_text_buffer *buffer, const char *text)
{
    return omt_text_append_n(buffer, text, strlen(text));
}

bool omt_text_append_char(omt_text_buffer *buffer, char value)
{
    return omt_text_append_n(buffer, &value, 1u);
}

bool omt_json_append_string(omt_text_buffer *buffer, const char *value)
{
    static const char hex[] = "0123456789abcdef";
    const unsigned char *cursor = (const unsigned char *)value;
    if (!omt_text_append_char(buffer, '"')) {
        return false;
    }
    while (*cursor != 0u) {
        unsigned char ch = *cursor++;
        const char *escape = NULL;
        switch (ch) {
        case '"': escape = "\\\""; break;
        case '\\': escape = "\\\\"; break;
        case '\b': escape = "\\b"; break;
        case '\f': escape = "\\f"; break;
        case '\n': escape = "\\n"; break;
        case '\r': escape = "\\r"; break;
        case '\t': escape = "\\t"; break;
        default: break;
        }
        if (escape != NULL) {
            (void)omt_text_append(buffer, escape);
        } else if (ch < 0x20u) {
            char encoded[7] = {'\\', 'u', '0', '0', hex[ch >> 4u], hex[ch & 0x0fu], '\0'};
            (void)omt_text_append(buffer, encoded);
        } else {
            (void)omt_text_append_char(buffer, (char)ch);
        }
    }
    return omt_text_append_char(buffer, '"');
}

bool omt_json_string(char *output, size_t capacity, const char *value)
{
    omt_text_buffer buffer;
    omt_text_init(&buffer, output, capacity);
    return omt_json_append_string(&buffer, value) && !buffer.failed;
}
