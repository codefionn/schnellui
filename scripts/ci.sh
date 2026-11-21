#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

export SCHNELLUI_DEBUG_SERVER=off
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_INCREMENTAL=0

# On Windows, Vulkan on an Intel Arc B580 (driver 101.9150) intermittently leaves
# stale texels in the tile cache's picture, failing
# tile_cache_matches_a_full_repaint_on_random_edits about one run in eight. DX12 on
# the same GPU and WARP do not, so the gate pins DX12 unless the caller chose.
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) export WGPU_BACKEND="${WGPU_BACKEND:-dx12}" ;;
esac

# Native core tests run serially because the signal graph is process-global.
cargo fmt --all -- --check
cargo test --locked \
    -p schnellui-view-parser -p schnellui-macro -p schnellui \
    -p schnellui-icons-md -p schnellui-bench -- --test-threads=1
cargo test --locked -p schnellui --features render-html,ssr --lib render_html -- --test-threads=1
cargo test --locked -p schnellui --features render-android --lib render_android -- --test-threads=1
cargo test --locked -p android_native -- --test-threads=1
scripts/check-android-symbols.sh

# Run live apps with unusable display addresses, drive them through private debug
# sockets, and verify offscreen pixels, remounts, session isolation, and cleanup.
cargo build --locked -p counter -p todo -p schnellui-debug
# Windows' `python3` is often the Microsoft Store stub, which exits non-zero.
PYTHON=python3
if ! "$PYTHON" --version >/dev/null 2>&1; then
    PYTHON=python
fi
"$PYTHON" scripts/test-headless-windowed.py

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
