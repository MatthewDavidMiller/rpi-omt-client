# Raspberry Pi OMT Client Build System
# Usage: make [target]

.PHONY: help install setup-arm64-emulation build build-arm64 build-amd64 build-deployer build-windows-deployer release build-omt-sender omt-sender-start omt-sender-stop omt-sender-status omt-sender-firewall-allow omt-sender-firewall-remove deploy up down logs lint test test-quick test-web test-receiver test-deployer test-setup security-scan clean

IMAGE_NAME   := omt-client
ARM64_TARBALL := omt-client-arm64.tar.gz
DEV_COMPOSE  := docker-compose.dev.yml
BUILD_METADATA_DIR := .build
RPI_OMT_CLIENT_VERSION ?= $(shell ./scripts/detect-version.sh "$(CURDIR)")
TEST_PYTHON := tests/.venv/bin/python
OMT_SENDER_TARGET ?= auto

# Every gate runs inside the toolbox image, so Docker or Podman is the only
# thing this repository needs from a workstation. scripts/toolbox.sh builds the
# image on first use and execs straight through when it is already inside one.
TOOLBOX := ./scripts/toolbox.sh

# Default target
help:
	@echo "RPi OMT Client Build System"
	@echo ""
	@echo "Build targets:"
	@echo "  build-arm64   Build ARM64 image (for Raspberry Pi), output: $(ARM64_TARBALL)"
	@echo "  build-amd64   Build amd64 image locally (for testing)"
	@echo "  build         Alias for build-arm64"
	@echo "  build-deployer Test and publish the Linux CLI + TUI (static musl)"
	@echo "  build-windows-deployer  Cross-compile the Windows CLI + egui GUI"
	@echo "                 Both embed $(ARM64_TARBALL), so run build-arm64 first"
	@echo "                 The post-commit hook runs all three: a published"
	@echo "                 artifact carries the version of its own commit"
	@echo "  release       Build locally, tag and push HEAD, and create the"
	@echo "                 GitHub Release (requires an authenticated gh CLI)"
	@echo "  build-omt-sender  Build the first-party Rust OMT A/V sender"
	@echo ""
	@echo "Deploy targets:"
	@echo "  deploy HOST=user@ip  Copy ARM64 image to Pi and start container"
	@echo "                       Example: make deploy HOST=pi@192.168.1.100"
	@echo ""
	@echo "Dev targets:"
	@echo "  setup-arm64-emulation  Install persistent ARM64 emulation on Linux x86-64"
	@echo "  up            Start local dev container (amd64)"
	@echo "  down          Stop local dev container"
	@echo "  logs          Follow local dev container logs"
	@echo "  omt-sender-start/status/stop  Manage the local OMT A/V test sender"
	@echo "  omt-sender-firewall-allow SOURCE=IP_OR_CIDR  Allow a receiver source"
	@echo "  omt-sender-firewall-remove SOURCE=IP_OR_CIDR Remove that allowance"
	@echo ""
	@echo "Quality targets:"
	@echo "  lint          Run ruff + hadolint + shellcheck + yamllint"
	@echo "  test          Run all tests (unit + live container build)"
	@echo "  test-quick    Run every unit suite, no container engine (~1m)"
	@echo "  test-web      Run Rust Web frontend tests"
	@echo "  test-receiver Build and test the Rust receiver and test sender"
	@echo "  test-deployer Build and test the Rust deployer"
	@echo "  test-setup    Bootstrap a host Python venv (not needed for the toolbox)"
	@echo "  security-scan Run Trivy filesystem + image scans"
	@echo "  clean         Remove build artifacts and stopped containers"
	@echo ""
	@echo "Prerequisite: Docker or Podman. Nothing else -- every gate runs"
	@echo "inside the toolbox image that 'make install' builds."

# Build the gate toolbox and install the git hooks. Nothing is installed onto
# the host: the compilers, linters, scanners, and Python tooling all live in
# the image. scripts/install-dev-deps.sh remains for anyone who wants that
# toolchain on the host as well, and is no longer required by any gate.
install:
	$(TOOLBOX) --build
	./scripts/setup-hooks.sh

setup-arm64-emulation:
	./scripts/install-arm64-emulation.sh

# Build ARM64 image tarball (for Pi deployment)
build-arm64:
	IMAGE_NAME="$(IMAGE_NAME)" \
	ARM64_TARBALL="$(abspath $(ARM64_TARBALL))" \
	BUILD_METADATA_DIR="$(abspath $(BUILD_METADATA_DIR))" \
	RPI_OMT_CLIENT_VERSION="$(RPI_OMT_CLIENT_VERSION)" \
		$(TOOLBOX) ./scripts/build-arm64.sh

# Build amd64 image locally (for testing)
build-amd64:
	@echo "Building amd64 image..."
	@mkdir -p $(BUILD_METADATA_DIR)
	docker build -f deploy/Dockerfile \
		--build-arg RPI_OMT_CLIENT_VERSION="$(RPI_OMT_CLIENT_VERSION)" \
		--iidfile "$(BUILD_METADATA_DIR)/amd64.iid" -t $(IMAGE_NAME):dev .
	@echo "Image digest: $$(cat $(BUILD_METADATA_DIR)/amd64.iid)"
	@echo "Built: $(IMAGE_NAME):dev"

build: build-arm64

# The deployer embeds the manifest-v3 capsule, appliance image included, so the
# image is one of its build inputs: run build-arm64 first. Not a make
# prerequisite on purpose -- an emulated ARM64 image build takes tens of
# minutes, and starting one from a target named "build the deployer" would be a
# surprise rather than a convenience. Both build scripts say so and stop.
build-deployer:
	RPI_OMT_CLIENT_VERSION="$(RPI_OMT_CLIENT_VERSION)" $(TOOLBOX) ./scripts/check-deployer.sh --publish

# Cross-compile the Windows x86-64 deployer from Linux with mingw-w64.
build-windows-deployer:
	RPI_OMT_CLIENT_VERSION="$(RPI_OMT_CLIENT_VERSION)" $(TOOLBOX) ./scripts/build-windows-deployer.sh

# This remains a local pipeline: the script runs the release builds above on
# this workstation, then uses the authenticated GitHub CLI only for the final
# push and Release API call.
release:
	./scripts/publish-github-release.sh

build-omt-sender:
	./scripts/build-omt-test-sender.sh --target "$(OMT_SENDER_TARGET)"

omt-sender-start:
	./scripts/omt-test-sender.sh start

omt-sender-stop:
	./scripts/omt-test-sender.sh stop

omt-sender-status:
	./scripts/omt-test-sender.sh status

omt-sender-firewall-allow:
	@if [ -z "$(SOURCE)" ]; then \
		echo "ERROR: SOURCE is required. Usage: make $@ SOURCE=<receiver-ip-or-cidr>"; \
		exit 1; \
	fi
	./scripts/configure-omt-test-sender-firewall.sh allow "$(SOURCE)"

omt-sender-firewall-remove:
	@if [ -z "$(SOURCE)" ]; then \
		echo "ERROR: SOURCE is required. Usage: make $@ SOURCE=<receiver-ip-or-cidr>"; \
		exit 1; \
	fi
	./scripts/configure-omt-test-sender-firewall.sh remove "$(SOURCE)"

# Deploy ARM64 image to Raspberry Pi
# Usage: make deploy HOST=pi@192.168.1.100
deploy:
	@if [ -z "$(HOST)" ]; then \
		echo "ERROR: HOST is required. Usage: make deploy HOST=pi@<ip>"; \
		exit 1; \
	fi
	@if [ ! -f "$(ARM64_TARBALL)" ]; then \
		echo "ERROR: $(ARM64_TARBALL) not found. Run 'make build-arm64' first."; \
		exit 1; \
	fi
	./scripts/deploy.sh "$(HOST)"

# Local dev container targets
up:
	docker compose -f $(DEV_COMPOSE) up -d --build

down:
	docker compose -f $(DEV_COMPOSE) down

logs:
	docker compose -f $(DEV_COMPOSE) logs -f

# Lint
lint:
	$(TOOLBOX) ./scripts/lint.sh
	$(TOOLBOX) ./scripts/check-no-c-sources.sh
	$(TOOLBOX) python3 ./scripts/check-legal-notices.py

# Run all tests (unit + live container build)
test:
	@echo "Running all tests..."
	$(TOOLBOX) ./scripts/test-local.sh

# Run every unit suite, no container engine (~1m)
test-quick:
	$(TOOLBOX) ./scripts/test-local.sh --quick

# Build and test the Rust Web frontend.
test-web:
	$(TOOLBOX) cargo test --locked -p omt-web

test-receiver:
	$(TOOLBOX) ./tools/test-receiver.sh

test-deployer:
	$(TOOLBOX) ./scripts/check-deployer.sh

security-scan:
	$(TOOLBOX) ./scripts/security-scan.sh

# Bootstrap pytest dev venv (run once)
test-setup:
	@echo "Setting up Python test venv..."
	python3 -m venv tests/.venv
	$(TEST_PYTHON) -m pip install --upgrade pip -q
	$(TEST_PYTHON) -m pip install -r tests/requirements-dev.txt
	@echo "Done. Run: make test-web test-receiver test-deployer"

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	rm -f $(ARM64_TARBALL)
	rm -rf $(BUILD_METADATA_DIR)
	rm -rf target
	docker compose -f $(DEV_COMPOSE) down --remove-orphans 2>/dev/null || true
	docker rmi $(IMAGE_NAME):dev 2>/dev/null || true
	@echo "Clean complete"
