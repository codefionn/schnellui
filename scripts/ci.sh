#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

export SCHNELLUI_DEBUG_SERVER=off
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_INCREMENTAL=0

# Native core tests run serially because the signal graph is process-global.
cargo fmt --all -- --check
cargo test --locked \
    -p schnellui-view-parser -p schnellui-macro -p schnellui \
    -p schnellui-icons-md -p schnellui-bench -- --test-threads=1

# Run live apps with unusable display addresses, drive them through private debug
# sockets, and verify offscreen pixels, remounts, session isolation, and cleanup.
cargo build --locked -p counter -p todo -p schnellui-debug
python3 scripts/test-headless-windowed.py

cargo test --locked -p schnellui --features count-allocations \
    --test alloc_budget --test view_reactive_attrs --test text_area_edit_allocations \
    -- --test-threads=1
cargo build --locked --release -p schnellui-bench

# Allocation counts and bytes are deterministic. Wall-clock ratios remain useful
# locally, but shared runners must not fail because another process consumed CPU.
if [[ -n "${SCHNELLUI_CI_REPORT:-}" ]]; then
    cargo run --locked --release -p schnellui-bench -- --allocations-only --iters 100 --json \
        > "$SCHNELLUI_CI_REPORT"
else
    cargo run --locked --release -p schnellui-bench -- --allocations-only --iters 100
fi
