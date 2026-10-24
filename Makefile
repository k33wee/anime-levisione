# Local mirror of CI: run `make ci` before pushing to main.
# `make act` replays the real GitHub workflow with act (needs Docker).
SHELL := /bin/bash
.PHONY: help fmt fmt-check lint test build commitlint ci act hooks bindings

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

fmt: ## Format all Rust sources (cargo fmt)
	cargo fmt --all

fmt-check: ## Verify formatting like CI does
	cargo fmt --all -- --check

lint: ## Clippy with warnings as errors like CI does
	cargo clippy --all-targets --all-features --locked -- -D warnings

test: ## Run the full test suite
	cargo test --all-targets --all-features --locked

build: ## Release build like CI does
	cargo build --release

commitlint: ## Lint the last commit message (FROM=main to lint a branch range)
	@if [ -n "$$FROM" ]; then \
		bunx commitlint --from "$$FROM" --to HEAD; \
	else \
		bunx commitlint --last; \
	fi

ci: fmt-check lint test commitlint ## Everything CI checks (fast local gate)

bindings: ## Regenerate the Kotlin bindings committed under android/
	cargo build -p anime-levisione-core --features uniffi
	rm -rf target/bindings && mkdir -p target/bindings
	lib=$$(ls -1 target/debug/libanime_levisione_core.so target/debug/libanime_levisione_core.dylib target/debug/libanime_levisione_core.dll 2>/dev/null | head -1); \
	test -n "$$lib" || { echo 'core library not found; build failed?' >&2; exit 1; }; \
	cargo run -q -p uniffi-bindgen -- generate --library "$$lib" --crate anime_levisione_core -l kotlin --no-format -o target/bindings
	rm -rf android/app/src/main/java/uniffi
	mkdir -p android/app/src/main/java/uniffi
	cp -R target/bindings/uniffi/* android/app/src/main/java/uniffi/
	@echo "bindings refreshed: android/app/src/main/java/uniffi/"

act: ## Replay the GitHub workflow locally via act (ubuntu job only)
	act push -j commitlint -P ubuntu-latest=catthehacker/ubuntu:act-22.04
	act push -j test --matrix os:ubuntu-latest -P ubuntu-latest=catthehacker/ubuntu:act-22.04

hooks: ## Install the git hooks from .githooks (commitlint on commit)
	git config core.hooksPath .githooks
	@echo "git hooks installed (.githooks)"
