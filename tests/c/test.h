/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The first-party test harness: one binary per suite, each test a function,
 * and every failed check reported with its file and line. A suite exits
 * non-zero when any check failed, so `make -f mk/c.mk test` fails with it.
 */
#ifndef OMT_TEST_H
#define OMT_TEST_H

#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Allocation for test fixtures: a suite that cannot allocate stops at once
 * rather than testing a NULL. */
static inline void *test_alloc(size_t n) {
    void *p = calloc(1, n ? n : 1);
    if (!p) {
        fprintf(stderr, "test fixture allocation failed\n");
        exit(2);
    }
    return p;
}

static int omt_test_failures;
static int omt_test_checks;

#define CHECK(cond)                                                                                \
    do {                                                                                           \
        omt_test_checks++;                                                                         \
        if (!(cond)) {                                                                             \
            omt_test_failures++;                                                                   \
            fprintf(stderr, "%s:%d: CHECK(%s) failed\n", __FILE__, __LINE__, #cond);               \
        }                                                                                          \
    } while (0)

#define CHECK_MSG(cond, ...)                                                                       \
    do {                                                                                           \
        omt_test_checks++;                                                                         \
        if (!(cond)) {                                                                             \
            omt_test_failures++;                                                                   \
            fprintf(stderr, "%s:%d: CHECK(%s) failed: ", __FILE__, __LINE__, #cond);               \
            fprintf(stderr, __VA_ARGS__);                                                          \
            fputc('\n', stderr);                                                                   \
        }                                                                                          \
    } while (0)

#define CHECK_INT(a, b)                                                                            \
    do {                                                                                           \
        long long omt_a_ = (long long)(a), omt_b_ = (long long)(b);                                \
        omt_test_checks++;                                                                         \
        if (omt_a_ != omt_b_) {                                                                    \
            omt_test_failures++;                                                                   \
            fprintf(stderr, "%s:%d: %s == %s failed: %lld != %lld\n", __FILE__, __LINE__, #a, #b,  \
                    omt_a_, omt_b_);                                                               \
        }                                                                                          \
    } while (0)

#define CHECK_STR(a, b)                                                                            \
    do {                                                                                           \
        const char *omt_a_ = (a), *omt_b_ = (b);                                                   \
        omt_test_checks++;                                                                         \
        if (!omt_a_ || !omt_b_ || strcmp(omt_a_, omt_b_) != 0) {                                   \
            omt_test_failures++;                                                                   \
            fprintf(stderr, "%s:%d: %s == %s failed: \"%s\" != \"%s\"\n", __FILE__, __LINE__, #a,  \
                    #b, omt_a_ ? omt_a_ : "(null)", omt_b_ ? omt_b_ : "(null)");                   \
        }                                                                                          \
    } while (0)

#define RUN(test)                                                                                  \
    do {                                                                                           \
        int omt_before_ = omt_test_failures;                                                       \
        test();                                                                                    \
        fprintf(stderr, "%s %s\n", omt_test_failures == omt_before_ ? "ok  " : "FAIL", #test);     \
    } while (0)

#define TEST_EXIT()                                                                                \
    (fprintf(stderr, "%d checks, %d failures\n", omt_test_checks, omt_test_failures),              \
     omt_test_failures ? 1 : 0)

#endif
