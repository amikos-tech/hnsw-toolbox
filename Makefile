.PHONY: build build-debug build-release clean test test-go test-rust test-all lint lint-go lint-rust fmt fmt-go fmt-rust help

GOLANGCI_LINT_VERSION := v2.5.0
GOLANGCI_LINT_CMD := go run github.com/golangci/golangci-lint/v2/cmd/golangci-lint@$(GOLANGCI_LINT_VERSION)

UNAME_S := $(shell uname -s 2>/dev/null || echo UNKNOWN)
OS_ENV := $(OS)

ifeq ($(UNAME_S),UNKNOWN)
$(warning uname -s failed; OS detection will rely on OS env and fallback rules)
endif

ifeq ($(OS_ENV),Windows_NT)
	HOST_OS := windows
else ifneq (,$(findstring MINGW,$(UNAME_S)))
	HOST_OS := windows
else ifneq (,$(findstring MSYS,$(UNAME_S)))
	HOST_OS := windows
else ifneq (,$(findstring CYGWIN,$(UNAME_S)))
	HOST_OS := windows
else ifeq ($(UNAME_S),Darwin)
	HOST_OS := darwin
else ifeq ($(UNAME_S),Linux)
	HOST_OS := linux
else ifeq ($(UNAME_S),UNKNOWN)
	HOST_OS := linux
else
	HOST_OS := linux
$(warning unrecognized uname -s '$(UNAME_S)'; defaulting HOST_OS to linux)
endif

ifeq ($(HOST_OS),darwin)
	LIB_NAME := libhnsw_toolbox.dylib
else ifeq ($(HOST_OS),windows)
	LIB_NAME := hnsw_toolbox.dll
else
	LIB_NAME := libhnsw_toolbox.so
endif

TARGET_DIR_ENV := $(strip $(CARGO_TARGET_DIR))

ifeq ($(TARGET_DIR_ENV),)
	TARGET_DIR := target
else ifneq ($(filter /%,$(TARGET_DIR_ENV)),)
	TARGET_DIR := $(TARGET_DIR_ENV)
else ifneq ($(findstring :,$(TARGET_DIR_ENV)),)
	TARGET_DIR := $(TARGET_DIR_ENV)
else ifneq ($(findstring \\,$(TARGET_DIR_ENV)),)
	TARGET_DIR := $(TARGET_DIR_ENV)
else
	TARGET_DIR := $(TARGET_DIR_ENV)
endif

TARGET_DEBUG := $(TARGET_DIR)/debug/$(LIB_NAME)
TARGET_RELEASE := $(TARGET_DIR)/release/$(LIB_NAME)

ifeq ($(HOST_OS),windows)
VERIFY_DEBUG_ARTIFACT := @echo "Skipping POSIX artifact check on Windows Make host."
VERIFY_RELEASE_ARTIFACT := @echo "Skipping POSIX artifact check on Windows Make host."
else
VERIFY_DEBUG_ARTIFACT := @test -f "$(TARGET_DEBUG)" || (echo "Expected debug library not found at $(TARGET_DEBUG). Check CARGO_TARGET_DIR." && exit 1)
VERIFY_RELEASE_ARTIFACT := @test -f "$(TARGET_RELEASE)" || (echo "Expected release library not found at $(TARGET_RELEASE). Check CARGO_TARGET_DIR." && exit 1)
endif

help:
	@echo "hnsw-toolbox Build System"
	@echo ""
	@echo "Available targets:"
	@echo "  build         - Build Rust library in debug mode"
	@echo "  build-debug   - Build Rust library in debug mode"
	@echo "  build-release - Build Rust library in release mode"
	@echo "  test          - Run Go + Rust tests"
	@echo "  test-go       - Run Go tests"
	@echo "  test-rust     - Run Rust tests"
	@echo "  test-all      - Run Go + Rust tests"
	@echo "  lint          - Run Go and Rust linters"
	@echo "  lint-go       - Run golangci-lint"
	@echo "  lint-rust     - Run cargo clippy with -D warnings"
	@echo "  fmt           - Format Go and Rust code"
	@echo "  fmt-go        - Format Go code"
	@echo "  fmt-rust      - Format Rust code"
	@echo "  clean         - Clean Rust target directory"
	@echo ""
	@echo "Environment variables:"
	@echo "  CARGO_TARGET_DIR - Optional cargo target directory override"

build: build-debug

build-debug:
	cargo build --locked
	$(VERIFY_DEBUG_ARTIFACT)
	@echo "Built debug library at $(TARGET_DEBUG)"

build-release:
	cargo build --locked --release
	$(VERIFY_RELEASE_ARTIFACT)
	@echo "Built release library at $(TARGET_RELEASE)"

test: test-all

test-go:
	go test ./...

test-rust:
	cargo test --locked

test-all: test-go test-rust

lint: lint-go lint-rust

lint-go:
	$(GOLANGCI_LINT_CMD) run --timeout=5m ./...

lint-rust:
	cargo clippy --locked --all-targets -- -D warnings

fmt: fmt-go fmt-rust

fmt-go:
	gofmt -w .
	@if command -v goimports >/dev/null 2>&1; then \
		goimports -w .; \
	else \
		echo "goimports not found; skipping goimports formatting"; \
	fi

fmt-rust:
	cargo fmt

clean:
	cargo clean
