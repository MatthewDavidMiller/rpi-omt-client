# The C build. Every product is a static archive of one source directory plus
# the binaries that link them; there is no configure step and no generated
# makefile, so a clean build is a single `make -f mk/c.mk` of a few seconds.
#
#   make -f mk/c.mk [BUILD=release|debug|asan] [CC=...] <target>
#
# Outputs land in build/c/<triple>/<build>/, so cross builds, sanitizer builds,
# and release builds never share an object file.

.SUFFIXES:
.DELETE_ON_ERROR:
.DEFAULT_GOAL := all

BUILD     ?= release
# Sanitizer builds use clang: Alpine's GCC ships no libasan for musl, and the
# toolbox is Alpine. An explicit CC on the command line still wins.
ifeq ($(origin CC),default)
CC := $(if $(filter asan fuzz,$(BUILD)),clang,cc)
endif
PYTHON    ?= python3
OMT_VERSION ?= $(shell ./scripts/detect-version.sh "$(CURDIR)")

TRIPLE      := $(shell $(CC) -dumpmachine)
TARGET_ARCH := $(firstword $(subst -, ,$(TRIPLE)))
ifneq ($(findstring mingw,$(TRIPLE)),)
TARGET_OS   := windows
EXE         := .exe
PLATFORM    := win32
else
TARGET_OS   := linux
EXE         :=
PLATFORM    := posix
endif

OUT := build/c/$(TRIPLE)/$(BUILD)-$(notdir $(firstword $(CC)))

# Release objects carry LTO bytecode, so archives are built with the
# compiler's own plugin-aware ar. An explicit AR on the command line wins.
ifeq ($(origin AR),default)
ifneq ($(findstring clang,$(CC)),)
AR := llvm-ar
else ifneq ($(filter %gcc,$(CC)),)
AR := $(CC)-ar
else
AR := gcc-ar
endif
endif

include mk/flags.mk

# Outputs depend on the flags they were built with, not only on their sources,
# through three stamps that are rewritten only when their text changes.
#
#   * Objects depend on the compile flags.
#   * Binaries depend on the link flags, so check-deployer.sh's
#     LDFLAGS=-static-pie relinks the build check-c.sh just compiled rather
#     than recompiling it.
#   * The version is not a compile flag at all. It is stamped into one object,
#     common/version.o, because the gates build the same output directory with
#     different versions (check-c.sh uses OMT_VERSION=check). As a -D on every
#     file it made each gate run recompile the whole tree; without any stamp a
#     release build would link the gate's object and report the wrong version.
FLAGS_STAMP   := $(OUT)/.flags
FLAGS_TEXT    := $(CC) $(CFLAGS_ALL) $(ASFLAGS_ALL)
LINK_STAMP    := $(OUT)/.ldflags
LINK_TEXT     := $(CC) $(LDFLAGS_ALL)
VERSION_STAMP := $(OUT)/.version
ifneq ($(file < $(FLAGS_STAMP)),$(FLAGS_TEXT))
$(shell mkdir -p $(OUT))
$(file > $(FLAGS_STAMP),$(FLAGS_TEXT))
endif
ifneq ($(file < $(LINK_STAMP)),$(LINK_TEXT))
$(shell mkdir -p $(OUT))
$(file > $(LINK_STAMP),$(LINK_TEXT))
endif
ifneq ($(file < $(VERSION_STAMP)),$(OMT_VERSION))
$(shell mkdir -p $(OUT))
$(file > $(VERSION_STAMP),$(OMT_VERSION))
endif

$(OUT)/obj/common/version.o: FILE_CFLAGS := -DOMT_VERSION='"$(OMT_VERSION)"'
$(OUT)/obj/common/version.o: $(VERSION_STAMP)

# A source file named *_posix.c or *_win32.c only builds for its platform.
# Assembly (*.S) builds everywhere; each file guards its body on the
# architecture it is written for, so elsewhere it assembles to nothing.
OTHER_PLATFORM := $(if $(filter win32,$(PLATFORM)),posix,win32)
sources = $(filter-out %_$(OTHER_PLATFORM).c,$(wildcard $(1)/*.c)) $(wildcard $(1)/*.S)
objects = $(patsubst src/%.S,$(OUT)/obj/%.o,$(patsubst src/%.c,$(OUT)/obj/%.o,$(call sources,$(1))))

# Quiet by default; V=1 prints every command.
ifeq ($(V),1)
Q :=
else
Q := @
endif

$(OUT)/obj/%.o: src/%.c $(FLAGS_STAMP)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  CC      $<")
	$(Q)$(CC) $(CFLAGS_ALL) $(FILE_CFLAGS) -MMD -MP -c $< -o $@

$(OUT)/obj/%.o: src/%.S $(FLAGS_STAMP)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  AS      $<")
	$(Q)$(CC) $(ASFLAGS_ALL) -MMD -MP -c $< -o $@

$(OUT)/obj/tests/%.o: tests/c/%.c $(FLAGS_STAMP)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  CC      $<")
	$(Q)$(CC) $(CFLAGS_ALL) -MMD -MP -c $< -o $@

# ---------------------------------------------------------------- libraries
LIB_COMMON := $(OUT)/lib/libcommon.a
$(LIB_COMMON): $(call objects,src/common)
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

LIB_PROTOCOL := $(OUT)/lib/libprotocol.a
$(LIB_PROTOCOL): $(call objects,src/protocol)
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

LIB_VMX := $(OUT)/lib/libvmx.a
$(LIB_VMX): $(call objects,src/vmx)
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

LIB_RECEIVER_CORE := $(OUT)/lib/libreceiver_core.a
$(LIB_RECEIVER_CORE): $(call objects,src/receiver_core)
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

# Everything in src/receiver except the entry point, so the suites can link it.
LIB_RECEIVER := $(OUT)/lib/libreceiver.a
$(LIB_RECEIVER): $(filter-out %/main.o,$(call objects,src/receiver))
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

LIB_CRYPTO := $(OUT)/lib/libomtcrypto.a
$(LIB_CRYPTO): $(call objects,src/crypto)
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

LIB_WEB := $(OUT)/lib/libweb.a
$(LIB_WEB): $(filter-out %/main.o,$(call objects,src/web))
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

# The deployer: its core (with the platform layer) and the SSH client, in one
# archive because each calls the other. The capsule, which embeds the
# appliance image, is linked only into the deployer binaries, so this builds
# without the image.
LIB_DEPLOY := $(OUT)/lib/libdeploy.a
$(LIB_DEPLOY): $(call objects,src/deploy/core) $(call objects,src/deploy/ssh)
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

# The terminal application without its entry point, so the suites can link it.
LIB_TUI := $(OUT)/lib/libtui.a
$(LIB_TUI): $(filter-out %/main.o,$(call objects,src/deploy/tui))
	@mkdir -p $(@D)
	$(Q)rm -f $@ && $(AR) rcs $@ $^

include mk/targets.mk

-include $(shell find $(OUT) -name '*.d' 2>/dev/null)

.PHONY: all clean-c print-out
print-out:
	@echo $(OUT)
clean-c:
	rm -rf build/c
