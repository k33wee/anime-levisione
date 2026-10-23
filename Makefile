# Local mirror of CI: run `make ci` before pushing to main.
# `make act` replays the real GitHub workflow with act (needs Docker).
SHELL := /bin/bash
.PHONY: help fmt fmt-check lint test build commitlint ci act hooks

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-14s\033[0m %s\n", $$1, $$2}'

fmt: ## Format all Rust sources (cargo fmt)
	cargo fmt --all

fmt-check: ## Verify formatting like CI does
	cargo fmt --all -- --check

lint: ## Clippy with warnings as errors like CI does
	cargo clippy --all-targets -- -D warnings

test: ## Run the full test suite
	cargo test --all-targets

build: ## Release build like CI does
	cargo build --release

commitlint: ## Lint the last commit message (FROM=main to lint a branch range)
	@if [ -n "$$FROM" ]; then \
		bunx commitlint --from "$$FROM" --to HEAD; \
	else \
		bunx commitlint --last; \
	fi

ci: fmt-check lint test commitlint ## Everything CI checks (fast local gate)

act: ## Replay the GitHub workflow locally via act (ubuntu job only)
	act push -j commitlint -P ubuntu-latest=catthehacker/ubuntu:act-22.04
	act push -j test --matrix os:ubuntu-latest -P ubuntu-latest=catthehacker/ubuntu:act-22.04

hooks: ## Install the git hooks from .githooks (commitlint on commit)
	git config core.hooksPath .githooks
	@echo "git hooks installed (.githooks)"
