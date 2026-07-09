//! # settings — stateful widgets + accessibility as the oracle (SOUL §6, §7)
//!
//! Lesson: **interactive stateful widgets, and reading UI state back through the
//! accessibility tree.** A `Settings` panel stacks three `Checkbox`es — each backed
//! by its own `bool` signal via `on_toggle` — a `Switch` (its own signal), a
//! `Divider` separating the two sections, a read-only `ProgressBar`, and one
//! *derived* summary `Text` ("N of M enabled") that a `Memo` recomputes reactively
//! from the checkbox signals.
//!
//! The teaching point (SOUL §7.5): the **screen reader's view is the test oracle.**
//! We never inspect pixels to check correctness. We drive the UI the way an assistive
//! tool would — dispatching AccessKit `ActionRequest`s at widgets located by *role +
//! name* — and then assert on the dumped a11y tree: each checkbox's checked state, the
//! switch's checked state, the progress bar's percentage, and the summary's live
//! value. The PNG is only a secondary visual check.
//!
//! Run:  `settings --scenario all_enabled --assert --out settings.png`

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::{Scene, WidgetId};
use schnellui::signal::{create_memo, create_signal};
// `view!` is re-exported from the umbrella crate; `View` is the trait every widget
// (and every `view!` block) implements.
use schnellui::view;
use schnellui::widgets::{Pad, View};
use schnellui::App;
use schnellui_testing::{assert_value_contains, find_by_role_name};
use strum::IntoEnumIterator;

/// The settings, in tree order. Each string is both the visible label *and* the
/// checkbox's accessible name — the handle SOUL §7.5 uses to find, drive, and assert
/// it. `M` (the "of M" in the summary) is just `SETTINGS.len()`.
const SETTINGS: [&str; 3] = ["dark mode", "notifications", "telemetry"];
const WINDOW_PADDING: f32 = 20.0;

fn stage(content: impl View) -> impl View {
    Pad::all(WINDOW_PADDING).child(content)
}

/// The enumerable scenario table (SOUL §7.1 — `clap::ValueEnum` + `strum::EnumIter`,
/// so `--scenario` is validated and `--list`/`--all` can introspect the set).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// all checkboxes off, switch off — reached by **construction** (SOUL §7.5).
    Defaults,
    /// all checkboxes on — reached by **driving**: dispatch the `Click` action each
    /// `Checkbox` routes, at each checkbox located by Role+name (SOUL §7.5, §6.3).
    AllEnabled,
    /// the `Switch` toggled on — reached by **driving** a `Click` `ActionRequest` at
    /// the widget located by `Role::Switch` (SOUL §7.5, §6.3). Checkboxes stay off.
    SwitchOn,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output).
    fn name(self) -> &'static str {
        match self {
            Scenario::Defaults => "defaults",
            Scenario::AllEnabled => "all_enabled",
            Scenario::SwitchOn => "switch_on",
        }
    }
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "settings",
    about = "schnellui one-shot screenshotter (SOUL §7.1)"
)]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 360)]
    width: u32,
    #[arg(long, default_value_t = 260)]
    height: u32,
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// print scenario names one per line and exit.
    #[arg(long)]
    list: bool,
    /// render every scenario into `--out-dir`.
    #[arg(long)]
    all: bool,
    #[arg(long)]
    out_dir: Option<String>,
    /// write the AccessKit tree JSON alongside the PNG.
    #[arg(long)]
    dump_a11y: Option<String>,
    /// run the a11y assertions (the oracle); nonzero on failure.
    #[arg(long)]
    assert: bool,
    /// opt-in **windowed** (non-headless) mode (SOUL §8): open a real window with the
    /// chosen scenario instead of writing a PNG. Checkboxes toggle with real clicks.
    #[arg(long)]
    windowed: bool,
}

/// The settings UI. We use the **`view!` macro** rather than a builder chain because
/// its grammar already covers checkboxes: `checkbox(on:toggle = …)` lowers to
/// `Checkbox::new(false).on_toggle(…)` (see `schnellui-view-parser`). The macro splits
/// this into a static skeleton (title, labels) and the handful of dynamic sites (the
/// three toggle handlers + the reactive summary) — SOUL §3.3.
fn settings_view() -> impl View {
    // One `bool` signal per setting — the widgets' backing state (SOUL §3.1).
    let dark = create_signal(false);
    let notifications = create_signal(false);
    let telemetry = create_signal(false);
    // The `Switch`'s own backing signal — a separate on/off preference (SOUL §3.1).
    let reduced_motion = create_signal(false);

    // The summary is a *derived* node: a `Memo` over the three checkbox signals.
    // Reading them inside the compute auto-subscribes it (SOUL §3.3), so flipping any
    // checkbox recomputes exactly this one string — a 3-source → 1-derived diamond.
    // The switch is deliberately *not* counted, so the summary stays stable across it.
    let summary = create_memo(move || {
        let n = [dark, notifications, telemetry]
            .into_iter()
            .filter(|s| s.get())
            .count();
        format!("{n} of {} enabled", SETTINGS.len())
    });

    view! {
        column(gap = 12.0) {
            text(size = 22.0) { "Settings" }
            // Each row pairs a checkbox with its visible label. `on:toggle` receives
            // the checkbox's *new* checked state and writes it straight into the
            // signal — the same handler an inbound `Click` action fires (SOUL §6.3).
            row(gap = 8.0) {
                checkbox(on:toggle = move |v: bool| dark.set(v))
                text { "dark mode" }
            }
            row(gap = 8.0) {
                checkbox(on:toggle = move |v: bool| notifications.set(v))
                text { "notifications" }
            }
            row(gap = 8.0) {
                checkbox(on:toggle = move |v: bool| telemetry.set(v))
                text { "telemetry" }
            }
            // A decorative separator between the checkbox section and the switch /
            // status section (SOUL §8.1 — a `Group`, transparent to the a11y oracle).
            divider
            // A `Switch` toggling a preference. `on:toggle` writes its *new* state into
            // the signal — the same handler an inbound `Click` action fires (SOUL §6.3).
            row(gap = 8.0) {
                switch(on:toggle = move |v: bool| reduced_motion.set(v))
                text { "reduced motion" }
            }
            // A read-only `ProgressBar` (SOUL §8.1). Its accessible value is the
            // percentage — an oracle-visible status the run below asserts ("60%").
            progress(value = 60.0, min = 0.0, max = 100.0)
            // A dynamic text slot reading the memo. `Role::Status` marks it a live
            // region, and makes `run_dynamic_slots` publish its value into the a11y
            // column — which is exactly what the oracle reads back below (SOUL §7.5).
            text(role = Role::Status) { (summary.get()) }
        }
    }
}

/// Builds the app for a scenario in its target state (SOUL §7.5). `Defaults` is left
/// as constructed; `AllEnabled` is *driven* there through the real inbound
/// `ActionRequest` path — proving the state is reachable, not merely constructible.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    let mut app = App::mount_with_size_scaled(stage(settings_view()), width, height, scale);
    // The page background is a design token (SOUL §8.1): the ambient theme's
    // page colour keeps the white checkbox surfaces, check marks, and glyphs
    // distinct from the background in the PNG (SOUL §7.3).
    app.set_clear_color(app.theme().page);

    // The `Checkbox` primitive has no label slot, so it builds unnamed. We set each
    // accessible name to match its visible label — SOUL §7.5 locates widgets by
    // Role+name, so an unnamed checkbox could be neither driven nor asserted.
    label_checkboxes(&mut app);

    if scenario == Scenario::AllEnabled {
        for name in SETTINGS {
            toggle_checkbox(&mut app, name);
        }
    }
    if scenario == Scenario::SwitchOn {
        toggle_switch(&mut app);
    }
    app
}

/// Assigns each checkbox its accessible name, in tree order, from [`SETTINGS`].
fn label_checkboxes(app: &mut App) {
    fn collect(scene: &Scene, id: WidgetId, out: &mut Vec<WidgetId>) {
        let is_checkbox = scene
            .a11y(id)
            .is_some_and(|a| a.role == Role::CheckBox.as_u16());
        if is_checkbox {
            out.push(id);
        }
        if let Some(node) = scene.node(id) {
            for &c in &node.children {
                collect(scene, c, out);
            }
        }
    }
    let mut ids = Vec::new();
    if let Some(root) = app.scene().root() {
        collect(app.scene(), root, &mut ids);
    }
    for (id, name) in ids.into_iter().zip(SETTINGS) {
        app.scene_mut().a11y_mut(id).name = Some(name.to_string());
    }
}

/// Drives one `Click` `ActionRequest` at the checkbox named `name`, located by
/// Role+name (SOUL §7.5 — semantic query, never pixels). A `Checkbox` advertises the
/// **`Click`** action (not a distinct `Toggle`), and the router flips its checked
/// state and fires the same `on_toggle` a mouse click would (SOUL §6.3).
fn toggle_checkbox(app: &mut App, name: &str) {
    let Some(id) = app.find_widget(Role::CheckBox, Some(name)) else {
        eprintln!("drive: no checkbox named {name:?}");
        return;
    };
    let req = ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    };
    app.dispatch_action(&req);
}

/// Drives one `Click` `ActionRequest` at the `Switch`, located by `Role::Switch`
/// (SOUL §7.5 — semantic query, never pixels). A `Switch` advertises the **`Click`**
/// action; the router flips its CHECKED state and fires the same `on_toggle` a mouse
/// click would (SOUL §6.3). There is a single switch, so no name is needed to aim it.
fn toggle_switch(app: &mut App) {
    let Some(id) = app.find_widget(Role::Switch, None) else {
        eprintln!("drive: no switch found");
        return;
    };
    let req = ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    };
    app.dispatch_action(&req);
}

/// The oracle (SOUL §7.5): read the *dumped a11y tree* — a screen reader's view — and
/// assert every checkbox's checked state, the switch's checked state, the progress
/// bar's percentage, and the summary's live value. No pixels.
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let want_checked = scenario == Scenario::AllEnabled;

    for name in SETTINGS {
        let node = find_by_role_name(&tree, "checkbox", Some(name))
            .ok_or_else(|| format!("missing checkbox {name:?}"))?;
        let is_checked = node.state.iter().any(|s| s == "checked");
        if is_checked != want_checked {
            return Err(format!(
                "checkbox {name:?}: checked={is_checked}, want {want_checked}"
            ));
        }
    }

    // The `Switch` is CHECKED only in the `SwitchOn` scenario — where it was *driven*
    // there through the real inbound `Click` `ActionRequest` path (SOUL §6.3, §7.5).
    // Located by `Role::Switch` (semantics, never pixels), asserted from the a11y dump.
    let want_switch = scenario == Scenario::SwitchOn;
    let switch =
        find_by_role_name(&tree, "switch", None).ok_or_else(|| "missing switch".to_string())?;
    let switch_checked = switch.state.iter().any(|s| s == "checked");
    if switch_checked != want_switch {
        return Err(format!(
            "switch: checked={switch_checked}, want {want_switch}"
        ));
    }

    // The read-only `ProgressBar` announces its percentage as its a11y value (SOUL
    // §6.1) — the same in every scenario (60/100 → "60%").
    assert_value_contains(&tree, "progress_indicator", None, "60%")?;

    // The summary the memo produced: "N of M enabled" (e.g. "2 of 2" in the canonical
    // spec; here M = 3). This is the a11y *value* of the `Status` node. The switch does
    // not contribute, so it stays "0 of 3" even when the switch is on.
    let n = if want_checked { SETTINGS.len() } else { 0 };
    let expect = format!("{n} of {}", SETTINGS.len());
    assert_value_contains(&tree, "status", None, &expect)
}

/// Renders one scenario: build → one synchronous frame → (dump) → (assert) → PNG.
fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame(); // one synchronous frame settles the summary memo (SOUL §7.1)

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
        for s in Scenario::iter() {
            let out = format!("{dir}/{}.png", s.name());
            let code = render_one(s, &cli, &out);
            if code != ExitCode::SUCCESS {
                return code;
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
        return match app.run_windowed("settings") {
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
