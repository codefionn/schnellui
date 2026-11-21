# schnellui Documentation

Welcome to the `schnellui` documentation. Guides are organized from first steps to deep architectural references.

## Getting Started & Core Guides

| Document | What it covers |
| --- | --- |
| [Getting Started](getting-started.md) | Installation, Cargo features, the `mount` → `frame` → `render` lifecycle, windowed vs headless modes, and environment variables. |
| [Reactivity & State](reactivity.md) | `Signal`, `Memo`, `Effect`, `Scope`, `Store`/`Selector`, dynamic text and reactive attributes, subtree replacement, remounts, and zero-allocation rules. |
| [Widgets, Layout & Theming](widgets-and-layout.md) | Two-layer layout/widget design, complete `view!` syntax reference, widget catalog, `Theme`/`Shape` system, localization, motion, and platform APIs. |
| [Renderers & Targets](renderers.md) | Native `wgpu` GPU, SIMD CPU fallback (`render_soft`), semantic HTML/CSS/WASM/SSR (`render_html`), native Android views (`render_android`), and Servo embedding. |
| [Accessibility, Testing & Agents](accessibility-and-testing.md) | AccessKit integration, semantic driving and assertions, golden PNG diffing, deterministic simulation, fuzzing, allocation benchmarks, and AI-agent loops. |

## Architecture & Specifications

| Document | What it covers |
| --- | --- |
| [Architecture](architecture.md) | Source-level explanation of every subsystem as implemented today, plus a list of known gaps against `SOUL.md`. |
| [SOUL.md](../SOUL.md) | The project's design manifesto, prime directives, and long-term architectural north star. |
| [Android Renderer Spec](android-renderer.md) | Binary op-stream encoding, view kinds, properties, events, diffing rules, and JNI signatures for `render_android`. |
| [Android Host README](../android/README.md) | Java host library layout, Gradle build/test commands, and APK installation steps. |
| [Binary Size](binary-size.md) | Cargo dev and release profiles that dramatically shrink binary and `target/` sizes while preserving panic backtraces. |

## Debugging & Diagnostics

| Document | What it covers |
| --- | --- |
| [Live Debug Instrumentation](debugging/DEBUG_INSTRUMENTATION.md) | Local debug server, per-run bearer authentication, `schnellui-debug` CLI commands, jq selectors, waits, tree captures, and scripts. |
| [Interaction & Remount Tracing](debugging/INTERACTION_DEBUGGING.md) | JSONL tracing of pointer routing, focus, cursor changes, keyboard/a11y events, interrupted drags, and reasoned remounts. |

## Examples

- [Examples Catalog](../examples/README.md) — 23 progressive examples covering every feature, plus the common CLI contract and `--headless-windowed` workflow.
