#!/bin/sh
# Run all checks required before a commit.
set -e
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
