# Compiler and linker flags for the C tree. Included by mk/c.mk.
#
# These flags stand in for what the Rust workspace got from the language:
# warnings are errors, implicit conversions are warnings, and every binary is
# a hardened PIE. The safety rules that no flag expresses -- the banned libc
# calls -- are enforced by force-including src/common/banned.h.

C_STD      := -std=c17
C_WARN     := -Wall -Wextra -Werror -Wconversion -Wsign-conversion -Wshadow \
              -Wformat=2 -Wformat-security -Wvla -Wimplicit-fallthrough \
              -Wstrict-prototypes -Wmissing-prototypes -Wcast-qual \
              -Wnull-dereference -Wundef -Wpointer-arith -Wwrite-strings
C_HARDEN   := -fstack-protector-strong -fno-strict-aliasing -fno-common \
              -ftrivial-auto-var-init=zero
C_DEFS     := -D_GNU_SOURCE -DOMT_VERSION='"$(OMT_VERSION)"'
C_INCLUDE  := -Isrc -include src/common/banned.h

LD_HARDEN  := -Wl,-z,relro,-z,now,-z,noexecstack

# Per-target additions. TARGET_OS is linux or windows, TARGET_ARCH the
# compiler's machine name.
ifeq ($(TARGET_OS),linux)
C_HARDEN   += -fstack-clash-protection -fPIE
LD_HARDEN  += -pie
endif
ifeq ($(TARGET_ARCH),aarch64)
C_HARDEN   += -mbranch-protection=standard
endif
ifeq ($(TARGET_OS),windows)
# mingw's C99-conformant stdio, so %zu and friends mean what they mean
# everywhere else.
C_DEFS     += -D_WIN32_WINNT=0x0A00 -DWIN32_LEAN_AND_MEAN -DUNICODE -D_UNICODE \
              -D__USE_MINGW_ANSI_STDIO=1
LD_HARDEN  := -Wl,--dynamicbase -Wl,--nxcompat -Wl,--high-entropy-va
endif

ifeq ($(BUILD),release)
C_OPT      := -O2 -U_FORTIFY_SOURCE -D_FORTIFY_SOURCE=3 -flto
LD_OPT     := -flto -s
else ifeq ($(BUILD),analyze)
C_OPT      := -O1 -fanalyzer -Wno-analyzer-too-complex
LD_OPT     :=
else ifeq ($(BUILD),fuzz)
# Every library is instrumented for coverage, not just the harnesses, or
# libFuzzer would steer blind.
C_OPT      := -O1 -g -fno-omit-frame-pointer -fsanitize=address,undefined,fuzzer-no-link \
              -fno-sanitize-recover=all
LD_OPT     := -fsanitize=address,undefined
else ifeq ($(BUILD),asan)
C_OPT      := -O1 -g -fno-omit-frame-pointer -fsanitize=address,undefined \
              -fno-sanitize-recover=all
LD_OPT     := -fsanitize=address,undefined
else
C_OPT      := -O0 -g
LD_OPT     :=
endif

# A non-system OpenSSL (the pinned Windows build from scripts/build-openssl.sh).
# Its headers are system headers to the warning flags above.
ifneq ($(OPENSSL_PREFIX),)
C_INCLUDE  += -isystem $(OPENSSL_PREFIX)/include
LD_OPENSSL := -L$(OPENSSL_PREFIX)/lib
endif

CFLAGS_ALL  = $(C_STD) $(C_WARN) $(C_HARDEN) $(C_DEFS) $(C_INCLUDE) $(C_OPT) $(CFLAGS)
LDFLAGS_ALL = $(LD_HARDEN) $(LD_OPT) $(LD_OPENSSL) $(LDFLAGS)
