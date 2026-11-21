#!/usr/bin/env bash
# Builds examples/android_native for the Android ABIs and copies the .so files into
# the demo app. Needs cargo-ndk, the Android NDK and the rust android targets.
# Extra arguments go to cargo build, for example --no-default-features for a
# library without SVG support.
set -euo pipefail
cd "$(dirname "$0")"

root="$(cd .. && pwd)"
out="$(pwd)/demo/src/main/jniLibs"

if ! command -v cargo-ndk >/dev/null 2>&1; then
    echo "cargo-ndk not found. Install it with: cargo install cargo-ndk" >&2
    exit 1
fi

rustup target add aarch64-linux-android x86_64-linux-android

rm -rf "$out"
mkdir -p "$out"
(
    cd "$root"
    cargo ndk -t arm64-v8a -t x86_64 -o "$out" build --release -p android_native "$@"
)

ls -l "$out"/*/libandroid_native.so
