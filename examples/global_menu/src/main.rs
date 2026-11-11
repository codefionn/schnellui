//! # global_menu: the system menu bar (SOUL §7.1)
//!
//! Sets a File, Edit and View menu with `App::set_global_menu` and shows in the
//! window whether the system has a global menu and which item was chosen last.
//! schnellui draws no menu bar in the window: on a system without a global menu
//! (Linux and Windows today) nothing is shown and the app would offer its
//! commands some other way. On macOS the bar appears at the top of the screen.
//!
//! Headless runs never ask the system for a menu, so the scenarios drive items
//! with `App::dispatch_global_menu_item`, the same call
//! `schnellui-debug global-menu-item <id>` makes in a live window.
//!
//! Run `global_menu --scenario initial --windowed` and look at the first line.

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, Role};
use schnellui::platform::{Menu, MenuAction, MenuBar, StandardItem};
use schnellui::signal::{create_signal, Signal};
use schnellui::testing::{assert_value_contains, find_by_role_name};
use schnellui::view;
use schnellui::widgets::{Pad, Stack};
use schnellui::{App, Shortcut};
use strum::IntoEnumIterator;

const APP_ID: &str = "dev.schnellui.GlobalMenu";
const WINDOW_PADDING: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// no item chosen yet.
    Initial,
    /// File > Open, then View > Sidebar twice (on, then off), then the disabled Save.
    Chosen,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::Initial => "initial",
            Scenario::Chosen => "chosen",
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "global_menu",
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
    /// open a real window; on macOS the menu shows in the menu bar.
    #[arg(long)]
    windowed: bool,
    /// run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
}

/// What the first line says about the system.
fn support_line(supported: bool) -> &'static str {
    if supported {
        "Global menu: supported, see the menu bar at the top of the screen"
    } else {
        "Global menu: not supported on this system, no menu bar is shown"
    }
}

/// The menu the example sets. Ids are stable names the handlers key on.
fn build_menu(sidebar_shown: bool) -> MenuBar {
    MenuBar::new()
        .menu(
            Menu::new("Global menu")
                .item(StandardItem::About)
                .separator()
                .item(StandardItem::Hide)
                .item(StandardItem::HideOthers)
                .item(StandardItem::ShowAll)
                .separator()
                .item(StandardItem::Quit),
        )
        .menu(
            Menu::new("File")
                .item(MenuAction::new("file.new", "New").shortcut(Shortcut::command('n')))
                .item(MenuAction::new("file.open", "Open").shortcut(Shortcut::command('o')))
                .item(
                    Menu::new("Open Recent")
                        .item(MenuAction::new("file.recent.notes", "notes.txt"))
                        .item(MenuAction::new("file.recent.todo", "todo.txt")),
                )
                .separator()
                .item(
                    MenuAction::new("file.save", "Save")
                        .shortcut(Shortcut::command('s'))
                        .enabled(false),
                )
                .separator()
                .item(StandardItem::Minimize)
                .item(StandardItem::Zoom),
        )
        .menu(
            Menu::new("Edit")
                .item(StandardItem::Undo)
                .item(StandardItem::Redo)
                .separator()
                .item(StandardItem::Cut)
                .item(StandardItem::Copy)
                .item(StandardItem::Paste)
                .item(StandardItem::SelectAll),
        )
        .menu(
            Menu::new("View").item(
                MenuAction::new("view.sidebar", "Show Sidebar")
                    .shortcut(Shortcut::new('s', true, false, true))
                    .checked(Some(sidebar_shown)),
            ),
        )
}

fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    let last = create_signal(String::from("Last chosen: nothing yet"));
    let view = view! {
        column(gap = 8.0) {
            text { "Global menu" }
            text { (support_line(App::global_menu_supported())) }
            text(role = Role::Status) { (last.get()) }
        }
    };
    let stage = Stack::new()
        .fill()
        .child(Pad::all(WINDOW_PADDING).child(view));
    let mut app = App::mount_with_size_scaled(stage, width, height, scale);
    app.set_global_menu(build_menu(false))
        .expect("the example's ids are valid and unique");
    for id in [
        "file.new",
        "file.open",
        "file.save",
        "file.recent.notes",
        "file.recent.todo",
    ] {
        choose_handler(&mut app, id, last);
    }
    // The handler tracks the state it shows in the window. A real app would also
    // call `App::update_global_menu_item` to move the check mark in the menu.
    app.on_global_menu_item("view.sidebar", {
        let toggled = create_signal(false);
        move || {
            toggled.set(!toggled.get());
            last.set(format!("Last chosen: view.sidebar ({})", toggled.get()));
        }
    });
    if scenario == Scenario::Chosen {
        for id in ["file.open", "view.sidebar", "view.sidebar", "file.save"] {
            app.dispatch_global_menu_item(id);
        }
    }
    app
}

fn choose_handler(app: &mut App, id: &'static str, last: Signal<String>) {
    app.on_global_menu_item(id, move || last.set(format!("Last chosen: {id}")));
}

fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    find_by_role_name(&tree, "status", None).ok_or("missing status text")?;
    let expected = match scenario {
        Scenario::Initial => "nothing yet",
        // The disabled Save did not run, so the last item is the sidebar,
        // toggled on and then off.
        Scenario::Chosen => "view.sidebar (false)",
    };
    assert_value_contains(&tree, "status", None, expected)?;
    let titles: Vec<_> = app
        .global_menu()
        .menus()
        .iter()
        .map(|m| m.title())
        .collect();
    if titles != ["Global menu", "File", "Edit", "View"] {
        return Err(format!("menus {titles:?}"));
    }
    if app.global_menu().find_action("file.recent.todo").is_none() {
        return Err("a submenu item is not found by id".into());
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
        if let Err(e) = schnellui::platform::set_app_id(APP_ID) {
            eprintln!("app id: {e}");
        }
        let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        app.set_escape_exits(true);
        return match app.run_windowed("global menu") {
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

    /// Resize the live window: every scenario lays out as a fresh mount at the
    /// new size does, so no size from the launch is left in the page.
    #[test]
    fn a_live_resize_matches_a_fresh_mount() {
        schnellui::testing::fuzz::assert_resize_matches_fresh(
            Scenario::iter(),
            (520, 220),
            (400, 300),
            &[1.0, 1.5],
            scenario_app,
        );
    }

    #[test]
    fn scenarios_satisfy_their_assertions() {
        for scenario in Scenario::iter() {
            let mut app = scenario_app(scenario, 520, 220, 1.0);
            app.frame();
            run_assertions(scenario, &app).unwrap_or_else(|e| panic!("{}: {e}", scenario.name()));
        }
    }

    #[test]
    fn the_first_line_names_the_support_state() {
        assert!(support_line(true).contains("supported, see"));
        assert!(support_line(false).contains("not supported"));
    }

    #[test]
    fn the_menu_is_valid() {
        assert_eq!(build_menu(true).validate(), Ok(()));
        assert_eq!(
            build_menu(true)
                .find_action("view.sidebar")
                .unwrap()
                .checked_state(),
            Some(true)
        );
    }
}
