//! # hello — the smallest complete schnellui program
//!
//! This is lesson 1. It builds a *static* UI with the [`view!`] macro (a title and
//! a subtitle — no signals, no interaction), mounts it, renders exactly one frame to
//! a PNG, and exits. There is no window and no event loop (SOUL §7.1).
//!
//! Every schnellui program follows the same three-step lifecycle:
//!
//! 1. **mount** — `App::mount*` runs your UI's setup **once**, building a retained
//!    tree of widgets. In larger programs reactivity lives in the leaf closures the
//!    macro wraps; the build itself is *not* a per-frame render (SOUL §3.3).
//! 2. **frame** — `App::frame()` runs one synchronous pass (pull → layout → paint →
//!    a11y) over whatever is dirty. A static UI reaches its final pixels in one frame.
//! 3. **output** — `App::render_to_png` reads the painted scene back off the GPU and
//!    encodes a deterministic PNG (SOUL §7.2); `--dump-a11y` writes the semantic tree.
//!
//! Run it: `hello --out hello.png`.

use std::process::ExitCode;

use clap::Parser;
// `view!` is re-exported from the umbrella crate, so a program never needs to depend
// on the proc-macro crate directly. `View` is the trait every widget (and every
// `view!` block) implements.
use schnellui::view;
use schnellui::widgets::View;
use schnellui::App;

/// Lesson 1 keeps the CLI intentionally tiny: an output path, a viewport, a scale,
/// and an optional accessibility dump. There is exactly one UI state here, so there
/// is no `--scenario` table — see the `counter` example for that.
#[derive(Parser)]
#[command(name = "hello", about = "the smallest complete schnellui program")]
struct Cli {
    /// where to write the PNG.
    #[arg(long, default_value = "hello.png")]
    out: String,
    /// logical viewport width in pixels.
    #[arg(long, default_value_t = 400)]
    width: u32,
    /// logical viewport height in pixels.
    #[arg(long, default_value_t = 160)]
    height: u32,
    /// logical→physical scale: the UI is shaped and painted at `size * scale`, and
    /// the PNG comes out `width*scale × height*scale` physical pixels (SOUL §7.1).
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// also write the AccessKit tree as JSON here — *what* the UI is (roles, names),
    /// not just how it looks (SOUL §6.5). Optional; omit it and you just get the PNG.
    #[arg(long)]
    dump_a11y: Option<String>,
    /// opt-in **windowed** (non-headless) mode (SOUL §8): open a real window with a
    /// live event loop instead of writing a PNG. This static UI has no interaction —
    /// close the window (or press Esc) to exit. Headless PNG output stays the default;
    /// `SCHNELLUI_AUTOCLOSE_MS=<n>` auto-exits for smoke tests.
    #[arg(long)]
    windowed: bool,
}

/// Our entire UI: a `column` stacking two static `text` leaves. Because nothing
/// reads a signal, the whole tree is the macro's "static skeleton" — built once at
/// mount and never re-visited on later frames (SOUL §3.3). The `size = …` attribute
/// on each `text` lowers to a `.size(…)` builder call setting the font size.
fn hello_view() -> impl View {
    view! {
        pad(all = 20.0) {
            column {
                text(size = 32.0) { "Hello, schnellui" }
                text(size = 16.0) { "the smallest complete program" }
            }
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // MOUNT (runs once): build the retained tree at the requested viewport and scale.
    // The scale is applied *before* the build so glyphs rasterize at their real
    // physical size and the text measures come out glyph-exact (SOUL §7.1).
    let mut app = App::mount_with_size_scaled(hello_view(), cli.width, cli.height, cli.scale);

    // Opt-in windowed mode (SOUL §8): open a real window instead of writing a PNG.
    // This static UI just displays; close the window or press Esc to exit.
    if cli.windowed {
        return match app.run_windowed("hello") {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("windowed run failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // ONE SYNCHRONOUS FRAME: pull → layout → paint → a11y, walked over the dirty set.
    // A static UI settles in exactly one frame, so there is no loop to spin here.
    app.frame();

    // Optional: dump the semantic tree so an agent (or a snapshot test) can read the
    // UI's roles and names, not only its pixels (SOUL §6.5).
    if let Some(path) = &cli.dump_a11y {
        if let Err(e) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {e}");
            return ExitCode::FAILURE;
        }
    }

    // DETERMINISTIC OUTPUT: read the painted scene back off the GPU and encode a PNG.
    // Given the same inputs it produces the same bytes every run (SOUL §7.3) — which
    // is exactly what makes the screenshots diffable.
    match app.render_to_png(&cli.out) {
        Ok(()) => {
            println!("wrote {}", cli.out);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("render failed: {e}");
            ExitCode::FAILURE
        }
    }
}
