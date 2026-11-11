//! # global_shortcuts: system-wide keyboard shortcuts (SOUL §7.1)
//!
//! Registers two global shortcuts, `increment` and `reset`, shows the chords
//! the system bound to them and counts presses. They fire while the window is
//! unfocused. Headless runs never ask the system for shortcuts, so the
//! scenarios drive them with `App::dispatch_global_shortcut`, the same call
//! `schnellui-debug global-shortcut <id>` makes in a live window.
//!
//! Run `global_shortcuts --scenario initial --windowed` and look at the list.
//! On Hyprland the chords stay empty until the config binds
//! `global, dev.schnellui.GlobalShortcuts:increment`.

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, Role};
use schnellui::platform::{GlobalShortcut, GlobalShortcutBinding, GlobalShortcutPhase};
use schnellui::signal::create_signal;
use schnellui::testing::{assert_value_contains, find_by_role_name};
use schnellui::view;
use schnellui::widgets::{Pad, Stack};
use schnellui::{App, Shortcut};
use strum::IntoEnumIterator;

const APP_ID: &str = "dev.schnellui.GlobalShortcuts";
const WINDOW_PADDING: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// nothing pressed yet, no chords reported.
    Initial,
    /// `increment` driven three times, `reset` once, `increment` twice more.
    Pressed,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::Initial => "initial",
            Scenario::Pressed => "pressed",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "global_shortcuts",
    about = "schnellui one-shot screenshotter (SOUL §7.1)"
)]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 520)]
    width: u32,
    #[arg(long, default_value_t = 220)]
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
    /// run the scenario's embedded a11y assertions, nonzero on failure.
    #[arg(long)]
    assert: bool,
    /// open a real window; the shortcuts then fire while it is unfocused.
    #[arg(long)]
    windowed: bool,
    /// run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
}

/// What the list line says for the bindings the system reported.
fn describe(bindings: &[GlobalShortcutBinding]) -> String {
    if bindings.is_empty() {
        return "the system has reported no bindings".into();
    }
    bindings
        .iter()
        .map(|binding| {
            let chord = if binding.trigger.is_empty() {
                "unbound"
            } else {
                binding.trigger.as_str()
            };
            format!("{}: {chord}", binding.id)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    let presses = create_signal(0i64);
    let bound = create_signal(describe(&[]));
    let view = view! {
        column(gap = 8.0) {
            text { "Global shortcuts" }
            text(role = Role::Status) { (presses.get().to_string()) }
            text { (bound.get()) }
        }
    };
    let stage = Stack::new()
        .fill()
        .child(Pad::all(WINDOW_PADDING).child(view));
    let mut app = App::mount_with_size_scaled(stage, width, height, scale);
    app.register_global_shortcut(
        GlobalShortcut::new("increment", "Add one to the counter")
            .preferred_trigger(Shortcut::new('i', true, false, true)),
        move |phase| {
            if phase == GlobalShortcutPhase::Pressed {
                presses.set(presses.get() + 1);
            }
        },
    );
    app.register_global_shortcut(
        GlobalShortcut::new("reset", "Set the counter back to zero")
            .preferred_trigger(Shortcut::new('r', true, false, true)),
        move |phase| {
            if phase == GlobalShortcutPhase::Pressed {
                presses.set(0);
            }
        },
    );
    app.on_global_shortcuts_changed(move |bindings| bound.set(describe(bindings)));
    if scenario == Scenario::Pressed {
        let press = |app: &mut App, id: &str| {
            app.dispatch_global_shortcut(id, GlobalShortcutPhase::Pressed);
        };
        for _ in 0..3 {
            press(&mut app, "increment");
        }
        press(&mut app, "reset");
        for _ in 0..2 {
            press(&mut app, "increment");
        }
    }
    app
}

fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    find_by_role_name(&tree, "status", None).ok_or("missing status text")?;
    if !app.global_shortcuts().is_empty() {
        return Err("a headless run must not get bindings from the system".into());
    }
    let expected = match scenario {
        Scenario::Initial => "0",
        Scenario::Pressed => "2",
    };
    assert_value_contains(&tree, "status", None, expected)?;
    let ids: Vec<_> = app.registered_global_shortcuts().map(|s| s.id()).collect();
    if ids != ["increment", "reset"] {
        return Err(format!("registered {ids:?}"));
    }
    Ok(())
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame();
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
    if cli.headless_windowed {
        std::env::set_var("SCHNELLUI_HEADLESS_WINDOWED", "1");
    }
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
            let code = render_one(s, &cli, &format!("{dir}/{}.png", s.name()));
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
    if cli.windowed || cli.headless_windowed {
        // The portal keys global shortcuts by app id, so name the app first.
        if let Err(e) = schnellui::platform::set_app_id(APP_ID) {
            eprintln!("app id: {e}");
        }
        let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        app.set_escape_exits(true);
        return match app.run_windowed("global shortcuts") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenarios_satisfy_their_assertions() {
        for scenario in Scenario::iter() {
            let mut app = scenario_app(scenario, 520, 220, 1.0);
            app.frame();
            run_assertions(scenario, &app).unwrap_or_else(|e| panic!("{}: {e}", scenario.name()));
        }
    }

    #[test]
    fn the_observer_shows_what_the_system_bound() {
        let bindings = [
            GlobalShortcutBinding {
                id: "increment".into(),
                description: String::new(),
                trigger: "Ctrl+Alt+I".into(),
            },
            GlobalShortcutBinding {
                id: "reset".into(),
                description: String::new(),
                trigger: String::new(),
            },
        ];
        assert_eq!(describe(&bindings), "increment: Ctrl+Alt+I, reset: unbound");
    }
}
