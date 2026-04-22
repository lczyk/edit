.SUFFIXES:

help:
	@echo "Available targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.PHONY: build
build:  ## Release build (stable toolchain, larger binary)
	cargo build --release

.PHONY: build-nightly
build-nightly:  ## Release build (nightly toolchain, smaller binary via build-std)
	cargo build --release --config .cargo/release.toml

.PHONY: check
check:  ## Fast type-check across all targets and features
	cargo check --all-targets --all-features

.PHONY: clippy
clippy:  ## Clippy with warnings denied (CI bar)
	cargo clippy --all-targets --all-features -- --deny warnings

.PHONY: test
test:  ## Run the test suite with all features enabled
	cargo test --all-features

.PHONY: fmt
fmt:  ## Format the workspace with rustfmt
	cargo fmt --all

.PHONY: fmt-check
fmt-check:  ## Verify formatting without modifying files
	cargo fmt --all -- --check

.PHONY: clean
clean:  ## Remove the target/ directory
	cargo clean

.PHONY: verify
verify: fmt-check clippy test  ## Run the full pre-commit gate (fmt, clippy, test)
	@echo "All checks passed."
