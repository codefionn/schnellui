//! # layout_gallery — the layout system, one scenario per concept (SOUL §7.1, §8.1)
//!
//! **Layout answers WHERE and HOW BIG, and never draws** (SOUL §8.1). That is the
//! whole lesson. Two layers compose in the same tree but do different jobs:
//!
//!   * **Widgets** (`Text`, `Button`, `Image`, …) answer *what* is on screen: they
//!     emit pixels and always carry an AccessKit role/name.
//!   * **Layout containers** (`Column`, `Row`, `Stack`, `Pad`, `Spacer`) answer
//!     *where* / *how big*: they emit only rects + transforms, draw no pixels, and
//!     carry the transparent `Group` role. A `column` cannot draw; a `button` cannot
//!     position its siblings. That one-way seam is the point (SOUL §8.1).
//!
//! Each scenario below is fully **static** (no signals): it isolates one container
//! concept, labels it with enough on-screen text that a rendered PNG is
//! self-explanatory to a human or a vision agent (SOUL §7.6), renders exactly one
//! frame, and asserts its a11y tree. No event loop (SOUL §7.1).
//!
//! **Both authoring idioms coexist and interoperate** (SOUL §3.3): the `view!` macro
//! for containers its grammar covers (`column`/`row`/`stack`/`scroll`/`pad`/`spacer`/
//! `flex` plus `text`/`button`/…, including `width`/`height`/`size`, `wrap`, and
//! `justify`/`align` keywords), and the raw builder chain the macro lowers to
//! (`Column::new().gap(8.0).child(…)`) for anything the grammar can't spell — a
//! custom `ContainerStyle`, asymmetric `EdgeInsets`. Some scenarios below stay in
//! the builder idiom deliberately, so the two idioms are visibly the same thing at
//! different sugar levels.

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y;
use schnellui::layout::{Align, Container, ContainerStyle};
use schnellui::scene::Size;
use schnellui::view;
use schnellui::widgets::{Column, Image, Pad, Row, Spacer, Stack, Text, TextAlign, View, WrapMode};
use schnellui::App;
use schnellui_testing::find_by_role_name;
use strum::IntoEnumIterator;

const WINDOW_PADDING: f32 = 20.0;

fn stage(content: impl View) -> impl View {
    Pad::all(WINDOW_PADDING).child(content)
}

/// The enumerable scenario table (SOUL §7.1 — `clap::ValueEnum` + `strum::EnumIter`,
/// so `--scenario` is validated and `--list` can enumerate the set). One row per
/// layout concept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// Row nested inside Column, both with gaps — how the two flex axes compose.
    RowsAndColumns,
    /// The same button with and without `Pad::all` — insets made visible.
    Padding,
    /// A label layered over a backdrop panel — Z-overlay.
    Stack,
    /// A Spacer shoving two labels to opposite ends — the space-between idiom.
    Spacer,
    /// One paragraph, four line-break policies — how a wrapping text's height
    /// depends on its column width (SOUL §8.1).
    Wrap,
    /// Weighted `flex(grow = n)` gaps sharing a row's free space 1 : 2 — the
    /// responsive-share primitive (SOUL §8.1).
    FlexGrow,
    /// A `row(wrap)` flowing fixed-size buttons onto new lines at a definite
    /// width — the responsive-flow switch (SOUL §8.1).
    FlexWrap,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output + the golden PNG path).
    fn name(self) -> &'static str {
        match self {
            Scenario::RowsAndColumns => "rows_and_columns",
            Scenario::Padding => "padding",
            Scenario::Stack => "stack",
            Scenario::Spacer => "spacer",
            Scenario::Wrap => "wrap",
            Scenario::FlexGrow => "flex_grow",
            Scenario::FlexWrap => "flex_wrap",
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 1 — rows_and_columns: the two flex axes compose (via `view!`)
// ---------------------------------------------------------------------------

/// A `column` stacks its children **top-to-bottom**; a `row` lays its children
/// **left-to-right**. `gap` inserts fixed space *between* siblings on the main axis
/// (only between them — outer-edge insets are `pad`'s job). A `row` nested in a
/// `column` is how you build a labelled grid of leaves. Fully expressible in the
/// macro grammar, so there is no builder fallback here — pure `view!`.
fn rows_and_columns_view() -> impl View {
    view! {
        column(gap = 16.0) {
            text(size = 20.0) { "Column stacks vertically; Row lays out horizontally" }
            // first Row: three labelled leaves, spaced 12px apart on the main axis.
            row(gap = 12.0) {
                text { "Row A left" }
                text { "Row A middle" }
                text { "Row A right" }
            }
            // second Row: the column's 16px gap sits between this row and the one above.
            row(gap = 12.0) {
                text { "Row B one" }
                text { "Row B two" }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 2 — padding: `Pad::all` insets a child (via `view!`)
// ---------------------------------------------------------------------------

/// `Pad::all(v)` reserves `v` logical px on **every edge** inside its box, pushing
/// its single child inward and growing the box by `2*v` on each axis. Placed beside
/// the *same* button with no `pad`, the inset is unmistakable: the left button sits
/// 24px in from its column's origin, the right one is flush. Uniform padding is
/// `pad(all = v)` in the grammar; asymmetric insets (`Pad::insets(EdgeInsets::…)`)
/// would need the builder.
fn padding_view() -> impl View {
    view! {
        column(gap = 16.0) {
            text(size = 20.0) { "Pad::all(24) insets its child; the right button has none" }
            row(gap = 56.0) {
                // left cell: identical button, wrapped in 24px of padding on all sides.
                column(gap = 6.0) {
                    text { "with Pad::all(24)" }
                    pad(all = 24.0) { button { "Hello" } }
                }
                // right cell: the same button, no padding — the visual baseline.
                column(gap = 6.0) {
                    text { "no padding" }
                    button { "Hello" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 3 — stack: Z-overlay (raw builder chain)
// ---------------------------------------------------------------------------

/// A `stack` places every child in the **same box**, layered back-to-front in child
/// order — the last child paints on top (SOUL §8.1). Its children are out of normal
/// flow, so the stack derives no size from them: we frame it to the backdrop's own
/// 64×64 box with a fixed size. **Builder idiom, kept deliberately:** the macro
/// would lower `stack(width = 64.0, height = 64.0) { … }` to exactly this
/// `Stack::new().style(…)` chain — writing the chain by hand shows the two idioms
/// are the same thing.
///
/// No widget takes an explicit background colour, so the backdrop panel is an
/// `Image` placeholder box (its fixed 64×64 grey square); the short foreground `Text`
/// draws its glyph ink *on top* of that square — sparse glyphs let the panel show
/// through, so both Z-layers are visible at once.
fn stack_view() -> impl View {
    let mut stack_style = ContainerStyle::new(Container::Stack);
    stack_style.fixed_size = Some(Size {
        width: 64.0,
        height: 64.0,
    }); // <- the knob view! can't express

    Column::new()
        .gap(12.0)
        .child(Text::new("Stack layers children in Z; the last child paints on top").size(20.0))
        .child(
            Stack::new()
                .style(stack_style)
                // child 0: painted first ⇒ sits behind. The grey backdrop panel.
                .child(Image::new("panel").alt("backdrop panel"))
                // child 1: painted last ⇒ its ink lands on top of the panel.
                .child(Text::new("Top").size(22.0)),
        )
}

// ---------------------------------------------------------------------------
// scenario 4 — spacer: the space-between idiom (raw builder chain)
// ---------------------------------------------------------------------------

/// A `Spacer` is an empty flex child that **grows to absorb all leftover main-axis
/// space**, shoving its siblings to opposite ends — the "space-between" idiom. It
/// only works if the row has a *definite* main-axis length: a content-sized row has
/// no free space to distribute, so the spacer collapses to zero. **Builder idiom,
/// kept deliberately:** the fixed width via `ContainerStyle` is what the macro's
/// `row(width = …)` lowers to (and `row(justify = space_between)` spells the same
/// idiom without a spacer; `flex_grow` shows the weighted generalization).
fn spacer_view(width: u32, height: u32) -> impl View {
    let row_w = (width as f32 - 40.0).max(160.0);
    let mut row_style = ContainerStyle::new(Container::Row);
    // A definite width is what gives the Spacer room to grow into.
    row_style.fixed_size = Some(Size {
        width: row_w,
        height: (height as f32 * 0.3).clamp(30.0, 60.0),
    });

    Column::new()
        .gap(12.0)
        .child(Text::new("Spacer grows to fill free space, pushing the labels apart").size(20.0))
        .child(
            Row::new()
                .style(row_style)
                .child(Text::new("Left"))
                .child(Spacer::new()) // eats the whole gap between Left and Right
                .child(Text::new("Right")),
        )
}

// ---------------------------------------------------------------------------
// scenario 5 — wrap: one paragraph, four line-break policies (raw builder chain)
// ---------------------------------------------------------------------------

/// The single paragraph every block re-renders (≈40 words). It is deliberately
/// self-describing so the PNG reads on its own (SOUL §7.6).
const WRAP_PARAGRAPH: &str = "Layout answers where and how big while widgets answer \
what is on screen, and wrapped text is the one place the two must negotiate: a width \
comes first, and only then can a height exist — measured through the layout pass, \
then wrapped, then finally painted.";

/// **Wrapped text is width-dependent height** (SOUL §8.1): a `Text` that wraps cannot
/// know how tall it is until layout hands it a width, so its size is *measured through*
/// the layout pass, not fixed at build. This scenario shows one paragraph four ways in
/// the **same narrow column** so the contrast is apples-to-apples:
///
///   * **NoWrap** (the default) stays one line and simply overflows the column edge;
///   * **Word** breaks at spaces to fit the width — visibly *more ink rows*;
///   * **Ellipsis** stays one line but truncates with a trailing `…` *before* the edge;
///   * **Center** wraps like Word, then centers each line in the column width.
///
/// **Builder idiom (same as `stack`/`spacer`, kept deliberately):** a wrapping `Text`
/// fills its parent's width (`set_fill_width` → 100%), so that parent needs a
/// *definite* width to wrap against. The canvas is a `Column` with a fixed size via
/// `ContainerStyle` — what the macro's `column(width = …, height = …, align = stretch)`
/// lowers to; `Align::Stretch` then makes every block (and the paragraph inside it)
/// fill exactly that width. The wrap builders themselves
/// (`Text::wrap`/`align`/`ellipsis`) are ordinary widget methods.
fn wrap_view(width: u32, height: u32) -> impl View {
    // A column deliberately NARROWER than the viewport, so `NoWrap` overflows it while
    // `Word` breaks into several lines inside it. Responsive to `--width`, clamped so
    // the wrap is always visibly narrow.
    let col_w = (width as f32 - 40.0).clamp(240.0, 320.0);

    // One fixed-width, height-filling canvas. The fixed WIDTH is what gives every
    // wrapping paragraph a definite wrap width; `Align::Stretch` makes each child fill
    // it; the definite HEIGHT lets the blocks stack top-anchored (no per-block sizing).
    let mut canvas = ContainerStyle::new(Container::Column);
    canvas.align = Align::Stretch;
    canvas.gap = 14.0;
    canvas.fixed_size = Some(Size {
        width: col_w,
        height: (height as f32 - WINDOW_PADDING * 2.0).max(0.0),
    });

    // Each block = a small header label + the SAME paragraph under one wrap policy.
    // `Align::Stretch` propagates the canvas's definite width down to the paragraph.
    let block = |header: &'static str, body: Text| -> Column {
        let mut style = ContainerStyle::new(Container::Column);
        style.align = Align::Stretch;
        style.gap = 3.0;
        Column::new()
            .style(style)
            .child(Text::new(header).size(13.0))
            .child(body)
    };

    Column::new()
        .style(canvas)
        .child(
            Text::new("Wrap: one paragraph, four line-break policies (narrow column)").size(18.0),
        )
        // NoWrap is the default (WrapMode::NoWrap + Start, no ellipsis): the legacy
        // single-line path. Its natural width exceeds the column, so it overflows.
        .child(block(
            "NoWrap: single line, overflows the column edge",
            Text::new(WRAP_PARAGRAPH),
        ))
        // Word wrap breaks at spaces to fit the column: MORE ink rows than NoWrap.
        .child(block(
            "Word wrap: breaks at spaces to fit the width",
            Text::new(WRAP_PARAGRAPH).wrap(WrapMode::Word),
        ))
        // Ellipsis stays one line but truncates with a trailing ellipsis before the edge.
        .child(block(
            "Ellipsis: single line, truncated before the edge",
            Text::new(WRAP_PARAGRAPH).ellipsis(),
        ))
        // Center wraps like Word, then centers each line within the column width.
        .child(block(
            "Center: wrapped lines centered in the column",
            Text::new(WRAP_PARAGRAPH)
                .wrap(WrapMode::Word)
                .align(TextAlign::Center),
        ))
}

// ---------------------------------------------------------------------------
// scenario 6 — flex_grow: weighted responsive shares (via `view!`)
// ---------------------------------------------------------------------------

/// `flex(grow = n)` is the **responsive-share** primitive (SOUL §8.1): a flex
/// child claims `n` weights of its parent's leftover main-axis space, so the
/// geometry re-derives from the available width instead of being fixed. A
/// *childless* `flex` is a weighted spacer — here two of them split a row's free
/// space 1 : 2, so the `B`→`C` gap is exactly twice the `A`→`B` gap at every
/// viewport width. Fully expressible in the macro grammar now: `width = <px>`
/// gives the row its definite main axis (the knob that used to need a
/// `ContainerStyle` builder fallback), and `flex(grow = …)` the weights.
fn flex_grow_view(width: u32) -> impl View {
    let row_w = (width as f32 - 40.0).max(240.0);
    view! {
        column(gap = 16.0) {
            text(size = 20.0) { "flex(grow = n): weighted gaps share free space 1 : 2" }
            row(width = row_w) {
                text { "A" }
                flex(grow = 1.0)
                text { "B" }
                flex(grow = 2.0)
                text { "C" }
            }
            text(size = 13.0) { "the B-to-C gap is exactly twice the A-to-B gap" }
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 7 — flex_wrap: overflow flows onto new lines (via `view!`)
// ---------------------------------------------------------------------------

/// `row(wrap)` is the **responsive-flow** switch (SOUL §8.1): children that no
/// longer fit the definite line width flow onto additional lines instead of
/// shrinking or overflowing — the card-grid idiom. The row is deliberately
/// narrower than the viewport so six intrinsic-size buttons must break across
/// lines; its *height* is not fixed anywhere — it derives from however many
/// lines the children wrap into (`width = …` fixes only the line width; the
/// cross axis stays content-sized).
fn flex_wrap_view() -> impl View {
    view! {
        column(gap = 16.0) {
            text(size = 20.0) { "row(wrap): buttons flow onto new lines at the fixed width" }
            row(width = 260.0, wrap, gap = 10.0) {
                button { "alpha" }
                button { "beta" }
                button { "gamma" }
                button { "delta" }
                button { "epsilon" }
                button { "zeta" }
            }
            text(size = 13.0) { "the row's height derives from the wrapped line count" }
        }
    }
}

// ---------------------------------------------------------------------------
// harness: build → one frame → (assert) → PNG (SOUL §7.1)
// ---------------------------------------------------------------------------

/// Builds the app for a scenario in its target state (SOUL §7.5). Every scenario is
/// *constructed* directly (all static — no signals to drive). Scenarios that need a
/// fixed container size read the viewport so the geometry scales with `--width`.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::RowsAndColumns => {
            App::mount_with_size_scaled(stage(rows_and_columns_view()), width, height, scale)
        }
        Scenario::Padding => {
            App::mount_with_size_scaled(stage(padding_view()), width, height, scale)
        }
        Scenario::Stack => App::mount_with_size_scaled(stage(stack_view()), width, height, scale),
        Scenario::Spacer => {
            App::mount_with_size_scaled(stage(spacer_view(width, height)), width, height, scale)
        }
        Scenario::Wrap => {
            App::mount_with_size_scaled(stage(wrap_view(width, height)), width, height, scale)
        }
        Scenario::FlexGrow => {
            App::mount_with_size_scaled(stage(flex_grow_view(width)), width, height, scale)
        }
        Scenario::FlexWrap => {
            App::mount_with_size_scaled(stage(flex_wrap_view()), width, height, scale)
        }
    }
}

/// Runs the scenario's embedded a11y assertions (SOUL §7.5 oracle): each on-screen
/// label must be findable by **role + accessible name** — the semantic ground truth
/// that survives cosmetic pixel churn. `Text` leaves carry `Role::Label`; the stack
/// backdrop is an `Image` whose `alt` is its name; buttons carry `Role::Button`.
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let need = |role: &str, name: &str| -> Result<(), String> {
        find_by_role_name(&tree, role, Some(name))
            .map(|_| ())
            .ok_or_else(|| format!("missing {role} named {name:?}"))
    };
    match scenario {
        Scenario::RowsAndColumns => {
            need("label", "Row A left")?;
            need("label", "Row A right")?;
            need("label", "Row B one")?;
        }
        Scenario::Padding => {
            need("label", "with Pad::all(24)")?;
            need("label", "no padding")?;
            need("button", "Hello")?;
        }
        Scenario::Stack => {
            need("label", "Top")?;
            need("image", "backdrop panel")?;
        }
        Scenario::Spacer => {
            need("label", "Left")?;
            need("label", "Right")?;
        }
        Scenario::Wrap => {
            // Each block's header is the self-explanatory on-screen label (SOUL §7.6);
            // the paragraph body is a `Role::Label` carrying the full paragraph text.
            need("label", "NoWrap: single line, overflows the column edge")?;
            need("label", "Word wrap: breaks at spaces to fit the width")?;
            need("label", "Ellipsis: single line, truncated before the edge")?;
            need("label", "Center: wrapped lines centered in the column")?;
            need("label", WRAP_PARAGRAPH)?;
        }
        Scenario::FlexGrow => {
            need("label", "A")?;
            need("label", "B")?;
            need("label", "C")?;
        }
        Scenario::FlexWrap => {
            need("button", "alpha")?;
            need("button", "delta")?;
            need("button", "zeta")?;
        }
    }
    Ok(())
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "layout_gallery",
    about = "schnellui layout gallery (SOUL §7.1, §8.1)"
)]
struct Cli {
    /// scenario to render.
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    /// output PNG path.
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 680)]
    width: u32,
    // Tall enough that the `wrap` scenario's four stacked paragraph blocks fit the
    // PNG; the shorter scenarios simply top-anchor and leave whitespace below.
    #[arg(long, default_value_t = 560)]
    height: u32,
    /// logical→physical scale (SOUL §7.1): shaping + painting run at `size*scale`.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// print scenario names one per line and exit (SOUL §7.1).
    #[arg(long)]
    list: bool,
    /// render every scenario into `--out-dir` (SOUL §7.1).
    #[arg(long)]
    all: bool,
    #[arg(long)]
    out_dir: Option<String>,
    /// emit `[{scenario, path, width, height}]` mapping (SOUL §7.1).
    #[arg(long)]
    manifest: Option<String>,
    /// write the AccessKit tree JSON alongside the PNG (SOUL §7.1).
    #[arg(long)]
    dump_a11y: Option<String>,
    /// run the scenario's embedded a11y assertions, nonzero on failure (SOUL §7.1).
    #[arg(long)]
    assert: bool,
    /// opt-in **windowed** (non-headless) mode (SOUL §8): open a real window with the
    /// chosen scenario instead of writing a PNG.
    #[arg(long)]
    windowed: bool,
}

/// Builds one scenario, runs a single frame, optionally dumps a11y + asserts, writes
/// the PNG (SOUL §7.1).
fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame(); // one synchronous frame: pull → layout → paint → a11y (SOUL §8.1)

    if let Some(path) = &cli.dump_a11y {
        if let Err(e) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    if cli.assert {
        if let Err(e) = run_assertions(scenario, &app) {
            eprintln!("assertion failed ({}): {e}", scenario.name());
            return ExitCode::FAILURE;
        }
    }
    if let Err(e) = app.render_to_png(out) {
        eprintln!("render failed: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.list {
        for s in Scenario::iter() {
            println!("{}", s.name());
        }
        return ExitCode::SUCCESS;
    }

    if cli.all {
        let dir = cli.out_dir.clone().unwrap_or_else(|| ".".to_string());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("could not create out-dir {dir:?}: {e}");
            return ExitCode::FAILURE;
        }
        // The manifest reports the PNG's *physical* dimensions (SOUL §7.1 `--scale`).
        let pw = (cli.width as f32 * cli.scale).round().max(1.0) as u32;
        let ph = (cli.height as f32 * cli.scale).round().max(1.0) as u32;
        let mut manifest = Vec::new();
        for s in Scenario::iter() {
            let out = format!("{dir}/{}.png", s.name());
            let code = render_one(s, &cli, &out);
            if code != ExitCode::SUCCESS {
                return code;
            }
            manifest.push(format!(
                "{{\"scenario\":\"{}\",\"path\":\"{out}\",\"width\":{pw},\"height\":{ph}}}",
                s.name()
            ));
        }
        if let Some(path) = &cli.manifest {
            if let Err(e) = std::fs::write(path, format!("[{}]", manifest.join(","))) {
                eprintln!("manifest write failed: {e}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    let Some(scenario) = cli.scenario else {
        eprintln!("one of --scenario, --list, or --all is required");
        return ExitCode::FAILURE;
    };
    // Opt-in windowed mode (SOUL §8): run the event loop instead of writing a PNG.
    if cli.windowed {
        let app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        return match app.run_windowed("layout_gallery") {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("windowed run failed: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let out = cli
        .out
        .clone()
        .unwrap_or_else(|| format!("{}.png", scenario.name()));
    render_one(scenario, &cli, &out)
}
