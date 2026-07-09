# schnellui

schnellui is rust native, gpu accelerated ui framework.

It natively supports accessibility, native rendering via gpu, html output in
browser and has great instrumentation for debugging and automatically
controlling the ui.

## Documentation

- [docs/architecture.md](docs/architecture.md) describes how the code works today and
  lists where it falls short of [SOUL.md](SOUL.md).
- [examples/README.md](examples/README.md) lists the examples and their CLI.
- [docs/debugging](docs/debugging/DEBUG_INSTRUMENTATION.md) covers the instrumentation
  server.
- [Live offscreen examples](examples/README.md#live-apps-without-a-desktop-window)
  shows how agents can start, drive, and screenshot apps with `--headless-windowed`
  without opening a desktop window.
- CI and its local entry point: `scripts/ci.sh`. The workflow tests the core crates
  and enforces allocation budgets and allocation scaling. It reports wall-clock
  timings without making them a shared-runner gate.
- Allocation and scaling gates: `cargo test -p schnellui --features count-allocations
  --test alloc_budget` and `cargo run --release -p schnellui-bench`.
- [docs/binary-size.md](docs/binary-size.md) explains why debug info dominates binary
  size and gives the cargo profiles that shrink debug and release builds.

## AI Disclosure

This project is being developed with AI assistance.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in schnellui by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
