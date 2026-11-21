#!/usr/bin/env bash
# Builds the android_native cdylib for x86_64-linux-android and fails if the
# library has an undefined android_main. Bionic resolves every symbol at load,
# so such a library cannot be loaded by the Java host.
# Skips when the NDK or the Rust target is missing.
set -euo pipefail
cd "$(dirname "$0")/.."

ndk_root="${ANDROID_NDK_HOME:-}"
if [ -z "$ndk_root" ] && [ -d "$HOME/Android/Sdk/ndk" ]; then
    ndk_root="$(ls -d "$HOME"/Android/Sdk/ndk/*/ 2>/dev/null | sort -V | tail -n 1)"
fi
if [ -z "$ndk_root" ] || ! rustup target list --installed | grep -qx x86_64-linux-android; then
    echo "check-android-symbols: no NDK or x86_64-linux-android target, skipping"
    exit 0
fi
bin="${ndk_root%/}/toolchains/llvm/prebuilt/linux-x86_64/bin"

CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$bin/x86_64-linux-android24-clang" \
    cargo build --locked -p android_native --target x86_64-linux-android

lib=target/x86_64-linux-android/debug/libandroid_native.so
if "$bin/llvm-nm" -D --undefined-only "$lib" | grep -qw android_main; then
    echo "check-android-symbols: $lib has an undefined android_main" >&2
    exit 1
fi
for f in Mount Render Dispatch Resize Unmount; do
    if ! "$bin/llvm-nm" -D --defined-only "$lib" | grep -q "Java_io_github_codefionn_schnellui_SchnellBridge_native$f$"; then
        echo "check-android-symbols: $lib does not export native$f" >&2
        exit 1
    fi
done
echo "check-android-symbols: ok"
