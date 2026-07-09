//! # charts — a one-shot screenshotter example (SOUL §7.1)
//!
//! Builds the GPU context once, puts a scenario's chart UI in a specific state,
//! renders exactly one synchronous frame, writes a PNG (and optionally an a11y
//! dump), asserts the a11y oracle (each chart's deterministic summary — SOUL §6.1,
//! §7.5), and exits 0. **No event loop** (SOUL §7.1).

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, A11yNodeDump, A11yTreeDump};
use schnellui::charts::{BarChart, LineChart, Sparkline, SERIES};
use schnellui::widgets::{Column, Pad, Stack, Text, View};
use schnellui::App;
use schnellui_testing::SnapshotConfig;
use strum::IntoEnumIterator;

/// Keeps instructional content off the physical window edge.
const WINDOW_PADDING: f32 = 20.0;

fn stage(content: impl View) -> impl View {
    Stack::new()
        .fill()
        .child(Pad::all(WINDOW_PADDING).child(content))
}

/// The enumerable scenario table (SOUL §7.1 — `clap::ValueEnum` + `strum::EnumIter`
/// so `--scenario` is validated and the set is introspectable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// a basic bar chart over `[3,7,2,9,4]` (SOUL §8.1).
    BarBasic,
    /// a line chart over a dozen points, with per-point markers.
    LineBasic,
    /// a minimal sparkline (segments only).
    Sparkline,
    /// a bar chart with mixed-sign data (negatives hang below the baseline).
    Negatives,
    /// a dashboard: `Text` titles composed above a `BarChart`, `LineChart` and
    /// `Sparkline`, colored by `SERIES[0..3]` in fixed order (SOUL §6.1 data-viz).
    Dashboard,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output + golden path).
    fn name(self) -> &'static str {
        match self {
            Scenario::BarBasic => "bar_basic",
            Scenario::LineBasic => "line_basic",
            Scenario::Sparkline => "sparkline",
            Scenario::Negatives => "negatives",
            Scenario::Dashboard => "dashboard",
        }
    }
}

// --- the scenario UIs (constructed directly — SOUL §7.5) ---------------------

/// A dozen points for the line/trend charts.
const TREND: [f32; 12] = [
    4.0, 6.0, 5.0, 8.0, 7.0, 9.0, 6.0, 10.0, 8.0, 11.0, 9.0, 12.0,
];

fn bar_basic_view() -> impl View {
    Column::new()
        .child(Text::new("Bar chart").size(18.0))
        .child(BarChart::new([3.0, 7.0, 2.0, 9.0, 4.0]).title("Bar chart"))
        .gap(8.0)
}

fn line_basic_view() -> impl View {
    Column::new()
        .child(Text::new("Line chart").size(18.0))
        .child(LineChart::new(TREND).title("Line chart").markers(true))
        .gap(8.0)
}

fn sparkline_view() -> impl View {
    Column::new()
        .child(Text::new("Sparkline").size(18.0))
        .child(Sparkline::new([1.0, 3.0, 2.0, 5.0, 4.0, 6.0, 3.0, 7.0]))
        .gap(8.0)
}

fn negatives_view() -> impl View {
    Column::new()
        .child(Text::new("Net change").size(18.0))
        .child(BarChart::new([5.0, -3.0, 8.0, -6.0, 2.0, -1.0, 4.0]).title("Net change"))
        .gap(8.0)
}

/// A dashboard composing three charts, each entity keeping its fixed `SERIES` slot
/// (SOUL §6.1 — assign color by entity in fixed order, never cycled).
fn dashboard_view() -> impl View {
    Column::new()
        .child(Text::new("Dashboard").size(20.0))
        .child(Text::new("Revenue"))
        .child(
            BarChart::new([3.0, 7.0, 2.0, 9.0, 4.0, 6.0])
                .title("Revenue")
                .color(SERIES[0]),
        )
        .child(Text::new("Trend"))
        .child(
            LineChart::new(TREND)
                .title("Trend")
                .color(SERIES[1])
                .markers(true),
        )
        .child(Text::new("Load"))
        .child(Sparkline::new([2.0, 4.0, 3.0, 6.0, 5.0, 7.0, 4.0, 8.0]).color(SERIES[2]))
        .gap(6.0)
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "charts",
    about = "schnellui one-shot chart screenshotter (SOUL §7.1)"
)]
struct Cli {
    /// scenario to render.
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    /// output PNG path.
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 420)]
    width: u32,
    // Tall enough for the three-chart dashboard plus the shared outer gutter.
    #[arg(long, default_value_t = 480)]
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
    /// opt-in **windowed** (non-headless) mode (SOUL §8): mount the chosen `--scenario`
    /// and open a real window with a live event loop instead of writing a PNG. Charts
    /// are static figures, so this just displays them. Smoke-test with
    /// `SCHNELLUI_AUTOCLOSE_MS=<n>` to auto-exit.
    #[arg(long)]
    windowed: bool,
}

/// Builds the app for a scenario in its target state (SOUL §7.5). Every chart scenario
/// is *constructed* directly (charts are static figures — no drive path needed). Each
/// arm mounts its own concrete `impl View` (the `View` trait is object-consuming, so a
/// `Box<dyn View>` is not itself a `View` — mount the concrete type per arm).
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::BarBasic => {
            App::mount_with_size_scaled(stage(bar_basic_view()), width, height, scale)
        }
        Scenario::LineBasic => {
            App::mount_with_size_scaled(stage(line_basic_view()), width, height, scale)
        }
        Scenario::Sparkline => {
            App::mount_with_size_scaled(stage(sparkline_view()), width, height, scale)
        }
        Scenario::Negatives => {
            App::mount_with_size_scaled(stage(negatives_view()), width, height, scale)
        }
        Scenario::Dashboard => {
            App::mount_with_size_scaled(stage(dashboard_view()), width, height, scale)
        }
    }
}

/// All `Chart`-role nodes in the dumped a11y tree, in pre-order (SOUL §6.5). Used to
/// assert each chart's summary — including the nameless sparkline, which cannot be
/// located by name.
fn charts_in(tree: &A11yTreeDump) -> Vec<&A11yNodeDump> {
    fn walk<'a>(node: &'a A11yNodeDump, out: &mut Vec<&'a A11yNodeDump>) {
        if node.role == "chart" {
            out.push(node);
        }
        for c in &node.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    if let Some(r) = &tree.root {
        walk(r, &mut out);
    }
    out
}

/// Asserts the chart at pre-order index `i` carries `needle` in its accessible summary
/// value (SOUL §7.5 oracle — the a11y summary is the primary correctness check).
fn assert_chart_value(charts: &[&A11yNodeDump], i: usize, needle: &str) -> Result<(), String> {
    let node = charts
        .get(i)
        .ok_or_else(|| format!("missing chart #{i} (found {})", charts.len()))?;
    match &node.value {
        Some(v) if v.contains(needle) => Ok(()),
        other => Err(format!(
            "chart #{i} value {other:?} does not contain {needle:?}"
        )),
    }
}

/// Runs the scenario's embedded a11y assertions (SOUL §7.5 oracle). Each chart's
/// deterministic summary (`n=… min=… max=… last=…`, SOUL §6.1) is the ground truth.
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let charts = charts_in(&tree);
    match scenario {
        Scenario::BarBasic => assert_chart_value(&charts, 0, "n=5 min=2 max=9 last=4"),
        Scenario::LineBasic => assert_chart_value(&charts, 0, "n=12 min=4 max=12 last=12"),
        Scenario::Sparkline => assert_chart_value(&charts, 0, "n=8 min=1 max=7 last=7"),
        Scenario::Negatives => assert_chart_value(&charts, 0, "n=7 min=-6 max=8 last=4"),
        Scenario::Dashboard => {
            if charts.len() != 3 {
                return Err(format!(
                    "dashboard expects 3 charts, found {}",
                    charts.len()
                ));
            }
            assert_chart_value(&charts, 0, "n=6 min=2 max=9 last=6")?; // BarChart (Revenue)
            assert_chart_value(&charts, 1, "n=12 min=4 max=12 last=12")?; // LineChart (Trend)
            assert_chart_value(&charts, 2, "n=8 min=2 max=8 last=8")?; // Sparkline (Load)
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
    // instead of writing a PNG. Headless PNG output stays the default.
    if cli.windowed {
        let app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        return match app.run_windowed("charts") {
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
