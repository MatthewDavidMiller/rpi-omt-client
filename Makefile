# Raspberry Pi OMT Client Build System
# Usage: make [target]

.PHONY: help install setup-arm64-emulation build build-arm64 build-amd64 build-deployer build-windows-deployer deploy up down logs lint test test-quick test-py test-receiver test-deployer test-setup security-scan clean

IMAGE_NAME   := omt-client
ARM64_TARBALL := omt-client-arm64.tar.gz
DEV_COMPOSE  := docker-compose.dev.yml
BUILD_METADATA_DIR := .build
RPI_OMT_CLIENT_VERSION ?= $(shell ./scripts/detect-version.sh "$(CURDIR)")
TEST_PYTHON := tests/.venv/bin/python

# Default target
help:
	@echo "RPi OMT Client Build System"
	@echo ""
	@echo "Build targets:"
	@echo "  build-arm64   Build ARM64 image (for Raspberry Pi), output: $(ARM64_TARBALL)"
	@echo "  build-amd64   Build amd64 image locally (for testing)"
	@echo "  build         Alias for build-arm64"
	@echo "  build-deployer Test and publish the native deployer for this host"
	@echo "  build-windows-deployer  Cross-compile the Windows x86-64 deployer (Linux host)"
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
	@echo ""
	@echo "Quality targets:"
	@echo "  lint          Run ruff + hadolint + shellcheck + yamllint"
	@echo "  test          Run all tests (unit + live container build)"
	@echo "  test-quick    Run every unit suite, no container engine (~1m)"
	@echo "  test-py       Run Python unit tests only (requires test-setup)"
	@echo "  test-receiver Build and test the native receiver with sanitizers"
	@echo "  test-deployer Build and test the native deployer"
	@echo "  test-setup    Bootstrap Python test tooling"
	@echo "  security-scan Run Trivy filesystem + image scans"
	@echo "  clean         Remove build artifacts and stopped containers"
	@echo ""
	@echo "Build prerequisite: Docker with buildx support and ARM64 emulation"
	@echo "Live test prerequisite: Docker or Podman (auto-detected)"

# Install local developer prerequisites and Python test tooling
install:
	./scripts/install-dev-deps.sh
	$(MAKE) test-setup
	./scripts/setup-hooks.sh

setup-arm64-emulation:
	./scripts/install-arm64-emulation.sh

# Build ARM64 image tarball (for Pi deployment)
build-arm64:
	IMAGE_NAME="$(IMAGE_NAME)" \
	ARM64_TARBALL="$(abspath $(ARM64_TARBALL))" \
	BUILD_METADATA_DIR="$(abspath $(BUILD_METADATA_DIR))" \
	RPI_OMT_CLIENT_VERSION="$(RPI_OMT_CLIENT_VERSION)" \
		./scripts/build-arm64.sh

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

build-deployer:
	RPI_OMT_CLIENT_VERSION="$(RPI_OMT_CLIENT_VERSION)" ./scripts/check-deployer.sh --publish

# Cross-compile the Windows x86-64 deployer from Linux with mingw-w64.
build-windows-deployer:
	RPI_OMT_CLIENT_VERSION="$(RPI_OMT_CLIENT_VERSION)" ./scripts/build-windows-deployer.sh

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
	./scripts/lint.sh
	python3 ./scripts/check-legal-notices.py

# Run all tests (unit + live container build)
test:
	@echo "Running all tests..."
	./scripts/test-local.sh

# Run every unit suite, no container engine (~1m)
test-quick:
	./scripts/test-local.sh --quick

# Run Python unit tests only (requires 'make test-setup' first)
test-py:
	@if [ ! -x "$(TEST_PYTHON)" ]; then \
		echo "Run 'make test-setup' first to install Python test tools"; exit 1; fi
	$(TEST_PYTHON) -m pytest tests/unit \
		--cov=src/omt_client --cov-report=term-missing --cov-fail-under=98 -v

test-receiver:
	./tools/test-receiver.sh

test-deployer:
	./scripts/check-deployer.sh

security-scan:
	./scripts/security-scan.sh

# Bootstrap pytest dev venv (run once)
test-setup:
	@echo "Setting up Python test venv..."
	python3 -m venv tests/.venv
	$(TEST_PYTHON) -m pip install --upgrade pip -q
	$(TEST_PYTHON) -m pip install -r tests/requirements-dev.txt
	@echo "Done. Run: make test-py test-receiver test-deployer"

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	rm -f $(ARM64_TARBALL)
	rm -rf $(BUILD_METADATA_DIR)
	docker compose -f $(DEV_COMPOSE) down --remove-orphans 2>/dev/null || true
	docker rmi $(IMAGE_NAME):dev 2>/dev/null || true
	@echo "Clean complete"
