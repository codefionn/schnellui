//! # temperature — derived state with signals and memos (SOUL §3.1, §7)
//!
//! The lesson: **one source of truth, everything else derived.** `celsius: i64` is
//! the *only* signal. `fahrenheit` is a **memo** (`create_memo`) — a cached value
//! derived as `c * 9 / 5 + 32`, *not* a second signal. A second signal could drift
//! out of sync and would store state already implied by `celsius`; a memo cannot
//! drift, because it is only ever a *function* of its inputs. The "+"/"−" buttons
//! mutate **only** `celsius`; fahrenheit follows automatically — that is the point.
//!
//! ## Push-then-pull (SOUL §3.1)
//!
//! `signal.set()` recomputes nothing — it only *pushes*, marking dependents `Dirty`
//! (the memo) and returning. The *pull* happens later: `App::frame()` settles the
//! graph and re-reads each dynamic slot, and reading `fahrenheit.get()` recomputes
//! the dirty memo exactly once, lazily, on demand. Five clicks push five times, but
//! the derived value costs one recompute at read — work ∝ what is read, not set.
//!
//! ## Construct vs. drive (SOUL §7.5)
//!
//! `freezing` is **constructed** at 0 °C (fast, pure). `warmer` is **driven**: from
//! 0, it dispatches five `Click`s at the "+" button located by Role+name (never
//! pixels), through the *same* handler a real click fires (§6.3) — proving the state
//! is reachable, not merely constructible, and that the memo tracks live mutations.

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, A11yNodeDump, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::signal::{create_memo, create_signal};
use schnellui::view;
use schnellui::widgets::{Pad, View};
use schnellui::App;
use schnellui_testing::{find_by_role_name, SnapshotConfig};
use strum::IntoEnumIterator;

const WINDOW_PADDING: f32 = 20.0;

fn stage(content: impl View) -> impl View {
    Pad::all(WINDOW_PADDING).child(content)
}

/// The enumerable scenario table (SOUL §7.1 — `clap::ValueEnum` + `strum::EnumIter`
/// so `--scenario` is validated and the set is introspectable).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// constructed at 0 °C — freezing point (32 °F). A pure appearance check (§7.5).
    Freezing,
    /// *driven*: start at 0 °C, dispatch 5 `Click`s at the "+" button so the signal
    /// climbs to 5 °C — the memo follows to 41 °F (SOUL §7.5 drive, §6.3).
    Warmer,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output + golden path).
    fn name(self) -> &'static str {
        match self {
            Scenario::Freezing => "freezing",
            Scenario::Warmer => "warmer",
        }
    }
}

/// The screenshotter CLI contract (SOUL §7.1) — identical surface to every example.
#[derive(Parser, Debug)]
#[command(
    name = "temperature",
    about = "schnellui one-shot screenshotter (SOUL §7.1)"
)]
struct Cli {
    /// scenario to render.
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    /// output PNG path.
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 520)]
    width: u32,
    #[arg(long, default_value_t = 220)]
    height: u32,
    /// logical→physical scale (SOUL §7.1): shaping + painting run at `size*scale`.
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
    /// opt-in **windowed** (non-headless) mode (SOUL §8): mount the chosen
    /// `--scenario` and open a real window with a live event loop instead of writing a
    /// PNG. The +/− buttons respond to real mouse clicks and the fahrenheit memo
    /// follows. Headless PNG output stays the default. `SCHNELLUI_AUTOCLOSE_MS=<n>`
    /// auto-exits for smoke tests.
    #[arg(long)]
    windowed: bool,
}

/// The temperature UI, built with the `view!` macro (SOUL §3.3). The macro is the
/// first-choice idiom here: the grammar expresses every part of this tree — a static
/// title, two dynamic `(expr)` slots, and event-bound buttons — and lowers it to the
/// same typed builder chain used internally by every macro-authored example.
fn temperature_view(start: i64) -> impl View {
    // The ONE source of truth. Nothing else stores temperature state.
    let celsius = create_signal(start);

    // The DERIVED value: a memo, not a second signal (see the module docs for *why*).
    // `create_memo` caches the result and recomputes only when `celsius` changes —
    // and it reads `celsius` inside, so that dependency is tracked automatically.
    let fahrenheit = create_memo(move || celsius.get() * 9 / 5 + 32);

    view! {
        column {
            // Fully static → hoisted into the skeleton, never touched on update.
            text(size = 18.0) { "Celsius signal to Fahrenheit memo" }

            // Two dynamic Status slots. A `Role::Status` announces its text as its
            // accessible *value* (a live region a screen reader re-reads on change).
            // `Text` has no `.name()` builder, so each slot gets a distinct, queryable
            // identity by folding its label into the content: "celsius: N" vs
            // "fahrenheit: N". Each `(expr)` is a render effect (SOUL §3.3) that reads
            // its signal/memo and mutates the retained node in place on change.
            text(role = Role::Status) { (format!("celsius: {}", celsius.get())) }
            text(role = Role::Status) { (format!("fahrenheit: {}", fahrenheit.get())) }

            // Buttons mutate ONLY `celsius`; the memo is never set by hand.
            row {
                button(on:click = move || celsius.set(celsius.get() - 1)) { "-" }
                button(on:click = move || celsius.set(celsius.get() + 1)) { "+" }
            }
        }
    }
}

/// Builds the app for a scenario in its target state (SOUL §7.5). `Freezing` is
/// *constructed*; `Warmer` is *driven* there through the real inbound `ActionRequest`
/// path (§6.3), proving reachability.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::Freezing => {
            App::mount_with_size_scaled(stage(temperature_view(0)), width, height, scale)
        }
        Scenario::Warmer => {
            let mut app =
                App::mount_with_size_scaled(stage(temperature_view(0)), width, height, scale);
            drive_clicks(&mut app, "+", 5);
            app
        }
    }
}

/// Drives `times` `Click` `ActionRequest`s at the button named `name`, located by
/// Role+name (SOUL §7.5 — semantic query, never pixels). Each dispatch routes through
/// the same handler a mouse click would fire (§6.3), so each advances the `celsius`
/// signal exactly as under real input. (Verbatim the `counter` example's idiom.)
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

/// Finds the first `Status` node whose accessible *value* contains `needle` (SOUL
/// §7.5 — query by semantics). The two status slots carry their label in their value
/// ("celsius: …" / "fahrenheit: …"), so this is how we tell them apart in the oracle.
fn find_status_value_containing<'a>(
    node: &'a A11yNodeDump,
    needle: &str,
) -> Option<&'a A11yNodeDump> {
    if node.role == "status" && node.value.as_deref().is_some_and(|v| v.contains(needle)) {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|c| find_status_value_containing(c, needle))
}

/// Runs the scenario's embedded a11y assertions (SOUL §7.5 oracle). The a11y tree is
/// the *primary* correctness check: memo correctness is observable purely through the
/// semantics — the fahrenheit status value literally reads the derived number.
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let root = tree
        .root
        .as_ref()
        .ok_or_else(|| "empty a11y tree".to_string())?;

    // The "+" button is reachable — the seam the drive script aimed at (§7.5).
    find_by_role_name(&tree, "button", Some("+")).ok_or_else(|| "missing + button".to_string())?;

    let cel = find_status_value_containing(root, "celsius")
        .ok_or_else(|| "missing celsius status".to_string())?
        .value
        .clone()
        .unwrap_or_default();
    let fah = find_status_value_containing(root, "fahrenheit")
        .ok_or_else(|| "missing fahrenheit status".to_string())?
        .value
        .clone()
        .unwrap_or_default();

    // (celsius must contain, fahrenheit must contain) — the memo relation, observed.
    let (c_needle, f_needle) = match scenario {
        Scenario::Freezing => ("0", "32"), // 0 °C → 32 °F
        Scenario::Warmer => ("5", "41"),   // 5 °C → 41 °F  (5*9/5+32)
    };
    if !cel.contains(c_needle) {
        return Err(format!("celsius {cel:?} does not contain {c_needle:?}"));
    }
    if !fah.contains(f_needle) {
        return Err(format!("fahrenheit {fah:?} does not contain {f_needle:?}"));
    }
    Ok(())
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame(); // one synchronous frame: the PULL that recomputes the memo (SOUL §7.1)

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
    // loop instead of writing a PNG. The +/− buttons work with real clicks.
    if cli.windowed {
        let app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        return match app.run_windowed("temperature") {
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

    // Read so the bless env-var contract is honored by the harness (§7.4).
    let _cfg = SnapshotConfig::from_env("snapshots");
    render_one(scenario, &cli, &out)
}

/// One `--manifest` entry (SOUL §7.1). Hand-rolled to avoid a serde derive on a
/// throwaway type.
fn serde_manifest_entry(name: &str, path: &str, width: u32, height: u32) -> String {
    format!("{{\"scenario\":\"{name}\",\"path\":\"{path}\",\"width\":{width},\"height\":{height}}}")
}
