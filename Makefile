.SUFFIXES:

help:
	@echo "Available targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.PHONY: build
build:  ## Release build (stable toolchain, larger binary)
	cargo build --release
	@if command -v upx >/dev/null 2>&1; then \
		upx target/release/edit || echo "upx failed, skipping compression"; \
	fi

.PHONY: build-nightly
build-nightly:  ## Release build (nightly toolchain, smaller binary via build-std)
	cargo build --release --config .cargo/release.toml
	@if command -v upx >/dev/null 2>&1; then \
		upx target/release/edit || echo "upx failed, skipping compression"; \
	fi

.PHONY: du
du: build  ## Show release binary size
	du -h target/release/edit

.PHONY: install
install:  ## Install the edit binary into ~/.cargo/bin
	cargo install --path crates/edit --force

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

.PHONY: cover
cover:  ## Coverage profile + HTML file (cover.out, cover.html)
	@if ! cargo llvm-cov --version >/dev/null 2>&1; then \
		echo "error: cargo-llvm-cov not installed."; \
		echo "  install: cargo install cargo-llvm-cov && rustup component add llvm-tools-preview"; \
		exit 1; \
	fi
	cargo llvm-cov --all-features --lcov --output-path cover.out
	cargo llvm-cov report --html --output-dir target/llvm-cov
	cargo llvm-cov report

.PHONY: cover-open
cover-open: cover  ## Run coverage and open the HTML report in a browser
	cargo llvm-cov --all-features --html --open

.PHONY: spellcheck
spellcheck:  ## Spellcheck sources and docs with cspell (via npx)
	npx --yes cspell --no-progress --gitignore "**/*.rs" "**/*.md" "Makefile"

.PHONY: clean
clean:  ## Remove the target/ directory
	cargo clean

.PHONY: verify
verify: fmt-check clippy test  ## Run the full pre-commit gate (fmt, clippy, test)
	@echo "All checks passed."
