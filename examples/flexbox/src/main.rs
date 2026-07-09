//! # flexbox — responsive layout, one idiom per scenario (SOUL §7.1, §8.1)
//!
//! Where `layout_gallery` teaches *what each container is*, this example teaches
//! **responsiveness**: geometry that re-derives from the viewport instead of being
//! fixed. Not one scenario names a pixel width — each mounts a `column(fill,
//! align = stretch)` inside a 20px window gutter, so the tree fills the responsive
//! work area: headlessly that is `--width`×`--height` minus the gutter, and in
//! `--windowed` mode it follows the **live window size**, re-derived on every resize
//! (grab a window edge and watch the layout adapt — gaps re-share, cards re-wrap,
//! the footer stays pinned). The flexbox contract
//! (SOUL §8.1):
//!
//!   * `fill` — size to 100% of the parent, and at the root to the viewport
//!     itself: the anchor that ties the whole layout to the real window;
//!   * `align = stretch` — children span the container's cross axis, carrying the
//!     viewport width down to every row;
//!   * `flex(grow = n)` — a child (or, childless, a weighted gap) claims `n`
//!     weights of the parent's leftover main-axis space;
//!   * `row(wrap)` — children that no longer fit the line width flow onto
//!     additional lines, and the row's height derives from the line count;
//!   * `row(justify = …)` — leftover main-axis space goes where the keyword says.
//!
//! Every scenario is fully **static** (no signals) and fully spelled in the
//! `view!` grammar — the macro lowers each attribute to the identical
//! `schnellui-widgets` builder chain (SOUL §3.3). Each renders one frame, labels
//! itself with enough on-screen text that the PNG is self-explanatory (SOUL §7.6),
//! and asserts its a11y tree. No event loop headlessly (SOUL §7.1).

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y;
use schnellui::view;
use schnellui::widgets::{Pad, Stack, View};
use schnellui::App;
use schnellui_testing::find_by_role_name;
use strum::IntoEnumIterator;

const WINDOW_PADDING: f32 = 20.0;

/// The outer stack owns the viewport; the padded child becomes the responsive
/// work area, so `fill` scenarios still derive from a definite box.
fn stage(content: impl View) -> impl View {
    Stack::new()
        .fill()
        .child(Pad::all(WINDOW_PADDING).child(content))
}

/// The enumerable scenario table (SOUL §7.1). One row per flexbox idiom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// The app-toolbar idiom: a nav cluster left, actions pinned right, one grown
    /// gap absorbing whatever width the viewport has to spare.
    Toolbar,
    /// Weighted gaps at `grow = 1 : 2 : 3` — proportional shares that re-derive
    /// at every viewport width.
    Weighted,
    /// The same three buttons under four `justify` keywords — where leftover
    /// main-axis space goes.
    Justify,
    /// The card-grid idiom: `row(wrap)` flows fixed-size cards onto as many lines
    /// as the viewport width demands.
    CardFlow,
    /// The vertical twin: a grown gap in a definite-height `column` pins the
    /// footer to the bottom edge.
    PinnedFooter,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output + the PNG path).
    fn name(self) -> &'static str {
        match self {
            Scenario::Toolbar => "toolbar",
            Scenario::Weighted => "weighted",
            Scenario::Justify => "justify",
            Scenario::CardFlow => "card_flow",
            Scenario::PinnedFooter => "pinned_footer",
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 1 — toolbar: one grown gap splits nav from actions
// ---------------------------------------------------------------------------

/// The most common flexbox layout in existence: a toolbar whose left cluster hugs
/// the leading edge and whose actions hug the trailing edge, at **every** width.
/// The childless `flex(grow = 1.0)` between them is a grown gap that absorbs all
/// leftover space; the buttons keep their intrinsic sizes. The `fill` root is the
/// padded viewport and `align = stretch` hands its width to the row — resize the window
/// and only the middle gap grows; no pixel width appears anywhere.
fn toolbar_view() -> impl View {
    view! {
        column(fill, align = stretch, gap = 16.0) {
            text(size = 20.0) { "toolbar: one flex(grow = 1) gap pins actions to the far edge" }
            row(gap = 8.0) {
                button { "Back" }
                button { "Forward" }
                flex(grow = 1.0)
                button { "Search" }
                button { "Menu" }
            }
            text(size = 13.0) { "resize the window: only the middle gap grows" }
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 2 — weighted: grow = 1 : 2 : 3 shares
// ---------------------------------------------------------------------------

/// `grow` is proportional, not absolute: three childless `flex` gaps at weights
/// 1, 2, and 3 split the row's free space in exactly those ratios, whatever the
/// viewport width. Render at `--width 400` and `--width 680` — or just resize the
/// `--windowed` window: the gaps shrink and grow together, but B→C stays twice
/// A→B and C→D three times it.
fn weighted_view() -> impl View {
    view! {
        column(fill, align = stretch, gap = 16.0) {
            text(size = 20.0) { "weighted gaps: flex(grow = 1 / 2 / 3) share free space" }
            row {
                text { "A" }
                flex(grow = 1.0)
                text { "B" }
                flex(grow = 2.0)
                text { "C" }
                flex(grow = 3.0)
                text { "D" }
            }
            text(size = 13.0) { "gap ratios stay 1 : 2 : 3 at every viewport width" }
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 3 — justify: where leftover space goes
// ---------------------------------------------------------------------------

/// `justify` distributes a row's leftover main-axis space without any flex child:
/// the same three intrinsic-size buttons sit at the start, the center, the end, or
/// spread with the space *between* them. Each row is labelled by its keyword so
/// the four distributions read top-to-bottom in one shot — and re-distribute live
/// as the window width changes.
fn justify_view() -> impl View {
    view! {
        column(fill, align = stretch, gap = 12.0) {
            text(size = 20.0) { "justify: the same three buttons, four distributions" }
            text(size = 13.0) { "justify = start" }
            row(gap = 8.0, justify = start) {
                button { "one" } button { "two" } button { "three" }
            }
            text(size = 13.0) { "justify = center" }
            row(gap = 8.0, justify = center) {
                button { "one" } button { "two" } button { "three" }
            }
            text(size = 13.0) { "justify = end" }
            row(gap = 8.0, justify = end) {
                button { "one" } button { "two" } button { "three" }
            }
            text(size = 13.0) { "justify = space_between" }
            row(justify = space_between) {
                button { "one" } button { "two" } button { "three" }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 4 — card_flow: wrap re-flows cards per viewport width
// ---------------------------------------------------------------------------

/// The card-grid idiom: a wrapping row of intrinsic-size cards. The stretched row
/// takes the viewport width as its *line* width; `wrap` flows whatever no longer
/// fits onto the next line, and the row's height derives from the resulting line
/// count — nothing here says "how many columns", the window decides. Render at
/// `--width 360` and `--width 680`, or drag a `--windowed` edge and watch the
/// grid re-flow live.
fn card_flow_view() -> impl View {
    view! {
        column(fill, align = stretch, gap = 16.0) {
            text(size = 20.0) { "card flow: row(wrap) re-flows cards to the viewport" }
            row(wrap, gap = 10.0) {
                button { "card 1" } button { "card 2" } button { "card 3" }
                button { "card 4" } button { "card 5" } button { "card 6" }
                button { "card 7" } button { "card 8" }
            }
            text(size = 13.0) { "the line count is derived, never specified" }
        }
    }
}

// ---------------------------------------------------------------------------
// scenario 5 — pinned_footer: the vertical twin
// ---------------------------------------------------------------------------

/// Flex is axis-agnostic: the same grown-gap idiom inside the viewport-filling
/// `column` pins a footer to the padded bottom edge however tall the window is. The
/// header hugs the top, the footer the bottom, and the `flex(grow = 1.0)` between
/// them absorbs the rest — the page-skeleton layout of every app shell. Here the
/// `fill` root *is* the whole layout: its definite height comes from the window,
/// so dragging the bottom edge keeps the footer glued to it.
fn pinned_footer_view() -> impl View {
    view! {
        column(fill) {
            text(size = 20.0) { "pinned footer: a grown gap in the viewport-filling column" }
            text(size = 13.0) { "header hugs the top" }
            flex(grow = 1.0)
            text(size = 13.0) { "footer hugs the bottom, at every viewport height" }
        }
    }
}

// ---------------------------------------------------------------------------
// harness: build → one frame → (assert) → PNG (SOUL §7.1)
// ---------------------------------------------------------------------------

/// Builds the app for a scenario in its target state (SOUL §7.5). Every scenario
/// is static and **viewport-derived**: no view function takes a size — the `fill`
/// root binds the layout to the padded viewport work area, so `--width`/`--height`
/// only set the *initial* one and a windowed resize re-derives everything.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::Toolbar => {
            App::mount_with_size_scaled(stage(toolbar_view()), width, height, scale)
        }
        Scenario::Weighted => {
            App::mount_with_size_scaled(stage(weighted_view()), width, height, scale)
        }
        Scenario::Justify => {
            App::mount_with_size_scaled(stage(justify_view()), width, height, scale)
        }
        Scenario::CardFlow => {
            App::mount_with_size_scaled(stage(card_flow_view()), width, height, scale)
        }
        Scenario::PinnedFooter => {
            App::mount_with_size_scaled(stage(pinned_footer_view()), width, height, scale)
        }
    }
}

/// Runs the scenario's embedded a11y assertions (SOUL §7.5 oracle): every
/// on-screen widget must be findable by **role + accessible name** — the semantic
/// ground truth that survives cosmetic pixel churn. The childless `flex` gaps are
/// deliberately *absent* here: a layout node draws nothing and carries only the
/// transparent `Group` role (SOUL §8.1), so the tree stays content-only.
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let need = |role: &str, name: &str| -> Result<(), String> {
        find_by_role_name(&tree, role, Some(name))
            .map(|_| ())
            .ok_or_else(|| format!("missing {role} named {name:?}"))
    };
    match scenario {
        Scenario::Toolbar => {
            need("button", "Back")?;
            need("button", "Forward")?;
            need("button", "Search")?;
            need("button", "Menu")?;
        }
        Scenario::Weighted => {
            need("label", "A")?;
            need("label", "B")?;
            need("label", "C")?;
            need("label", "D")?;
        }
        Scenario::Justify => {
            need("label", "justify = start")?;
            need("label", "justify = space_between")?;
            need("button", "one")?;
            need("button", "three")?;
        }
        Scenario::CardFlow => {
            need("button", "card 1")?;
            need("button", "card 5")?;
            need("button", "card 8")?;
        }
        Scenario::PinnedFooter => {
            need("label", "header hugs the top")?;
            need("label", "footer hugs the bottom, at every viewport height")?;
        }
    }
    Ok(())
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "flexbox",
    about = "schnellui responsive flexbox example (SOUL §7.1, §8.1)"
)]
struct Cli {
    /// scenario to render.
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    /// output PNG path.
    #[arg(long)]
    out: Option<String>,
    /// logical viewport width — the input every scenario responds to.
    #[arg(long, default_value_t = 680)]
    width: u32,
    // Tall enough for `justify`'s four labelled rows; `pinned_footer` fills
    // whatever it is given, and the shorter scenarios top-anchor.
    #[arg(long, default_value_t = 420)]
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
    /// opt-in **windowed** (non-headless) mode (SOUL §8): open a real window — and
    /// *resize it*: the flex layouts re-derive live on every `Resized` event.
    #[arg(long)]
    windowed: bool,
}

/// Builds one scenario, runs a single frame, optionally dumps a11y + asserts,
/// writes the PNG (SOUL §7.1).
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
    // Opt-in windowed mode (SOUL §8): resize the window to watch the layout adapt.
    if cli.windowed {
        let app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        return match app.run_windowed("flexbox") {
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
