//! # scroll_list — a one-shot screenshotter example (SOUL §7.1)
//!
//! Builds the GPU context once, puts a scroll-view scenario in a specific state
//! (constructed at the top, or *driven* to a mid / clamped offset through inbound
//! AccessKit `ScrollDown` actions — SOUL §7.5), renders exactly one synchronous
//! frame, writes a PNG (and optionally an a11y dump), asserts the a11y oracle, and
//! exits 0. **No event loop** (SOUL §7.1).

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::widgets::{Column, Pad, Scroll, Text, View};
use schnellui::App;
use schnellui_testing::{assert_value_contains, find_by_role_name, SnapshotConfig};
use strum::IntoEnumIterator;

const WINDOW_PADDING: f32 = 20.0;

fn stage(content: impl View) -> impl View {
    Pad::all(WINDOW_PADDING).child(content)
}

/// How many labeled rows the scroll content holds (taller than the viewport, so it
/// scrolls). Build-time allocation of the row strings is fine (SOUL §4 — mount).
const ROWS: usize = 25;
/// The scroll viewport box width (logical px).
const VIEWPORT_W: f32 = 320.0;
/// The scroll viewport box height (logical px). Content taller than this scrolls.
const VIEWPORT_H: f32 = 220.0;
/// One inbound `ScrollDown` moves the viewport by this much (mirrors `schnellui`'s
/// internal `SCROLL_STEP`, one wheel notch — SOUL §3.2).
const SCROLL_STEP: f32 = 48.0;
/// `scrolled` drives this many notches (a mid offset well short of the end).
const SCROLLED_NOTCHES: u32 = 3;
/// `bottom` drives far past the end so clamping shows (offset == max_offset).
const BOTTOM_NOTCHES: u32 = 50;

/// The enumerable scenario table (SOUL §7.1 — `clap::ValueEnum` + `strum::EnumIter`
/// so `--scenario` is validated and the set is introspectable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// the list at the top — constructed, offset 0 (SOUL §7.5).
    Top,
    /// the list *driven* to a mid offset by dispatching several `ScrollDown`
    /// `ActionRequest`s at the viewport, located by Role (SOUL §7.5 drive, §6.3).
    Scrolled,
    /// the list *driven* past the end, so the offset clamps to `max_offset` — the
    /// bottom of the content is flush with the viewport (SOUL §3.2 clamping).
    Bottom,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output + golden path).
    fn name(self) -> &'static str {
        match self {
            Scenario::Top => "top",
            Scenario::Scrolled => "scrolled",
            Scenario::Bottom => "bottom",
        }
    }
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "scroll_list",
    about = "schnellui one-shot scroll-view screenshotter (SOUL §7.1)"
)]
struct Cli {
    /// scenario to render.
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    /// output PNG path.
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 400)]
    width: u32,
    #[arg(long, default_value_t = 300)]
    height: u32,
    /// logical→physical scale (SOUL §7.1): shaping + painting run at `size*scale` and
    /// the PNG is `width*scale × height*scale` physical pixels.
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long)]
    theme: Option<String>,
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
    /// opt-in **windowed** (non-headless) mode (SOUL §8): instead of writing a PNG,
    /// mount the chosen `--scenario` and open a real window with a live event loop.
    /// The viewport responds to real mouse-wheel scrolling. Headless PNG output stays
    /// the default. Smoke-test with `SCHNELLUI_AUTOCLOSE_MS=<n>` to auto-exit.
    #[arg(long)]
    windowed: bool,
}

/// The scroll-list UI: a fixed viewport wrapping a column of `ROWS` labeled rows
/// (SOUL §3.3 builder chain). The content is taller than the viewport, so it scrolls.
fn scroll_list_view() -> impl View {
    let mut col = Column::new().gap(2.0);
    for i in 0..ROWS {
        col = col.child(Text::new(format!("Row {i}")));
    }
    Scroll::new().size(VIEWPORT_W, VIEWPORT_H).child(col)
}

/// Builds the app for a scenario in its target state (SOUL §7.5). `Top` is
/// *constructed*; `Scrolled`/`Bottom` are *driven* there through the real inbound
/// `ActionRequest` path (§6.3) — proving the offset is reachable, not merely set.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::Top => {
            App::mount_with_size_scaled(stage(scroll_list_view()), width, height, scale)
        }
        Scenario::Scrolled => {
            let mut app =
                App::mount_with_size_scaled(stage(scroll_list_view()), width, height, scale);
            // Lay out first so the viewport + content have rects to scroll against,
            // then drive to a mid offset through the inbound action path (§6.3).
            app.frame();
            drive_scrolls(&mut app, SCROLLED_NOTCHES);
            app
        }
        Scenario::Bottom => {
            let mut app =
                App::mount_with_size_scaled(stage(scroll_list_view()), width, height, scale);
            app.frame();
            drive_scrolls(&mut app, BOTTOM_NOTCHES);
            app
        }
    }
}

/// Drives `times` `ScrollDown` `ActionRequest`s at the scroll viewport, located by
/// `Role::ScrollView` (SOUL §7.5 — semantic query, never pixels). Each dispatch routes
/// through the same handler a mouse wheel would fire (§6.3), so the offset advances
/// (and clamps at the end) exactly as under real input.
fn drive_scrolls(app: &mut App, times: u32) {
    let Some(id) = app.find_widget(Role::ScrollView, None) else {
        eprintln!("drive: no scroll view");
        return;
    };
    let target = to_access_id(id);
    for _ in 0..times {
        let req = ActionRequest {
            action: Action::ScrollDown,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: None,
        };
        app.dispatch_action(&req);
    }
}

/// Runs the scenario's embedded a11y assertions (SOUL §7.5 oracle) — the accessible
/// value of the scroll viewport *is* its rounded vertical offset, so it is the ground
/// truth for "where did the list scroll to".
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    // The viewport must exist as a ScrollView in every scenario (SOUL §6.1).
    let sv = app
        .find_widget(Role::ScrollView, None)
        .ok_or_else(|| "missing scroll view".to_string())?;
    find_by_role_name(&tree, "scroll_view", None)
        .ok_or_else(|| "scroll view not in a11y dump".to_string())?;
    let offset = app.scene().scroll_offset(sv).y;
    match scenario {
        Scenario::Top => {
            // constructed at the top: offset 0.
            assert_value_contains(&tree, "scroll_view", None, "0")?;
            Ok(())
        }
        Scenario::Scrolled => {
            // driven three notches, well short of the end → exact offset.
            let expected = (SCROLLED_NOTCHES as f32 * SCROLL_STEP).round() as i64;
            assert_value_contains(&tree, "scroll_view", None, &expected.to_string())?;
            Ok(())
        }
        Scenario::Bottom => {
            // driven far past the end: the offset clamped to max_offset — strictly
            // positive, and strictly *below* the total delta requested (proof it
            // clamped rather than tracking every notch). The announced value equals
            // the rounded offset (SOUL §3.2 clamping).
            let requested = BOTTOM_NOTCHES as f32 * SCROLL_STEP;
            if offset <= 0.0 {
                return Err(format!("bottom offset not positive: {offset}"));
            }
            if offset >= requested {
                return Err(format!(
                    "bottom offset {offset} did not clamp below the requested {requested}"
                ));
            }
            let expected = (offset.round() as i64).to_string();
            assert_value_contains(&tree, "scroll_view", None, &expected)?;
            Ok(())
        }
    }
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame(); // one synchronous frame (SOUL §7.1)

    if let Some(path) = &cli.dump_a11y {
        if let Err(e) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    if cli.assert {
        if let Err(e) = run_assertions(scenario, &app) {
            eprintln!("assertion failed: {e}");
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
            manifest.push(serde_manifest_entry(s.name(), &out, pw, ph));
        }
        if let Some(path) = &cli.manifest {
            let json = format!("[{}]", manifest.join(","));
            if let Err(e) = std::fs::write(path, json) {
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
    // Opt-in windowed mode (SOUL §8): mount the chosen scenario and run the event loop
    // instead of writing a PNG. The viewport responds to real mouse-wheel scrolling.
    if cli.windowed {
        let app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        return match app.run_windowed("scroll_list") {
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

    // config is read so the bless env-var contract is honored by the harness (§7.4)
    let _cfg = SnapshotConfig::from_env("snapshots");
    render_one(scenario, &cli, &out)
}

/// One `--manifest` entry (SOUL §7.1). Kept as a hand-rolled object to avoid a serde
/// derive on a throwaway type.
fn serde_manifest_entry(name: &str, path: &str, width: u32, height: u32) -> String {
    format!("{{\"scenario\":\"{name}\",\"path\":\"{path}\",\"width\":{width},\"height\":{height}}}")
}
