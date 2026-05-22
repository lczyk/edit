# cspell:ignore gsub mdbook rustup
.SUFFIXES:

help:
	@echo "Available targets:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

.PHONY: sync-version
sync-version:  ## Sync crate Cargo.toml versions from VERSION (source of truth)
	@v=$$(awk '/^[[:space:]]*#/ {next} /^[[:space:]]*$$/ {next} {gsub(/[[:space:]]/,""); print; exit}' VERSION); \
	if [ -z "$$v" ]; then echo "VERSION has no version line" >&2; exit 1; fi; \
	for crate in edit eat; do \
	awk -v v="$$v" ' \
	  /^version = ".*"[[:space:]]*#[[:space:]]*source:[[:space:]]*\/VERSION/ { \
	    print "version = \"" v "\"  # source: /VERSION (synced by `make sync-version`; do not edit by hand)"; next \
	  } \
	  { print } \
	' crates/$$crate/Cargo.toml > crates/$$crate/Cargo.toml.tmp && mv crates/$$crate/Cargo.toml.tmp crates/$$crate/Cargo.toml; done

.PHONY: build
build: sync-version  ## Release build (stable toolchain, larger binary)
	cargo build --release
	@if command -v upx >/dev/null 2>&1; then \
		upx target/release/edit || echo "upx failed, skipping compression"; \
		upx target/release/eat || echo "upx failed, skipping compression"; \
	fi

.PHONY: du
du: build  ## Show release binary size
	du -h target/release/edit target/release/eat

.PHONY: install
install: sync-version  ## Install the edit binary (debug build) into ~/.cargo/bin
	cargo install --debug --path crates/edit --force
	ln -sf edit "$${CARGO_INSTALL_ROOT:-$$HOME/.cargo}/bin/eat"

.PHONY: check
check:  ## Fast type-check across all targets and features
	cargo check --all-targets --all-features

.PHONY: clippy
clippy:  ## Clippy with warnings denied (CI bar)
	cargo clippy --all-targets --all-features -- --deny warnings

.PHONY: test
test:  ## Run the test suite with all features enabled
	cargo test --all-features

.PHONY: format
format:  ## Format the workspace with rustfmt
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

.PHONY: docs-build
docs-build:  ## Build the mdBook knowledge base into doc/book/
	@if ! command -v mdbook >/dev/null 2>&1; then \
		echo "error: mdbook not installed."; \
		echo "  install: cargo install mdbook"; \
		exit 1; \
	fi
	mdbook build doc

.PHONY: docs-serve
docs-serve:  ## Serve the mdBook knowledge base with live reload
	@if ! command -v mdbook >/dev/null 2>&1; then \
		echo "error: mdbook not installed."; \
		echo "  install: cargo install mdbook"; \
		exit 1; \
	fi
	mdbook serve doc --open

.PHONY: docs-clean
docs-clean:  ## Remove the built mdBook output
	rm -rf doc/book

.PHONY: verify
verify: fmt-check clippy test spellcheck  ## Run the full pre-commit gate (fmt, clippy, test, spellcheck)
	@echo "All checks passed."
