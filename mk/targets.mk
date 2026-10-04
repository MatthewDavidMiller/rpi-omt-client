# Products of the C build, grouped by the component that owns them. mk/c.mk
# includes this after the shared rules, so every list here can use them.

BIN := $(OUT)/bin

# --------------------------------------------------------------- receiver
RECEIVER_LIBS := $(LIB_RECEIVER) $(LIB_RECEIVER_CORE) $(LIB_VMX) $(LIB_PROTOCOL) $(LIB_COMMON)
RECEIVER_SYSLIBS := -lasound -lpthread -lm

$(OUT)/obj/receiver/audio_alsa.o: FILE_CFLAGS := -DOMT_ALSA_HEADERS

# The decoder is the hot path. Its lane loops are written to be vectorized,
# which LLVM does at -O3; measured on a 1080p frame that is twice the speed
# of -O2, and faster than the Rust decoder it replaced. Debug and sanitizer
# builds keep their own levels.
ifeq ($(BUILD),release)
$(patsubst src/%.c,$(OUT)/obj/%.o,$(wildcard src/vmx/*.c)): FILE_CFLAGS := -O3
endif

$(BIN)/omt-receiver: $(OUT)/obj/receiver/main.o $(RECEIVER_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(RECEIVER_LIBS) $(RECEIVER_SYSLIBS)

# The sender embeds two conformance frames with .incbin, which the compiler's
# dependency output cannot see.
$(OUT)/obj/sender/main.o: tests/vectors/vmx/gradient-1920x1080-709.vmx \
                          tests/vectors/vmx/flat-1920x1080-709.vmx

$(BIN)/omt-test-sender: $(OUT)/obj/sender/main.o $(LIB_PROTOCOL) $(LIB_COMMON)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(LIB_PROTOCOL) $(LIB_COMMON) -lpthread -lm

# ------------------------------------------------------------------- web
WEB_LIBS := $(LIB_WEB) $(LIB_CRYPTO) $(LIB_PROTOCOL) $(LIB_COMMON)
WEB_SYSLIBS := -lssl -lcrypto -lm

$(BIN)/omt-web: $(OUT)/obj/web/main.o $(WEB_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(WEB_LIBS) $(WEB_SYSLIBS)

.PHONY: receiver sender web
receiver: $(BIN)/omt-receiver
sender: $(BIN)/omt-test-sender
web: $(BIN)/omt-web

# --------------------------------------------------------------- deployer
# The capsule embeds every manifest-v3 member, the ARM64 image included, with
# .incbin. The generator's depfile makes both the assembler file and its object
# depend on each member, so a rebuilt image or an edited host script
# re-assembles it.
CAPSULE_FORMAT := $(if $(filter windows,$(TARGET_OS)),pe,elf)
comma := ,
CAPSULE_ASFLAGS := $(if $(filter linux,$(TARGET_OS)),-Wa$(comma)--noexecstack)

$(OUT)/gen/capsule.S: tools/gen/gen_capsule.py deploy/manifest-v3.txt
	@mkdir -p $(@D)
	@$(if $(Q),echo "  GEN     $@")
	$(Q)$(PYTHON) tools/gen/gen_capsule.py --root . --format $(CAPSULE_FORMAT) \
	    --output $@ --depfile $(OUT)/gen/capsule.d --object $(OUT)/obj/gen/capsule.o

$(OUT)/obj/gen/%.o: $(OUT)/gen/%.S
	@mkdir -p $(@D)
	@$(if $(Q),echo "  AS      $<")
	$(Q)$(CC) $(CAPSULE_ASFLAGS) -c $< -o $@

CAPSULE_OBJS := $(OUT)/obj/gen/capsule.o $(OUT)/obj/deploy/capsule/capsule.o
DEPLOY_LIBS := $(LIB_DEPLOY) $(LIB_CRYPTO) $(LIB_COMMON)
ifeq ($(TARGET_OS),windows)
# Fully static apart from the system DLLs: an operator installs nothing.
DEPLOY_SYSLIBS := -static -lssl -lcrypto -lws2_32 -lcrypt32 -lbcrypt -lgdi32 -luser32 -ladvapi32
else
DEPLOY_SYSLIBS := -lssl -lcrypto -lpthread
endif

$(BIN)/rpi-omt-deploy$(EXE): $(OUT)/obj/deploy/cli/main.o $(CAPSULE_OBJS) $(DEPLOY_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(CAPSULE_OBJS) $(DEPLOY_LIBS) $(DEPLOY_SYSLIBS)

TUI_OBJS := $(call objects,src/deploy/tui)
$(BIN)/rpi-omt-deploy-tui$(EXE): $(TUI_OBJS) $(CAPSULE_OBJS) $(DEPLOY_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $(TUI_OBJS) $(CAPSULE_OBJS) $(DEPLOY_LIBS) $(DEPLOY_SYSLIBS)

.PHONY: deployer deploy-libs
deployer: $(BIN)/rpi-omt-deploy$(EXE) $(BIN)/rpi-omt-deploy-tui$(EXE)
# Everything but the capsule, for the gates that run without the image.
deploy-libs: $(DEPLOY_LIBS) $(TUI_OBJS) $(OUT)/obj/deploy/cli/main.o \
             $(OUT)/obj/deploy/capsule/capsule.o

# ----------------------------------------------------------------- tests
# Archives are listed dependents-first so a single link pass resolves them.
TEST_LIBS := $(LIB_WEB) $(LIB_CRYPTO) $(RECEIVER_LIBS)
TEST_SYSLIBS := $(RECEIVER_SYSLIBS) -lssl -lcrypto

# The deployer suites are their own set: they link the deployer libraries and
# a capsule, and the real capsule needs the ARM64 image built first.
TEST_NAMES := $(patsubst tests/c/test_%.c,%,$(filter-out tests/c/test_deploy%,$(wildcard tests/c/test_*.c)))
TEST_BINS  := $(patsubst %,$(OUT)/tests/test_%$(EXE),$(TEST_NAMES))

$(OUT)/tests/test_%$(EXE): $(OUT)/obj/tests/test_%.o $(TEST_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(TEST_LIBS) $(TEST_SYSLIBS)

.PHONY: tests test
tests: $(TEST_BINS)

# Every suite runs from the repository root, so a vector path is the same in
# every build directory. A failing suite fails the target; none is skipped.
test: $(TEST_BINS)
	@set -e; for t in $(TEST_BINS); do echo "== $$t"; $(TEST_RUNNER) $$t; done

all: receiver sender web

# Deployer suites. Each links a small fixture capsule generated into the build
# tree, except test_deploy_capsule, which holds the real one to its manifest.
FIXTURE_ROOT := $(OUT)/fixture-capsule
$(FIXTURE_ROOT)/.stamp: mk/targets.mk
	@rm -rf $(FIXTURE_ROOT) && mkdir -p $(FIXTURE_ROOT)/deploy/host
	@printf 'version=3\nomt-client-arm64.tar.gz\ndeploy/host/set-hostname.sh\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n' \
	    >$(FIXTURE_ROOT)/deploy/manifest-v3.txt
	@printf '\037\213fixture image\n' >$(FIXTURE_ROOT)/omt-client-arm64.tar.gz
	@printf '#!/bin/sh\necho "=== Appliance hostname set ==="\n' >$(FIXTURE_ROOT)/deploy/host/set-hostname.sh
	@printf '#!/bin/bash\n' >$(FIXTURE_ROOT)/deploy/transaction.sh
	@printf 'MIT License\n' >$(FIXTURE_ROOT)/LICENSE
	@printf 'THIRD-PARTY NOTICES\n' >$(FIXTURE_ROOT)/THIRD_PARTY_NOTICES.txt
	@touch $@

$(OUT)/gen/fixture_capsule.S: $(FIXTURE_ROOT)/.stamp tools/gen/gen_capsule.py
	@mkdir -p $(@D)
	$(Q)$(PYTHON) tools/gen/gen_capsule.py --root $(FIXTURE_ROOT) --format $(CAPSULE_FORMAT) --output $@

DEPLOY_TEST_NAMES := $(patsubst tests/c/test_%.c,%,$(filter-out tests/c/test_deploy_capsule.c,$(wildcard tests/c/test_deploy*.c)))
DEPLOY_TEST_BINS := $(patsubst %,$(OUT)/tests/test_%$(EXE),$(DEPLOY_TEST_NAMES))
FIXTURE_CAPSULE_OBJS := $(OUT)/obj/gen/fixture_capsule.o $(OUT)/obj/deploy/capsule/capsule.o

$(DEPLOY_TEST_BINS): $(OUT)/tests/test_%$(EXE): $(OUT)/obj/tests/test_%.o $(FIXTURE_CAPSULE_OBJS) $(LIB_TUI) $(DEPLOY_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(FIXTURE_CAPSULE_OBJS) $(LIB_TUI) $(DEPLOY_LIBS) $(DEPLOY_SYSLIBS)

$(OUT)/tests/test_deploy_capsule$(EXE): $(OUT)/obj/tests/test_deploy_capsule.o $(CAPSULE_OBJS) $(LIB_TUI) $(DEPLOY_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(CAPSULE_OBJS) $(LIB_TUI) $(DEPLOY_LIBS) $(DEPLOY_SYSLIBS)

# The SSH interop driver for tests/integration/test_ssh_client.sh.
$(OUT)/tests/ssh_interop$(EXE): $(OUT)/obj/tests/ssh_interop.o $(FIXTURE_CAPSULE_OBJS) $(DEPLOY_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(FIXTURE_CAPSULE_OBJS) $(DEPLOY_LIBS) $(DEPLOY_SYSLIBS)

# The VMX decode benchmark: a measurement, run by hand on each board.
$(OUT)/tests/bench_vmx$(EXE): $(OUT)/obj/tests/bench_vmx.o $(LIB_VMX) $(LIB_COMMON)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -o $@ $< $(LIB_VMX) $(LIB_COMMON) -lpthread -lm

.PHONY: bench
bench: $(OUT)/tests/bench_vmx$(EXE)
	$<

.PHONY: deploy-tests test-deploy test-deploy-capsule
deploy-tests: $(DEPLOY_TEST_BINS)
test-deploy: $(DEPLOY_TEST_BINS)
	@set -e; for t in $(DEPLOY_TEST_BINS); do echo "== $$t"; $(TEST_RUNNER) $$t; done
test-deploy-capsule: $(OUT)/tests/test_deploy_capsule$(EXE)
	$(TEST_RUNNER) $<

# ------------------------------------------------------------------ fuzz
# libFuzzer targets, one per parser family; build with BUILD=fuzz (clang). `fuzz-smoke` runs each
# for FUZZ_SECONDS from its committed corpus, so a regression in a parser the
# suites do not reach still fails the gate.
FUZZ_NAMES := $(patsubst tests/fuzz/fuzz_%.c,%,$(wildcard tests/fuzz/fuzz_*.c))
FUZZ_BINS  := $(patsubst %,$(OUT)/fuzz/fuzz_%,$(FUZZ_NAMES))
FUZZ_SECONDS ?= 60

$(OUT)/obj/fuzz/%.o: tests/fuzz/%.c $(FLAGS_STAMP)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  CC      $<")
	$(Q)$(CC) $(CFLAGS_ALL) -MMD -MP -c $< -o $@

# The SSH harness links the deployer, with the fixture capsule standing in.
$(OUT)/fuzz/fuzz_ssh: $(OUT)/obj/fuzz/fuzz_ssh.o $(FIXTURE_CAPSULE_OBJS) $(DEPLOY_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -fsanitize=fuzzer -o $@ $< $(FIXTURE_CAPSULE_OBJS) $(DEPLOY_LIBS) $(DEPLOY_SYSLIBS)

$(OUT)/fuzz/fuzz_%: $(OUT)/obj/fuzz/fuzz_%.o $(TEST_LIBS)
	@mkdir -p $(@D)
	@$(if $(Q),echo "  LD      $@")
	$(Q)$(CC) $(LDFLAGS_ALL) -fsanitize=fuzzer -o $@ $< $(TEST_LIBS) $(TEST_SYSLIBS)

# Each target is its own make goal, so `make -j` runs them side by side within
# the job budget instead of one after another; every one still gets the full
# FUZZ_SECONDS. A target's output goes to its own log, printed when it fails,
# so concurrent runs cannot interleave into an unreadable report.
FUZZ_RUNS := $(patsubst %,fuzz-run-%,$(FUZZ_NAMES))

.PHONY: fuzz fuzz-smoke $(FUZZ_RUNS)
fuzz: $(FUZZ_BINS)
fuzz-smoke: $(FUZZ_RUNS)
$(FUZZ_RUNS): fuzz-run-%: $(OUT)/fuzz/fuzz_%
	@mkdir -p $(OUT)/fuzz/work/$*
	@echo "== fuzz_$* ($(FUZZ_SECONDS)s)"
	@if LSAN_OPTIONS=suppressions=$(CURDIR)/tests/fuzz/lsan.supp \
	    $(OUT)/fuzz/fuzz_$* -max_total_time=$(FUZZ_SECONDS) -rss_limit_mb=2048 \
	        -max_len=1048576 $(OUT)/fuzz/work/$* tests/fuzz/corpus/$* \
	        >$(OUT)/fuzz/work/$*.log 2>&1; then \
	    echo "== fuzz_$* passed"; \
	else \
	    cat $(OUT)/fuzz/work/$*.log; echo "== fuzz_$* FAILED"; exit 1; \
	fi

# ---------------------------------------------------------- link stamp
# Every linked output relinks when the link flags change (see mk/c.mk). A rule
# without a recipe only adds the prerequisite; each still links by its own rule.
LINKED := $(BIN)/omt-receiver $(BIN)/omt-test-sender $(BIN)/omt-web \
          $(BIN)/rpi-omt-deploy$(EXE) $(BIN)/rpi-omt-deploy-tui$(EXE) \
          $(TEST_BINS) $(DEPLOY_TEST_BINS) $(OUT)/tests/test_deploy_capsule$(EXE) \
          $(OUT)/tests/ssh_interop$(EXE) $(OUT)/tests/bench_vmx$(EXE) $(FUZZ_BINS)
$(LINKED): $(LINK_STAMP)
