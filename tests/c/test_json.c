/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 */
#include "common/json.h"
#include "test.h"

static omt_json *parse(omt_json_doc *doc, const char *text) {
    return omt_json_parse(doc, text, strlen(text), 0);
}

static void duplicate_keys_are_rejected_at_every_depth(void) {
    omt_json_doc doc;
    CHECK(!parse(&doc, "{\"schema\":1,\"schema\":1}"));
    CHECK(!parse(&doc, "{\"outer\":{\"value\":1,\"value\":2}}"));
    CHECK(!parse(&doc, "[{\"a\":1,\"a\":1}]"));
    CHECK(parse(&doc, "{\"schema\":1}"));
    omt_json_doc_free(&doc);
}

static void strict_grammar(void) {
    const char *bad[] = {
        "",          "{",           "[1,]",        "{\"a\":1,}",   "01",
        "1.",        ".5",          "-",           "+1",           "\"\\x\"",
        "\"\x01\"",  "\"\\ud800\"", "\"\\udc00\"", "nul",          "true false",
        "{\"a\" 1}", "[1 2]",       "\"\xff\"",    "\"\xc0\x80\"", "\"\xed\xa0\x80\"",
        "1e",        "NaN"};
    for (size_t i = 0; i < OMT_ARRAY_LEN(bad); i++) {
        omt_json_doc doc;
        CHECK_MSG(!parse(&doc, bad[i]), "accepted %s", bad[i]);
    }
    omt_json_doc doc;
    omt_json *v = parse(&doc, " {\"s\":\"a\\u00e9\\ud83d\\ude00\\n\",\"n\":-12,\"f\":1.5e2,"
                              "\"b\":true,\"z\":null,\"a\":[]} ");
    CHECK(v != NULL);
    CHECK_STR(omt_json_as_str(omt_json_get(v, "s")), "a\xc3\xa9\xf0\x9f\x98\x80\n");
    int64_t n = 0;
    CHECK(omt_json_as_i64(omt_json_get(v, "n"), &n));
    CHECK_INT(n, -12);
    uint64_t u = 0;
    CHECK(!omt_json_as_u64(omt_json_get(v, "n"), &u));
    CHECK(!omt_json_as_u64(omt_json_get(v, "f"), &u));
    CHECK(omt_json_get(v, "f")->number == 150.0);
    CHECK(omt_json_get(v, "b")->boolean);
    CHECK(omt_json_get(v, "z")->type == OMT_JSON_NULL);
    const char *allowed[] = {"s", "n", "f", "b", "z", "a"};
    CHECK(omt_json_only_keys(v, allowed, 6));
    CHECK(!omt_json_only_keys(v, allowed, 5));
    omt_json_doc_free(&doc);

    /* An embedded NUL is legal JSON but never a usable C string. */
    v = parse(&doc, "\"a\\u0000b\"");
    CHECK(v && v->string_len == 3);
    CHECK(omt_json_as_str(v) == NULL);
    omt_json_doc_free(&doc);
}

static void depth_is_bounded(void) {
    char deep[600];
    memset(deep, '[', 300);
    memset(deep + 300, ']', 300);
    omt_json_doc doc;
    CHECK(!omt_json_parse(&doc, deep, 600, 0));
    memset(deep, '[', 100);
    memset(deep + 100, ']', 100);
    CHECK(omt_json_parse(&doc, deep, 200, 0));
    omt_json_doc_free(&doc);
}

static void write_string_matches_serde(void) {
    omt_buf b;
    omt_buf_init(&b, 1024);
    omt_json_write_cstr(&b, "a\"b\\c\n\x01/\x7f\xc3\xa9");
    CHECK_STR(omt_buf_cstr(&b), "\"a\\\"b\\\\c\\n\\u0001/\x7f\xc3\xa9\"");
    omt_buf_free(&b);
}

static void write_f64_matches_serde(void) {
    struct {
        double v;
        const char *s;
    } cases[] = {{60.0, "60.0"},
                 {59.94, "59.94"},
                 {0.5, "0.5"},
                 {1e16, "1e16"},
                 {1e15, "1000000000000000.0"},
                 {0.0001, "0.0001"},
                 {1e-5, "0.00001"},
                 {1e-6, "1e-6"},
                 {-2.25, "-2.25"},
                 {29.97002997002997, "29.97002997002997"},
                 {123456.789, "123456.789"},
                 {1.5e-7, "1.5e-7"},
                 {0.0, "0.0"}};
    for (size_t i = 0; i < OMT_ARRAY_LEN(cases); i++) {
        omt_buf b;
        omt_buf_init(&b, 64);
        omt_json_write_f64(&b, cases[i].v);
        CHECK_STR(omt_buf_cstr(&b), cases[i].s);
        omt_buf_free(&b);
    }
}

/* A lone lead byte at the end of the input, found by fuzz_web through a
 * Cookie header: the decoder stepped past the data and trim's span wrapped. */
static void truncated_utf8_never_steps_past_the_input(void) {
    size_t i = 0;
    (void)omt_utf8_next("\xcc", 1, &i);
    CHECK_INT(i, 1);
    omt_span t = omt_utf8_trim("\xcc", 1);
    CHECK(t.len <= 1);
    t = omt_utf8_trim("a \xf0\x9f", 4);
    CHECK(t.len <= 4);
}

int main(void) {
    RUN(duplicate_keys_are_rejected_at_every_depth);
    RUN(strict_grammar);
    RUN(depth_is_bounded);
    RUN(write_string_matches_serde);
    RUN(write_f64_matches_serde);
    RUN(truncated_utf8_never_steps_past_the_input);
    return TEST_EXIT();
}
