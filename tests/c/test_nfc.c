/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/nfc.h"
#include "test.h"

static size_t unhex(const char *hex, size_t n, char *out) {
    size_t len = 0;
    for (size_t i = 0; i + 1 < n; i += 2) {
        unsigned v;
        if (sscanf(hex + i, "%2x", &v) != 1) return 0;
        out[len++] = (char)v;
    }
    return len;
}

static void matches_the_python_oracle(void) {
    FILE *f = fopen("tests/vectors/nfc/vectors.txt", "r");
    CHECK(f != NULL);
    if (!f) return;
    char line[1024];
    int count = 0;
    while (fgets(line, sizeof(line), f)) {
        char *space = strchr(line, ' ');
        if (!space) continue;
        char input[256], expected[256];
        size_t in_len = unhex(line, (size_t)(space - line), input);
        size_t exp_len = unhex(space + 1, strcspn(space + 1, "\n"), expected);
        omt_buf out;
        omt_buf_init(&out, 1024);
        CHECK(omt_nfc_normalize(input, in_len, &out));
        bool same = out.len == exp_len && memcmp(out.data, expected, exp_len) == 0;
        CHECK_MSG(same, "line %d: %.*s", count + 1, (int)(space - line), line);
        bool normalized = in_len == exp_len && memcmp(input, expected, in_len) == 0;
        CHECK_MSG(omt_nfc_is_normalized(input, in_len, 1024) == normalized, "line %d", count + 1);
        omt_buf_free(&out);
        count++;
    }
    fclose(f);
    CHECK(count > 5000);
}

static void source_name_examples(void) {
    CHECK(omt_nfc_is_normalized("Camera \xf0\x9f\x98\x80", 11, 64));
    CHECK(!omt_nfc_is_normalized("Cafe\xcc\x81", 6, 64));
    CHECK(omt_nfc_is_normalized("Caf\xc3\xa9", 5, 64));
    CHECK(!omt_nfc_is_normalized("\xff", 1, 64));
    CHECK(!omt_nfc_is_normalized("abc", 3, 2));
}

int main(void) {
    RUN(matches_the_python_oracle);
    RUN(source_name_examples);
    return TEST_EXIT();
}
