//! # counter — a one-shot screenshotter example (SOUL §7.1)
//!
//! Builds the GPU context once, puts a scenario's UI in a specific state,
//! renders exactly one synchronous frame, writes a PNG (and optionally an a11y
//! dump), asserts the a11y oracle, and exits 0. **No event loop** (SOUL §7.1).

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::signal::create_signal;
use schnellui::view;
use schnellui::widgets::{Pad, Stack, View};
use schnellui::App;
use schnellui_testing::{assert_value_contains, find_by_role_name, SnapshotConfig};
use strum::IntoEnumIterator;

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
    /// the counter at its initial value (0) — constructed (SOUL §7.5).
    CounterZero,
    /// the counter *driven* to five by dispatching 5 `Click` `ActionRequest`s at
    /// the increment button, located by Role+name (SOUL §7.5 drive, §6.3).
    CounterFive,
    /// an empty window (baseline).
    Empty,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output + golden path).
    fn name(self) -> &'static str {
        match self {
            Scenario::CounterZero => "counter_zero",
            Scenario::CounterFive => "counter_five",
            Scenario::Empty => "empty",
        }
    }
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "counter",
    about = "schnellui one-shot screenshotter (SOUL §7.1)"
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
    #[arg(long, default_value_t = 200)]
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
    /// The increment button responds to real mouse clicks. Headless PNG output stays
    /// the default. Smoke-test with `SCHNELLUI_AUTOCLOSE_MS=<n>` to auto-exit.
    #[arg(long)]
    windowed: bool,
}

/// The counter UI, constructed at `start` with the first-choice `view!` syntax
/// (SOUL §3.3). Dynamic text and the click handler remain reactive slots in the
/// retained tree; this function itself still runs only once at mount.
fn counter_view(start: i64) -> impl View {
    let count = create_signal(start);
    view! {
        column(gap = 8.0) {
            text { "Counter" }
            text(role = Role::Status) { (count.get().to_string()) }
            button(on:click = move || count.set(count.get() + 1)) { "increment" }
        }
    }
}

/// Builds the app for a scenario in its target state (SOUL §7.5). `CounterZero` is
/// *constructed*; `CounterFive` is *driven* there through the real inbound
/// `ActionRequest` path (§6.3) — proving the state is reachable, not merely
/// constructible.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::CounterZero => {
            App::mount_with_size_scaled(stage(counter_view(0)), width, height, scale)
        }
        Scenario::CounterFive => {
            let mut app = App::mount_with_size_scaled(stage(counter_view(0)), width, height, scale);
            drive_clicks(&mut app, "increment", 5);
            app
        }
        Scenario::Empty => {
            let mut app = App::new(width, height);
            app.set_scale(scale);
            app
        }
    }
}

/// Drives `times` `Click` `ActionRequest`s at the button named `name`, located by
/// Role+name (SOUL §7.5 — semantic query, never pixels). Each dispatch routes
/// through the same handler a mouse click would fire (§6.3), so the counter's
/// signal advances exactly as under real input.
fn drive_clicks(app: &mut App, name: &str, times: u32) {
    let Some(id) = app.find_widget(Role::Button, Some(name)) else {
        eprintln!("drive: no button named {name:?}");
        return;
    };
    let target = to_access_id(id);
    for _ in 0..times {
        let req = ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: target,
            data: None,
        };
        app.dispatch_action(&req);
    }
}

/// Runs the scenario's embedded a11y assertions (SOUL §7.5 oracle).
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    match scenario {
        Scenario::CounterFive => {
            assert_value_contains(&tree, "status", None, "5")?;
            find_by_role_name(&tree, "button", Some("increment"))
                .ok_or_else(|| "missing increment button".to_string())?;
            Ok(())
        }
        Scenario::CounterZero => {
            find_by_role_name(&tree, "button", Some("increment"))
                .ok_or_else(|| "missing increment button".to_string())?;
            assert_value_contains(&tree, "status", None, "0")?;
            Ok(())
        }
        Scenario::Empty => Ok(()),
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
    // Opt-in windowed mode (SOUL §8): mount the chosen scenario and run the event
    // loop instead of writing a PNG. The increment button works with real clicks.
    if cli.windowed {
        let app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        return match app.run_windowed("counter") {
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

/// One `--manifest` entry (SOUL §7.1). Kept as a hand-rolled object to avoid a
/// serde derive on a throwaway type.
fn serde_manifest_entry(name: &str, path: &str, width: u32, height: u32) -> String {
    format!("{{\"scenario\":\"{name}\",\"path\":\"{path}\",\"width\":{width},\"height\":{height}}}")
}
