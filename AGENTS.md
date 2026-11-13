# AGENTS.md

schnellui is a Rust GUI framework: `view!` macros build a retained scene driven by signals, laid out by taffy and drawn by wgpu, with an HTML renderer and AccessKit accessibility. [SOUL.md](SOUL.md) settles design disputes. Its first rule, zero heap allocations on a steady-state re-render, is a CI gate.

The framework lives in `crates/schnellui`. The macro is in `crates/schnellui-macro` and `crates/schnellui-view-parser`. Examples are listed in [examples/README.md](examples/README.md).

## Checks

- `scripts/ci.sh` is the gate. Run tests with `--test-threads=1` because the signal graph is process-global.
- `servo_demo` builds all of Servo. Add `--exclude servo_demo` to workspace commands unless the change touches Servo.

## Dependency versions

A requirement is a floor: the lowest version the code builds and passes tests with, so downstream crates can resolve a wide range.

- When a newer major also compiles unchanged, accept both majors: `ashpd = ">=0.13.6, <0.15"`.
- After changing a dependency or starting to use a newer API, test the floors: run `cargo +nightly update -Z direct-minimal-versions`, then `cargo check --workspace --all-targets --exclude servo_demo` and the tests. A floor that fails moves up to the first version that passes. `cargo update` restores the latest lockfile, which is the one to commit.
- Platform crates (`windows-sys`, `objc2*`) compile only on their targets: check with `--target x86_64-pc-windows-gnu` and `--target aarch64-apple-darwin`.
