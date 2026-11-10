//! # playground — every component on one stage (SOUL §7, §8.1)
//!
//! Lesson: **the component library, end to end.** One `gallery` scenario shows every
//! content widget schnellui ships — text, button, link, badge, checkbox, switch,
//! radio group, input slider, progress bar, loading spinner, text input, image,
//! icon, divider, tabs, list, and the table — plus a focused
//! decorated/undecorated dialog comparison. One focused
//! scenario per *interactive* family proves its reactive
//! loop: drive the widget through the real inbound AccessKit `ActionRequest` path
//! (SOUL §6.3), let a signal-bound `Status` text react, and assert the result from
//! the dumped a11y tree (SOUL §7.5). The PNG shows how the components **look**; the
//! oracle checks what they **are**.
//!
//! Every scenario renders inside the shared **page chrome**: the example-switcher
//! tab bar (one tab per scenario, the current one selected) and the **Theme
//! dropdown** — the design-system switcher (SOUL §8.1 [`Theme`]) — above a
//! divider, all wrapped in the page padding. A mounted tree is a static skeleton
//! (SOUL §3.3), so in `--windowed` mode selecting another tab, opening the
//! dropdown, or picking a theme **remounts** the playground in the chosen state
//! inside the same window (the [`App::run_windowed_with`] remount hook — the
//! window never closes and reopens): the whole design system swaps on the fly.
//! Headless shots render the chrome inert, with the dropdown closed and the
//! `--theme` palette mounted.
//!
//! Run:  `playground --scenario gallery --assert --out playground.png`
//!       `playground --all --out-dir shots/`

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, A11yNodeDump, A11yTreeDump, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::signal::create_signal;
use schnellui::testing::{assert_value_contains, find_by_role_name};
use schnellui::theme;
use schnellui::view;
use schnellui::widgets::{Theme, View};
use schnellui::{App, State};
use strum::IntoEnumIterator;

/// Page padding around every scenario — the playground's outer gutter (SOUL §8.1).
const PAGE_PADDING: f32 = 20.0;
/// Size of each dialog preview canvas on the Dialogs page.
const DIALOG_CANVAS_W: f32 = 290.0;
const DIALOG_CANVAS_H: f32 = 250.0;
/// Side padding of a page-switcher tab, the same in every design.
const NAV_TAB_PAD_H: f32 = 8.0;

/// The selectable design systems (SOUL §8.1 [`Theme`]) — the chrome dropdown's
/// option set and the `--theme` CLI value. One row per built-in palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum ThemeChoice {
    #[default]
    Light,
    Dark,
    Forest,
    /// neo-brutalist: squared corners, ink frames, hard block shadows, 1.6× density
    Brutal,
    /// candy/bubble: everything saturates into pills, pastel pink, 1.35× density
    Candy,
}

impl ThemeChoice {
    /// The label on the chrome dropdown's option (and the trigger's value).
    fn title(self) -> &'static str {
        match self {
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Forest => "Forest",
            ThemeChoice::Brutal => "Brutal",
            ThemeChoice::Candy => "Candy",
        }
    }

    /// The design-system tokens this choice mounts with.
    fn theme(self) -> Theme {
        match self {
            ThemeChoice::Light => theme::LIGHT,
            ThemeChoice::Dark => theme::DARK,
            ThemeChoice::Forest => theme::FOREST,
            ThemeChoice::Brutal => theme::BRUTAL,
            ThemeChoice::Candy => theme::CANDY,
        }
    }
}

/// The enumerable scenario table (SOUL §7.1 — `clap::ValueEnum` + `strum::EnumIter`,
/// so `--scenario` is validated and `--list`/`--all` can introspect the set).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// every component on one screen — reached by **construction** (SOUL §7.5).
    Gallery,
    /// the button's reactive loop: three driven `Click`s, a `Status` shows "Clicked 3 times".
    CounterClicked,
    /// the tab bar's exclusivity: the "Privacy" tab driven selected by Role+name.
    TabsSecondSelected,
    /// the list's single-selection: the "Archive" item driven selected.
    ListSecondSelected,
    /// the dropdown's reactive loop: the open "Fruit" dropdown's "Banana" option
    /// driven selected — exclusivity + the trigger's accessible value follow.
    DropdownSecondSelected,
    /// the table's row selection: a `Click` at the "Grace Hopper" **cell** bubbles
    /// to its row (SOUL §6.3); the oracle checks the row's SELECTED state + index.
    TableSecondRowSelected,
    /// typography: sizes, word-wrap, and ellipsis truncation (SOUL §8.1).
    TextStyles,
    /// images: a rasterized bitmap (`Image::from_rgba`) and CPU-rasterized vector
    /// graphics (`Svg`, the SVG subset) drawn from the shared image atlas (§3.2).
    Images,
    /// dialogs: embedded decorated and undecorated modeless panels, with wrapped copy.
    Dialogs,
}

impl Scenario {
    /// Its stable snake_case name (matches `--list` output).
    fn name(self) -> &'static str {
        match self {
            Scenario::Gallery => "gallery",
            Scenario::CounterClicked => "counter_clicked",
            Scenario::TabsSecondSelected => "tabs_second_selected",
            Scenario::ListSecondSelected => "list_second_selected",
            Scenario::DropdownSecondSelected => "dropdown_second_selected",
            Scenario::TableSecondRowSelected => "table_second_row_selected",
            Scenario::TextStyles => "text_styles",
            Scenario::Images => "images",
            Scenario::Dialogs => "dialogs",
        }
    }

    /// Its short human title — the label on the example-switcher tab.
    fn title(self) -> &'static str {
        match self {
            Scenario::Gallery => "Gallery",
            Scenario::CounterClicked => "Button",
            Scenario::TabsSecondSelected => "Tabs",
            Scenario::ListSecondSelected => "List",
            Scenario::DropdownSecondSelected => "Dropdown",
            Scenario::TableSecondRowSelected => "Table",
            Scenario::TextStyles => "Text",
            Scenario::Images => "Images",
            Scenario::Dialogs => "Dialogs",
        }
    }
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "playground",
    about = "schnellui one-shot screenshotter (SOUL §7.1)"
)]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 640)]
    width: u32,
    #[arg(long, default_value_t = 700)]
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
    /// opt-in **windowed** (non-headless) mode (SOUL §8): open a real window with
    /// the chosen scenario instead of writing a PNG.
    #[arg(long)]
    windowed: bool,
    /// Run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
    /// the design system to mount with (SOUL §8.1 [`Theme`]); in windowed mode
    /// the chrome's Theme dropdown changes it on the fly.
    #[arg(long, value_enum, default_value_t)]
    theme: ThemeChoice,
}

// ---------------------------------------------------------------------------
// the scenario views (SOUL §3.3 — each is a setup function, run once)
// ---------------------------------------------------------------------------

/// Every component on one screen. Static skeleton throughout, except the one
/// `Status` slot bound to the demo button's click count (SOUL §3.3). Each
/// toggle has a visible label beside it and the same text as its accessible
/// name. The Density dropdown opens and picks in windowed mode (`live`) by
/// parking a remount, like the Fruit dropdown; headless it stays closed.
fn gallery_view(runtime: PlaygroundRuntime, live: bool, theme: ThemeChoice) -> impl View {
    use schnellui::layout::{Align, EdgeInsets};
    use schnellui::widgets::{
        BadgeTone, Column, Dropdown, DropdownOption, Image, LoadingSpinner, Pad, ProgressBar, Row,
        Slider, Text, TextInput,
    };
    let page = runtime.page();
    let clicks = create_signal(page.gallery_clicks);
    let project = runtime.project();
    let project_runtime = runtime.clone();
    let click_runtime = runtime.clone();
    let email_runtime = runtime.clone();
    let sync_runtime = runtime.clone();
    let daily_runtime = runtime.clone();
    let weekly_runtime = runtime.clone();
    let head = view! {
        column(gap = 12.0) {
            text(size = 22.0, bold) { "schnellui playground" }
            // actions: button (live + disabled), link, badge, and the live
            // click counter the button drives (SOUL §3.3)
            row(gap = 10.0, align = center, wrap) {
                button(on:click = move || {
                    clicks.update(|v| *v += 1);
                    click_runtime.edit(|p| p.gallery_clicks = clicks.get());
                }) { "Increment" }
                button(disabled = true) { "Disabled" }
                link(on:click = move || {}) { "Docs" }
                badge(tone = BadgeTone::Accent) { "NEW" }
                text(role = Role::Status) { (click_count(clicks.get())) }
            }
            // toggles, each with its own label, centered on the control
            row(gap = 8.0, align = center) {
                checkbox(
                    checked = page.email,
                    name = "Email updates",
                    on:toggle = move |on: bool| email_runtime.edit(|p| p.email = on)
                )
                text { "Email updates" }
                flex(basis = 6.0)
                switch(
                    on = page.sync,
                    name = "Sync",
                    on:toggle = move |on: bool| sync_runtime.edit(|p| p.sync = on)
                )
                text { "Sync" }
            }
            // an exclusive radio pair: one Tab stop, arrows move the choice
            row(gap = 8.0, align = center) {
                radio(
                    selected = !page.weekly,
                    name = "Daily",
                    on:select = move || daily_runtime.edit(|p| p.weekly = false)
                )
                text { "Daily" }
                flex(basis = 6.0)
                radio(
                    selected = page.weekly,
                    name = "Weekly",
                    on:select = move || weekly_runtime.edit(|p| p.weekly = true)
                )
                text { "Weekly" }
            }
        }
    };
    // ranges: a stepped input with its value, a determinate status on a thicker
    // bar (so it does not read as a slider without a thumb), and an
    // indeterminate status
    let volume = create_signal(page.volume);
    let volume_runtime = runtime.clone();
    let owner_runtime = runtime.clone();
    let ranges = Row::new()
        .gap(10.0)
        .wrap()
        .align(Align::Center)
        .child(Text::new("Volume"))
        .child(
            Slider::new(volume.get(), 0.0, 100.0)
                .step(5.0)
                .name("Volume")
                .on_change(move |v| {
                    volume.set(v);
                    volume_runtime.edit(|p| p.volume = v);
                }),
        )
        .child(Text::dynamic(move || format!("{:.0}", volume.get())).size(14.0))
        .child(
            Pad::insets(EdgeInsets {
                left: 14.0,
                ..EdgeInsets::default()
            })
            .child(Text::new("Upload 60%")),
        )
        .child(
            ProgressBar::new(60.0, 0.0, 100.0)
                .name("Upload")
                .size(140.0, 10.0),
        )
        .child(
            Pad::insets(EdgeInsets {
                left: 14.0,
                ..EdgeInsets::default()
            })
            .child(LoadingSpinner::new().size(20.0).name("Syncing")),
        )
        .child(Text::new("Syncing"));
    // text entry + an image that has no pixels yet, as tall as the fields
    let entry = Row::new()
        .gap(8.0)
        .wrap()
        .align(Align::Center)
        .child(
            TextInput::new(project)
                .label("Project name")
                .on_input(move |value: &str| project_runtime.set_project(value)),
        )
        .child(
            TextInput::new(page.owner.clone())
                .label("Owner")
                .on_input(move |value: &str| owner_runtime.edit(|p| p.owner = value.to_string())),
        )
        .child(Image::new("photo").alt("Project photo").size(52.0, 52.0));
    let tab_runtimes: [PlaygroundRuntime; 3] = std::array::from_fn(|_| runtime.clone());
    let [tab_a, tab_b, tab_c] = tab_runtimes;
    let list_a = runtime.clone();
    let list_b = runtime.clone();
    let row_runtime = runtime.clone();
    let selection = view! {
        column(gap = 12.0) {
            divider
            // selection: tabs + list beside the table (header + two data
            // rows, first row pre-selected)
            row(gap = 40.0, wrap) {
                column(gap = 12.0) {
                    tabs(gap = 2.0) {
                        tab(selected = page.gallery_tab == "General", on:select = move || tab_a.edit(|p| p.gallery_tab = "General")) { "General" }
                        tab(selected = page.gallery_tab == "Privacy", on:select = move || tab_b.edit(|p| p.gallery_tab = "Privacy")) { "Privacy" }
                        tab(selected = page.gallery_tab == "About", on:select = move || tab_c.edit(|p| p.gallery_tab = "About")) { "About" }
                    }
                    column(width = 200.0, align = stretch) {
                        list {
                            list_item(selected = page.gallery_list == "Inbox", on:select = move || list_a.edit(|p| p.gallery_list = "Inbox")) { "Inbox" }
                            list_item(selected = page.gallery_list == "Archive", on:select = move || list_b.edit(|p| p.gallery_list = "Archive")) { "Archive" }
                        }
                    }
                }
                table(selected_row = page.gallery_row, on:select_row = move |i: usize| row_runtime.edit(|p| p.gallery_row = i)) {
                    table_row(header) { "Name" "Age" "City" }
                    table_row { "Ada Lovelace" "36" "London" }
                    table_row { "Grace Hopper" "85" "Arlington" }
                }
            }
        }
    };
    let inner = Column::new()
        .gap(12.0)
        .child(head)
        .child(ranges)
        .child(entry)
        .child(selection);
    // the Density dropdown, labelled, live in windowed mode
    let density = runtime.density();
    let mut dropdown = Dropdown::new("Density").open(density.open);
    for label in ["Compact", "Comfortable", "Spacious"] {
        let mut option = DropdownOption::new(label).selected(label == density.chosen);
        if live {
            let option_runtime = runtime.clone();
            option = option.on_select(move || {
                option_runtime.set_density(DensityState {
                    open: false,
                    chosen: label,
                });
                option_runtime.request_mount(MountState {
                    scenario: Scenario::Gallery,
                    theme,
                    theme_open: false,
                });
            });
        }
        dropdown = dropdown.option(option);
    }
    if live {
        let toggle_runtime = runtime.clone();
        dropdown = dropdown.on_toggle(move || {
            toggle_runtime.set_density(DensityState {
                open: !density.open,
                ..density
            });
            toggle_runtime.request_mount(MountState {
                scenario: Scenario::Gallery,
                theme,
                theme_open: false,
            });
        });
    }
    Column::new().gap(12.0).child(inner).child(
        Row::new()
            .gap(10.0)
            .align(schnellui::layout::Align::Center)
            .child(Text::new("Density"))
            .child(dropdown),
    )
}

/// The click counter's status line, in words.
fn click_count(clicks: i32) -> String {
    match clicks {
        0 => "Not clicked yet".to_string(),
        1 => "Clicked once".to_string(),
        n => format!("Clicked {n} times"),
    }
}

/// A button and the `Status` it drives — the smallest reactive loop — above the
/// other button looks, so the page doubles as a reference for each state.
fn counter_view(runtime: PlaygroundRuntime) -> impl View {
    let clicks = create_signal(runtime.page().counter);
    view! {
        column(gap = 14.0, width = 460.0, fit_parent) {
            text(size = 22.0, bold) { "Button" }
            row(gap = 12.0, align = center) {
                button(on:click = move || {
                    clicks.update(|v| *v += 1);
                    runtime.edit(|p| p.counter = clicks.get());
                }) { "Increment" }
                text(role = Role::Status) { (click_count(clicks.get())) }
            }
            divider
            text(size = 13.0) { "Appearances" }
            row(gap = 10.0, align = center) {
                button(on:click = move || {}) { "Save" }
                button(appearance = schnellui::widgets::ButtonAppearance::Outline, on:click = move || {}) { "Preview" }
                button(appearance = schnellui::widgets::ButtonAppearance::Ghost, on:click = move || {}) { "Cancel" }
                button(disabled = true) { "Publish" }
            }
            text(size = 13.0, wrap = word) {
                "Solid for the main action, Outline for a second choice, Ghost for a quiet one. A disabled button keeps its place but takes no clicks or focus."
            }
        }
    }
}

/// What each tab of the tabs page shows.
fn tab_body(name: &str) -> &'static str {
    match name {
        "Privacy" => "Choose who sees your profile and what we keep in the activity log.",
        "About" => "schnellui playground, built from the same widgets as the gallery.",
        _ => "Your name, language and the theme the app opens with.",
    }
}

/// A tab bar whose `on:select` handlers write the chosen tab's name into a signal
/// the `Status` slot reads (SOUL §3.3, §6.3), with a panel for the chosen tab.
fn tabs_view(runtime: PlaygroundRuntime) -> impl View {
    let page = runtime.page().tabs;
    let chosen = create_signal(String::from(page));
    let pick = move |name: &'static str| {
        let runtime = runtime.clone();
        move || {
            chosen.set(name.to_string());
            runtime.edit(|p| p.tabs = name);
        }
    };
    let (general, privacy, about) = (pick("General"), pick("Privacy"), pick("About"));
    view! {
        column(gap = 10.0, width = 460.0, fit_parent) {
            text(size = 22.0, bold) { "Tabs" }
            tabs(gap = 2.0) {
                tab(selected = page == "General", on:select = general) { "General" }
                tab(selected = page == "Privacy", on:select = privacy) { "Privacy" }
                tab(selected = page == "About", on:select = about) { "About" }
            }
            text(role = Role::Status) { (format!("Showing {}", chosen.get())) }
            divider
            text(size = 15.0, wrap = word) { (tab_body(&chosen.get()).to_string()) }
        }
    }
}

/// A single-selection list mirroring the tabs scenario (SOUL §6.3). The rows
/// stretch to the column, so the selection fill is the same width on every row
/// and matches the gallery's list.
fn list_view(runtime: PlaygroundRuntime) -> impl View {
    let page = runtime.page().list;
    let chosen = create_signal(String::from(page));
    let pick = move |name: &'static str| {
        let runtime = runtime.clone();
        move || {
            chosen.set(name.to_string());
            runtime.edit(|p| p.list = name);
        }
    };
    let (inbox, starred, sent, archive, trash) = (
        pick("Inbox"),
        pick("Starred"),
        pick("Sent"),
        pick("Archive"),
        pick("Trash"),
    );
    view! {
        column(gap = 10.0) {
            text(size = 22.0, bold) { "List" }
            column(width = 240.0, align = stretch) {
                list {
                    list_item(selected = page == "Inbox", on:select = inbox) { "Inbox" }
                    list_item(selected = page == "Starred", on:select = starred) { "Starred" }
                    list_item(selected = page == "Sent", on:select = sent) { "Sent" }
                    list_item(selected = page == "Archive", on:select = archive) { "Archive" }
                    list_item(selected = page == "Trash", on:select = trash) { "Trash" }
                }
            }
            text(role = Role::Status) { (format!("Opened {}", chosen.get())) }
        }
    }
}

/// The Fruit dropdown's live state, carried across the in-window remounts that
/// opening/closing the list requires (SOUL §3.3 — open/closed is structural):
/// [`dropdown_view`] reads it at build time, its handlers write it before
/// parking [`PENDING_MOUNT`]. The default matches the headless shot's mount —
/// list open, "Apple" chosen — and headless never mutates it (SOUL §7.3).
#[derive(Clone, Copy)]
struct FruitState {
    open: bool,
    chosen: &'static str,
}

/// The dropdown's reactive loop: each option's `on:select` writes into a signal
/// the `Status` slot reads (SOUL §6.3), and the trigger's accessible value
/// mirrors the choice without a remount. Open/close is structural (SOUL §3.3),
/// so in windowed mode (`live`) the trigger toggles the list and an option
/// closes it — both by parking a remount in [`PENDING_MOUNT`], which also
/// refreshes the trigger's painted label — with [`FRUIT`] carrying the state
/// into the rebuild. Headless mounts the list open with no toggle handler, so
/// the driven shot stays deterministic (SOUL §7.3). Built with the builder
/// chain — the dropdown has no literal form in `view!` yet.
fn dropdown_view(runtime: PlaygroundRuntime, live: bool, theme: ThemeChoice) -> impl View {
    use schnellui::widgets::{Column, Dropdown, DropdownOption, Row, Text};
    let fruit = runtime.fruit();
    let chosen = create_signal(String::from(fruit.chosen));
    let opt = |label: &'static str| {
        let option_runtime = runtime.clone();
        DropdownOption::new(label)
            .selected(label == fruit.chosen)
            .on_select(move || {
                chosen.set(label.to_string());
                if live {
                    option_runtime.set_fruit(FruitState {
                        open: false,
                        chosen: label,
                    });
                    option_runtime.request_mount(MountState {
                        scenario: Scenario::DropdownSecondSelected,
                        theme,
                        theme_open: false,
                    });
                }
            })
    };
    let mut dropdown = Dropdown::new("Fruit")
        .open(fruit.open)
        .option(opt("Apple"))
        .option(opt("Banana"))
        .option(opt("Cherry"));
    if live {
        dropdown = dropdown.on_toggle(move || {
            runtime.set_fruit(FruitState {
                open: !fruit.open,
                ..fruit
            });
            runtime.request_mount(MountState {
                scenario: Scenario::DropdownSecondSelected,
                theme,
                theme_open: false,
            });
        });
    }
    // The readout sits beside the trigger, so the open list never covers it.
    Column::new()
        .gap(10.0)
        .child(Text::new("Dropdown").size(22.0).bold())
        .child(
            Row::new()
                .gap(16.0)
                .align(schnellui::layout::Align::Center)
                .child(dropdown)
                .child(
                    Text::dynamic(move || format!("You picked {}", chosen.get()))
                        .role(Role::Status),
                ),
        )
}

/// The table page's people, in data-row order.
const PEOPLE: [&str; 3] = ["Ada Lovelace", "Grace Hopper", "Alan Turing"];

/// The table with row selection: `on:select_row` receives the **data-row index**
/// (header rows don't count) and feeds the `Status` slot (SOUL §8.1, §6.3).
fn table_view(runtime: PlaygroundRuntime) -> impl View {
    let row = runtime.page().table;
    let selected = create_signal(row);
    view! {
        column(gap = 10.0) {
            text(size = 22.0, bold) { "Table" }
            table(selected_row = row, on:select_row = move |i: usize| {
                selected.set(i);
                runtime.edit(|p| p.table = i);
            }) {
                table_row(header) { "Name" "Age" "City" }
                table_row { "Ada Lovelace" "36" "London" }
                table_row { "Grace Hopper" "85" "Arlington" }
                table_row { "Alan Turing" "41" "London" }
            }
            text(role = Role::Status) { (format!("Selected: {}", PEOPLE[selected.get()])) }
        }
    }
}

/// Typography: sizes, a wrapping paragraph (width-aware, so the column fixes its
/// width), and single-line ellipsis truncation. Note the a11y point: a truncated
/// text's accessible **name stays the full string** — truncation is visual only.
fn text_view() -> impl View {
    view! {
        column(gap = 10.0, width = 460.0, fit_parent) {
            text(size = 22.0, bold) { "Typography" }
            text(size = 16.0) { "regular 16px" }
            text(size = 12.0) { "small 12px" }
            divider
            text(wrap = word) { "A longer paragraph that wraps onto multiple lines: wrapped text is measured width-aware through the layout pass, so its height tracks the number of lines it actually needs." }
            text(ellipsis) { "A single line that truncates with an ellipsis when it cannot fit the available width" }
        }
    }
}

/// Images (SOUL §8.1, §3.2): a **rasterized** bitmap generated deterministically in
/// code (`Image::from_rgba` — no file I/O, SOUL §7.3) beside two **vector** images
/// (`Svg`, the SVG subset) — all three land in the scene's shared RGBA atlas and
/// draw as real `ImageQuad`s. Built with the builder API: pixel data has no literal
/// form in `view!` (the `svg` tag exists there; see the macro tests).
fn images_view() -> impl View {
    use schnellui::widgets::{Column, Image, Row, Svg, Text, View, WrapMode};
    // a 64×64 gradient under an 8px checker — recognizable, deterministic
    let mut px = Vec::with_capacity(64 * 64 * 4);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let checker = (x / 8 + y / 8) % 2 == 0;
            let b = if checker { 0x40 } else { 0xff };
            px.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, b, 0xff]);
        }
    }
    // gradient-filled ring (radial), plus a stroked outline — circles take
    // strokes now that all geometry flattens to contours
    const LOGO: &str = r##"<svg viewBox="0 0 24 24">
        <defs>
          <radialGradient id="glow">
            <stop offset="0" stop-color="#ffffff"/>
            <stop offset="1" stop-color="#3366cc"/>
          </radialGradient>
        </defs>
        <circle cx="12" cy="12" r="11" fill="url(#glow)"/>
        <circle cx="12" cy="12" r="11" fill="none" stroke="#1a3d80" stroke-width="1.5"/>
        <path d="M8 12 L11 15 L16 8" fill="none" stroke="#228844" stroke-width="2"/>
    </svg>"##;
    // cubic Béziers + arcs, a linear gradient, and a rotated group transform
    const HEART: &str = r##"<svg viewBox="0 0 24 24">
        <defs>
          <linearGradient id="warm" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="#ff6666"/>
            <stop offset="1" stop-color="#aa1122"/>
          </linearGradient>
        </defs>
        <g transform="rotate(-8,12,12)">
          <path fill="url(#warm)"
                d="M12 21 C 5 15, 2 11, 2 7.5 A 4.5 4.5 0 0 1 12 5.5 A 4.5 4.5 0 0 1 22 7.5 C 22 11, 19 15, 12 21 Z"/>
        </g>
    </svg>"##;
    // sparkline + an evenodd donut hole + svg <text> through the real shaper
    const SPARK: &str = r##"<svg viewBox="0 0 64 24">
        <line x1="2" y1="21" x2="62" y2="21" stroke="gray"/>
        <polyline points="2,18 14,10 26,13 38,5 50,9 62,3" fill="none" stroke="#cc3333" stroke-width="1.5"/>
        <path fill-rule="evenodd" fill="#3366cc" d="M8 3 h10 v10 h-10 Z M11 6 h4 v4 h-4 Z"/>
        <text x="62" y="17" font-size="9" text-anchor="end" fill="black">42k</text>
    </svg>"##;
    let figure = |image: Box<dyn View>, caption: &'static str| {
        Column::new()
            .gap(6.0)
            .width(136.0)
            .child(image)
            .child(Text::new(caption).size(13.0).wrap(WrapMode::Word))
    };
    Column::new()
        .gap(14.0)
        .child(Text::new("Images").size(22.0).bold())
        .child(
            Text::new("One bitmap and three vector drawings, all drawn from the same atlas.")
                .wrap(WrapMode::Word),
        )
        .child(
            Row::new()
                .gap(16.0)
                .wrap()
                .align(schnellui::layout::Align::Start)
                .child(figure(
                    Box::new(Image::from_rgba(64, 64, px).alt("gradient checker")),
                    "Bitmap generated in code",
                ))
                .child(figure(
                    Box::new(Svg::new(LOGO).alt("schnellui logo").size(64.0, 64.0)),
                    "Radial gradient and stroke",
                ))
                .child(figure(
                    Box::new(Svg::new(HEART).alt("heart").size(64.0, 64.0)),
                    "Curves, arcs and a rotation",
                ))
                .child(figure(
                    Box::new(Svg::new(SPARK).alt("sparkline").size(128.0, 48.0)),
                    "Polyline, even-odd fill and SVG text",
                )),
        )
}

/// Dialog chrome, compared without covering the playground controls. Each
/// dialog is deliberately parent-scoped and modeless inside its own fixed-size
/// preview canvas; the standalone `dialogs` example exercises viewport-fixed,
/// modal, positioned, and persistent behavior at full size.
fn dialogs_view() -> impl View {
    use schnellui::widgets::{
        Button, ButtonAppearance, Column, Dialog, Image, Row, Stack, Text, WrapMode,
    };

    let decorated = Stack::new()
        .size(DIALOG_CANVAS_W, DIALOG_CANVAS_H)
        .child(
            Image::new("decorated dialog preview")
                .alt("decorated dialog preview canvas")
                .size(DIALOG_CANVAS_W, DIALOG_CANVAS_H),
        )
        .child(
            Dialog::new("Review changes")
                .modeless()
                .non_fixed()
                .width(DIALOG_CANVAS_W - 28.0)
                .viewport_inset(12.0)
                .padding(14.0)
                .gap(9.0)
                .child(
                    Text::new("You have 3 unsaved edits in the design file. Review them before you share it.").wrap(WrapMode::Word),
                )
                .child(
                    Row::new()
                        .gap(8.0)
                        .justify(schnellui::layout::Justify::End)
                        .child(Button::new("Cancel").appearance(ButtonAppearance::Outline))
                        .child(Button::new("Review")),
                ),
        );

    let undecorated = Stack::new()
        .size(DIALOG_CANVAS_W, DIALOG_CANVAS_H)
        .child(
            Image::new("undecorated dialog preview")
                .alt("undecorated dialog preview canvas")
                .size(DIALOG_CANVAS_W, DIALOG_CANVAS_H),
        )
        .child(
            Dialog::new("Undecorated dialog")
                .undecorated()
                .modeless()
                .non_fixed()
                .width(DIALOG_CANVAS_W - 28.0)
                .viewport_inset(12.0)
                .padding(14.0)
                .gap(9.0)
                .child(Text::new("Saved").size(18.0).bold())
                .child(
                    Text::new(
                        "Your changes are saved. Invite your team to see the latest version.",
                    )
                    .wrap(WrapMode::Word),
                )
                .child(
                    Row::new()
                        .gap(8.0)
                        .justify(schnellui::layout::Justify::End)
                        .child(Button::new("Later").appearance(ButtonAppearance::Outline))
                        .child(Button::new("Invite")),
                ),
        );

    Column::new()
        .width(600.0)
        .fit_parent()
        .gap(10.0)
        .child(Text::new("Dialogs").size(22.0).bold())
        .child(
            Text::new("The same dialog with and without its title bar and frame.")
                .wrap(WrapMode::Word),
        )
        .child(
            Row::new()
                .width(600.0)
                .fit_parent()
                .wrap()
                .gap(12.0)
                .child(
                    Column::new()
                        .gap(6.0)
                        .child(decorated)
                        .child(Text::new("Decorated: title bar, frame and shadow").size(13.0)),
                )
                .child(
                    Column::new().gap(6.0).child(undecorated).child(
                        Text::new("Undecorated: the content draws its own header").size(13.0),
                    ),
                ),
        )
        .child(
            Text::new(
                "Dialogs can also be modal, fixed to the viewport, or persistent. \
                 The dialogs example shows each at full size.",
            )
            .size(13.0)
            .wrap(WrapMode::Word),
        )
}

// ---------------------------------------------------------------------------
// the page chrome (padding + the example-switcher tab bar)
// ---------------------------------------------------------------------------

/// The playground state a live chrome control targets: which scenario is on
/// stage, which design system it wears, and whether the chrome's Theme dropdown
/// is showing its options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MountState {
    scenario: Scenario,
    theme: ThemeChoice,
    theme_open: bool,
}

#[derive(Clone)]
struct PlaygroundRuntime(State<PlaygroundRuntimeState>);

struct PlaygroundRuntimeState {
    fruit: FruitState,
    density: DensityState,
    /// The gallery's Project name, kept across the remounts a dropdown makes.
    project: String,
    /// The scenario on stage, so a remount can tell a switch from a rebuild.
    current: Option<Scenario>,
    /// What the user did on the scenario pages, kept across remounts.
    page: PageState,
    pending_mount: Option<MountState>,
}

/// The interactive state of every scenario page. The views read it at build
/// time and their handlers write it back, so a remount rebuilds the page as the
/// user left it. Entering a scenario resets its fields to the defaults.
#[derive(Clone, PartialEq)]
struct PageState {
    gallery_clicks: i32,
    email: bool,
    sync: bool,
    weekly: bool,
    volume: f32,
    owner: String,
    gallery_tab: &'static str,
    gallery_list: &'static str,
    gallery_row: usize,
    counter: i32,
    tabs: &'static str,
    list: &'static str,
    table: usize,
}

impl Default for PageState {
    fn default() -> Self {
        Self {
            gallery_clicks: 0,
            email: true,
            sync: true,
            weekly: false,
            volume: 40.0,
            owner: "Ada".to_string(),
            gallery_tab: "General",
            gallery_list: "Inbox",
            gallery_row: 0,
            counter: 0,
            tabs: "General",
            list: "Inbox",
            table: 0,
        }
    }
}

/// The gallery's Density dropdown, carried across its open/close remounts.
#[derive(Clone, Copy)]
struct DensityState {
    open: bool,
    chosen: &'static str,
}

impl Default for PlaygroundRuntime {
    fn default() -> Self {
        Self(State::new(PlaygroundRuntimeState {
            fruit: FruitState {
                open: true,
                chosen: "Apple",
            },
            density: DensityState {
                open: false,
                chosen: "Comfortable",
            },
            project: String::new(),
            current: None,
            page: PageState::default(),
            pending_mount: None,
        }))
    }
}

impl PlaygroundRuntime {
    fn fruit(&self) -> FruitState {
        self.0.read(|state| state.fruit)
    }

    fn set_fruit(&self, fruit: FruitState) {
        self.0.update(|state| state.fruit = fruit);
    }

    fn project(&self) -> String {
        self.0.read(|state| state.project.clone())
    }

    fn set_project(&self, value: &str) {
        self.0.update(|state| {
            state.project.clear();
            state.project.push_str(value);
        });
    }

    fn density(&self) -> DensityState {
        self.0.read(|state| state.density)
    }

    fn set_density(&self, density: DensityState) {
        self.0.update(|state| state.density = density);
    }

    fn page(&self) -> PageState {
        self.0.read(|state| state.page.clone())
    }

    fn edit(&self, change: impl FnOnce(&mut PageState)) {
        self.0.update(|state| change(&mut state.page));
    }

    /// Marks `scenario` as the one on stage. Returns whether it was entered
    /// (it was not already on stage), resetting its page state to the defaults.
    fn enter(&self, scenario: Scenario) -> bool {
        self.0.update(|state| {
            if state.current == Some(scenario) {
                return false;
            }
            state.current = Some(scenario);
            let fresh = PageState::default();
            let page = &mut state.page;
            match scenario {
                Scenario::Gallery => {
                    page.gallery_clicks = fresh.gallery_clicks;
                    page.email = fresh.email;
                    page.sync = fresh.sync;
                    page.weekly = fresh.weekly;
                    page.volume = fresh.volume;
                    page.owner = fresh.owner;
                    page.gallery_tab = fresh.gallery_tab;
                    page.gallery_list = fresh.gallery_list;
                    page.gallery_row = fresh.gallery_row;
                }
                Scenario::CounterClicked => page.counter = fresh.counter,
                Scenario::TabsSecondSelected => page.tabs = fresh.tabs,
                Scenario::ListSecondSelected => page.list = fresh.list,
                Scenario::TableSecondRowSelected => page.table = fresh.table,
                _ => {}
            }
            true
        })
    }

    fn request_mount(&self, mount: MountState) {
        self.0.update(|state| state.pending_mount = Some(mount));
    }

    fn take_mount(&self) -> Option<MountState> {
        self.0.update(|state| state.pending_mount.take())
    }
}

/// Wraps a scenario's content in the shared page chrome: the example-switcher tab
/// bar, then the **Theme dropdown** — the design-system switcher, its trigger
/// announcing the current palette, its floating option list offering the others —
/// then a divider, everything inside the page padding (SOUL §8.1). The tab for
/// `current` and the option for `theme` render selected. In windowed mode
/// (`live_switch`) the controls park the target [`MountState`] in
/// [`PENDING_MOUNT`] for the in-window remount: a tab switches the scenario, the
/// dropdown trigger opens/closes the option list, and an option remounts the
/// same scenario **re-themed** — the whole design system changes on the fly
/// without the window closing. On the headless PNG path the handlers never fire
/// and the shot stays deterministic (SOUL §7.3).
fn stage(
    runtime: PlaygroundRuntime,
    current: Scenario,
    live_switch: bool,
    theme: ThemeChoice,
    theme_open: bool,
    content: impl View,
) -> impl View {
    use schnellui::widgets::{
        Column, Divider, Dropdown, DropdownOption, Pad, Row, Scroll, Tab, TabAppearance, TabBar,
        Text,
    };
    // `wrap()`: the nine page tabs fit one line at the default 640px width;
    // a narrower window flows them onto a second line inside the padding.
    let mut bar = TabBar::new().gap(2.0).wrap();
    for s in Scenario::iter() {
        // The page switcher wears underline tabs, so it reads as navigation
        // apart from the filled tab bars the scenarios show off.
        // A fixed side padding keeps the nine pages on one line in the roomy
        // designs too; only the page body takes the design's density.
        let mut tab = Tab::new(s.title())
            .appearance(TabAppearance::Underline)
            .padding_h(NAV_TAB_PAD_H)
            .selected(s == current);
        if s != current && live_switch {
            let tab_runtime = runtime.clone();
            tab = tab.on_select(move || {
                tab_runtime.request_mount(MountState {
                    scenario: s,
                    theme,
                    theme_open: false,
                })
            });
        }
        bar = bar.child(tab);
    }
    let mut switcher = Dropdown::new("Theme").open(theme_open);
    if live_switch {
        let toggle_runtime = runtime.clone();
        switcher = switcher.on_toggle(move || {
            toggle_runtime.request_mount(MountState {
                scenario: current,
                theme,
                theme_open: !theme_open,
            })
        });
    }
    for t in ThemeChoice::iter() {
        let mut opt = DropdownOption::new(t.title()).selected(t == theme);
        if live_switch {
            let option_runtime = runtime.clone();
            opt = opt.on_select(move || {
                option_runtime.request_mount(MountState {
                    scenario: current,
                    theme: t,
                    theme_open: false,
                })
            });
        }
        switcher = switcher.option(opt);
    }
    // The page scrolls when a design's density makes it taller than the
    // window, with a visible bar so nothing hides below the fold.
    // `fit_width` gives the page the window's width, so rows wrap and prose
    // reflows at it instead of running off the right edge.
    Scroll::new().fill().fit_width().scrollbar(true).child(
        Pad::all(PAGE_PADDING).child(
            Column::new()
                .gap(12.0)
                .align(schnellui::layout::Align::Stretch)
                .child(bar)
                .child(
                    Row::new()
                        .gap(10.0)
                        .align(schnellui::layout::Align::Center)
                        .child(Text::new("Theme"))
                        .child(switcher),
                )
                .child(Divider::new())
                .child(content),
        ),
    )
}

// ---------------------------------------------------------------------------
// build + drive (SOUL §7.5 — construct, or reach the state through ActionRequests)
// ---------------------------------------------------------------------------

/// Drives one `Click` `ActionRequest` at the widget located by Role (+ name) —
/// semantic query, never pixels (SOUL §7.5). The router sends it down the same
/// handler a pointer click reaches (SOUL §6.3).
fn click(app: &mut App, role: Role, name: Option<&str>) {
    let Some(id) = app.find_widget(role, name) else {
        eprintln!("drive: no {role:?} named {name:?}");
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

/// Builds the app for a scenario in its target state (SOUL §7.5), inside the
/// shared page chrome ([`stage`]). `Gallery` and `TextStyles` are constructed;
/// the interactive scenarios are *driven* there through the real inbound
/// `ActionRequest` path — proving each state is reachable, not merely
/// constructible. `state` names the design system to mount with and whether the
/// chrome's Theme dropdown shows its option list (only ever open on a windowed
/// remount — headless shots take the closed, deterministic chrome, SOUL §7.3).
fn scenario_app(runtime: &PlaygroundRuntime, state: MountState, cli: &Cli) -> App {
    let scenario = state.scenario;
    // A scenario is entered on its first mount, a switcher-tab change, and every
    // headless run. It then starts from its defaults and is driven to its target
    // below. A remount of the scenario already on stage (theme open/close, theme
    // pick, Density or Fruit toggles) keeps what the user did and drives nothing:
    // a scripted click there would land outside an open list and close it again.
    let entered = runtime.enter(scenario);
    let (width, height, scale) = (cli.width, cli.height, cli.scale);
    // The ambient design system is read at build time (SOUL §8.1), so it is set
    // BEFORE the mount below; the remount hook re-enters here on a theme switch.
    let theme = state.theme.theme();
    // Only a windowed run gets live switcher chrome; headless shots stay inert.
    let (sw, t, so) = (
        cli.windowed || cli.headless_windowed,
        state.theme,
        state.theme_open,
    );
    let mut app = match scenario {
        Scenario::Gallery => App::mount_with_theme_size_scaled(
            theme,
            stage(
                runtime.clone(),
                scenario,
                sw,
                t,
                so,
                gallery_view(runtime.clone(), sw, t),
            ),
            width,
            height,
            scale,
        ),
        Scenario::CounterClicked => App::mount_with_theme_size_scaled(
            theme,
            stage(
                runtime.clone(),
                scenario,
                sw,
                t,
                so,
                counter_view(runtime.clone()),
            ),
            width,
            height,
            scale,
        ),
        Scenario::TabsSecondSelected => App::mount_with_theme_size_scaled(
            theme,
            stage(
                runtime.clone(),
                scenario,
                sw,
                t,
                so,
                tabs_view(runtime.clone()),
            ),
            width,
            height,
            scale,
        ),
        Scenario::ListSecondSelected => App::mount_with_theme_size_scaled(
            theme,
            stage(
                runtime.clone(),
                scenario,
                sw,
                t,
                so,
                list_view(runtime.clone()),
            ),
            width,
            height,
            scale,
        ),
        Scenario::DropdownSecondSelected => App::mount_with_theme_size_scaled(
            theme,
            stage(
                runtime.clone(),
                scenario,
                sw,
                t,
                so,
                dropdown_view(runtime.clone(), sw, t),
            ),
            width,
            height,
            scale,
        ),
        Scenario::TableSecondRowSelected => App::mount_with_theme_size_scaled(
            theme,
            stage(
                runtime.clone(),
                scenario,
                sw,
                t,
                so,
                table_view(runtime.clone()),
            ),
            width,
            height,
            scale,
        ),
        Scenario::TextStyles => App::mount_with_theme_size_scaled(
            theme,
            stage(runtime.clone(), scenario, sw, t, so, text_view()),
            width,
            height,
            scale,
        ),
        Scenario::Images => App::mount_with_theme_size_scaled(
            theme,
            stage(runtime.clone(), scenario, sw, t, so, images_view()),
            width,
            height,
            scale,
        ),
        Scenario::Dialogs => App::mount_with_theme_size_scaled(
            theme,
            stage(runtime.clone(), scenario, sw, t, so, dialogs_view()),
            width,
            height,
            scale,
        ),
    };
    // The page background is a design token too (SOUL §8.1): each theme carries
    // the page colour that keeps its widget surfaces distinct in the PNG (§7.3).
    app.set_clear_color(theme.page);

    match scenario {
        _ if !entered => {}
        Scenario::CounterClicked => {
            for _ in 0..3 {
                click(&mut app, Role::Button, Some("Increment"));
            }
        }
        Scenario::TabsSecondSelected => click(&mut app, Role::Tab, Some("Privacy")),
        Scenario::ListSecondSelected => click(&mut app, Role::ListItem, Some("Archive")),
        // The chrome's switcher dropdown is closed (no option nodes), so the
        // content dropdown's "Banana" is the only ListBoxOption by that name.
        // Headless only: windowed, this dropdown is live — a scripted select
        // would park a close-the-list remount, which re-enters here and fights
        // every open the user clicks for.
        Scenario::DropdownSecondSelected if !(cli.windowed || cli.headless_windowed) => {
            click(&mut app, Role::ListBoxOption, Some("Banana"))
        }
        // Aim at the CELL: the click bubbles to its row (SOUL §6.3) — the same
        // convergence a pointer hit-test produces.
        Scenario::TableSecondRowSelected => click(&mut app, Role::Cell, Some("Grace Hopper")),
        _ => {}
    }
    app
}

// ---------------------------------------------------------------------------
// the oracle (SOUL §7.5 — assert on the a11y tree, pixels are secondary)
// ---------------------------------------------------------------------------

/// Collects every dump node with the given role, in tree order.
fn all_with_role<'a>(tree: &'a A11yTreeDump, role: &str) -> Vec<&'a A11yNodeDump> {
    fn walk<'a>(n: &'a A11yNodeDump, role: &str, out: &mut Vec<&'a A11yNodeDump>) {
        if n.role == role {
            out.push(n);
        }
        for c in &n.children {
            walk(c, role, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = &tree.root {
        walk(root, role, &mut out);
    }
    out
}

/// `true` if the node's state list carries `flag`.
fn has_state(node: &A11yNodeDump, flag: &str) -> bool {
    node.state.iter().any(|s| s == flag)
}

fn run_assertions(scenario: Scenario, theme: ThemeChoice, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    // The page chrome is on every scenario: one switcher tab per example, and
    // exactly the current example's tab carries SELECTED (SOUL §6.1). The chrome
    // bar precedes the content in tree order, so the title lookup cannot land on
    // a content tab.
    for s in Scenario::iter() {
        let tab = find_by_role_name(&tree, "tab", Some(s.title()))
            .ok_or_else(|| format!("chrome: no switcher tab titled {:?}", s.title()))?;
        if has_state(tab, "selected") != (s == scenario) {
            return Err(format!(
                "chrome: switcher tab {:?} selected={}, want {}",
                s.title(),
                has_state(tab, "selected"),
                s == scenario
            ));
        }
    }
    // The chrome's Theme dropdown is on every scenario too: closed on the
    // headless path (no option nodes — deterministic, SOUL §7.3), its trigger
    // announcing the mounted design system as the accessible value (SOUL §6.1).
    let switcher = find_by_role_name(&tree, "combo_box", Some("Theme"))
        .ok_or("chrome: no design-system combo box named \"Theme\"")?;
    if switcher.value.as_deref() != Some(theme.title()) {
        return Err(format!(
            "chrome: Theme dropdown value={:?}, want {:?}",
            switcher.value,
            theme.title()
        ));
    }
    if has_state(switcher, "expanded") {
        return Err("chrome: the Theme dropdown must be closed on a headless shot".into());
    }
    match scenario {
        // The gallery must contain at least one of every component role — the
        // covenant made checkable: no widget without a role (SOUL §6.1).
        Scenario::Gallery => {
            for role in [
                "label",
                "button",
                "link",
                "status",
                "checkbox",
                "switch",
                "radio",
                "slider",
                "progress_indicator",
                "text_input",
                "image",
                "group",
                "tab_list",
                "tab",
                "list",
                "list_item",
                "table",
                "row",
                "cell",
                "column_header",
                "combo_box",
            ] {
                if all_with_role(&tree, role).is_empty() {
                    return Err(format!("gallery: no node with role {role:?}"));
                }
            }
            // the disabled button announces its state (SOUL §6.1)
            let d = find_by_role_name(&tree, "button", Some("Disabled"))
                .ok_or("missing the disabled button")?;
            if !has_state(d, "disabled") {
                return Err("the disabled button does not announce disabled".into());
            }
            // two Status live regions: the badge ("NEW") and the click counter
            let statuses = all_with_role(&tree, "status");
            for needle in ["NEW", "Not clicked yet"] {
                if !statuses
                    .iter()
                    .any(|s| s.value.as_deref().is_some_and(|v| v.contains(needle)))
                {
                    return Err(format!("gallery: no status value contains {needle:?}"));
                }
            }
            // the gallery table announces derived counts (SOUL §6.1)
            let t = all_with_role(&tree, "table");
            let table = t.first().ok_or("missing table")?;
            if table.row_count != Some(3) || table.column_count != Some(3) {
                return Err(format!(
                    "table counts: rows={:?} cols={:?}, want 3×3",
                    table.row_count, table.column_count
                ));
            }
            Ok(())
        }
        // Three driven clicks → the memo'd status shows exactly 3 (SOUL §7.5).
        Scenario::CounterClicked => assert_value_contains(&tree, "status", None, "Clicked 3 times"),
        // Exclusivity: "Privacy" gained SELECTED, "General" lost it, and the
        // on:select handler drove the status text (SOUL §6.3).
        Scenario::TabsSecondSelected => {
            let privacy = find_by_role_name(&tree, "tab", Some("Privacy"))
                .ok_or("missing the Privacy tab")?;
            if !has_state(privacy, "selected") {
                return Err("Privacy tab is not selected".into());
            }
            let general = find_by_role_name(&tree, "tab", Some("General"))
                .ok_or("missing the General tab")?;
            if has_state(general, "selected") {
                return Err("General tab is still selected (exclusivity broken)".into());
            }
            assert_value_contains(&tree, "status", None, "Showing Privacy")
        }
        Scenario::ListSecondSelected => {
            let archive = find_by_role_name(&tree, "list_item", Some("Archive"))
                .ok_or("missing the Archive item")?;
            if !has_state(archive, "selected") {
                return Err("Archive item is not selected".into());
            }
            let inbox = find_by_role_name(&tree, "list_item", Some("Inbox"))
                .ok_or("missing the Inbox item")?;
            if has_state(inbox, "selected") {
                return Err("Inbox item is still selected (exclusivity broken)".into());
            }
            assert_value_contains(&tree, "status", None, "Opened Archive")
        }
        // Exclusivity + the trigger's mirrored value: "Banana" gained SELECTED,
        // "Apple" lost it, the trigger stayed EXPANDED announcing "Banana", and
        // the on:select handler drove the status text (SOUL §6.1, §6.3).
        Scenario::DropdownSecondSelected => {
            let banana = find_by_role_name(&tree, "list_box_option", Some("Banana"))
                .ok_or("missing the Banana option")?;
            if !has_state(banana, "selected") {
                return Err("Banana option is not selected".into());
            }
            let apple = find_by_role_name(&tree, "list_box_option", Some("Apple"))
                .ok_or("missing the Apple option")?;
            if has_state(apple, "selected") {
                return Err("Apple option is still selected (exclusivity broken)".into());
            }
            let trigger = find_by_role_name(&tree, "combo_box", Some("Fruit"))
                .ok_or("missing the Fruit combo box")?;
            if !has_state(trigger, "expanded") {
                return Err("the Fruit dropdown does not announce expanded".into());
            }
            if trigger.value.as_deref() != Some("Banana") {
                return Err(format!(
                    "Fruit trigger value={:?}, want \"Banana\"",
                    trigger.value
                ));
            }
            assert_value_contains(&tree, "status", None, "You picked Banana")
        }
        // The row containing the "Grace Hopper" cell is selected, carries the
        // derived row index 2 (header = 0), the previously-selected row cleared,
        // and the on:select_row handler saw data-row index 1 (SOUL §6.1, §6.3).
        Scenario::TableSecondRowSelected => {
            let rows = all_with_role(&tree, "row");
            let by_cell = |name: &str| {
                rows.iter()
                    .find(|r| r.children.iter().any(|c| c.name.as_deref() == Some(name)))
                    .copied()
            };
            let grace = by_cell("Grace Hopper").ok_or("missing the Grace Hopper row")?;
            if !has_state(grace, "selected") {
                return Err("Grace Hopper's row is not selected".into());
            }
            if grace.row_index != Some(2) {
                return Err(format!("Grace row_index={:?}, want 2", grace.row_index));
            }
            let ada = by_cell("Ada Lovelace").ok_or("missing the Ada Lovelace row")?;
            if has_state(ada, "selected") {
                return Err("Ada's row is still selected (exclusivity broken)".into());
            }
            assert_value_contains(&tree, "status", None, "Selected: Grace Hopper")
        }
        // Truncation is visual only: the ellipsized text's accessible name stays
        // the full string (SOUL §6.1).
        Scenario::TextStyles => {
            let long = "A single line that truncates with an ellipsis when it cannot fit the available width";
            find_by_role_name(&tree, "label", Some(long))
                .ok_or("ellipsized text lost its full accessible name")?;
            Ok(())
        }
        // Raster + vector images each announce their alt as the accessible name
        // (SOUL §6.1) — the pixels themselves are checked by the renderer's own
        // headless round-trip test.
        Scenario::Images => {
            for name in ["gradient checker", "schnellui logo", "heart", "sparkline"] {
                find_by_role_name(&tree, "image", Some(name))
                    .ok_or_else(|| format!("missing image named {name:?}"))?;
            }
            Ok(())
        }
        Scenario::Dialogs => {
            let dialogs = all_with_role(&tree, "dialog");
            if dialogs.len() != 2 {
                return Err(format!(
                    "dialog showcase has {} dialogs, want 2",
                    dialogs.len()
                ));
            }
            if dialogs.iter().any(|dialog| has_state(dialog, "modal")) {
                return Err("embedded playground dialogs must remain modeless".into());
            }
            find_by_role_name(&tree, "label", Some("Review changes"))
                .ok_or("decorated dialog is missing its visible title")?;
            if find_by_role_name(&tree, "label", Some("Undecorated dialog")).is_some() {
                return Err("undecorated dialog leaked a visible title label".into());
            }
            for copy in [
                "You have 3 unsaved edits in the design file. Review them before you share it.",
                "Your changes are saved. Invite your team to see the latest version.",
            ] {
                find_by_role_name(&tree, "label", Some(copy))
                    .ok_or_else(|| format!("missing wrapped dialog copy {copy:?}"))?;
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// the one-shot harness (SOUL §7.1 — identical shape across examples)
// ---------------------------------------------------------------------------

/// Renders one scenario: build → one synchronous frame → (dump) → (assert) → PNG.
fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let runtime = PlaygroundRuntime::default();
    let mut app = scenario_app(
        &runtime,
        MountState {
            scenario,
            theme: cli.theme,
            theme_open: false,
        },
        cli,
    );
    app.frame(); // one synchronous frame settles the dynamic slots (SOUL §7.1)

    if let Some(path) = &cli.dump_a11y {
        if let Err(e) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    if cli.assert {
        if let Err(e) = run_assertions(scenario, cli.theme, &app) {
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
    // The remount hook drains [`PENDING_MOUNT`] after every event: a switcher tab,
    // the Theme dropdown's trigger (open/close), or one of its theme options parks
    // the target [`MountState`] there, and the hook mounts it into the SAME window
    // — scenario switches and whole-design-system swaps, both on the fly (no
    // process relaunch, no window close/reopen).
    if cli.windowed || cli.headless_windowed {
        let runtime = PlaygroundRuntime::default();
        let mut app = scenario_app(
            &runtime,
            MountState {
                scenario,
                theme: cli.theme,
                theme_open: false,
            },
            &cli,
        );
        app.set_escape_exits(true);
        let remount_runtime = runtime.clone();
        let result = app.run_windowed_with("playground", move || {
            remount_runtime
                .take_mount()
                .map(|state| scenario_app(&remount_runtime, state, &cli))
        });
        return match result {
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
    use schnellui::accesskit_action::ActionData;

    /// A live (windowed) CLI, so the chrome and dropdowns park remounts.
    fn live_cli() -> Cli {
        Cli::parse_from(["playground", "--headless-windowed"])
    }

    /// Every Tab stop of every scenario can be shown, at the default size and
    /// in a narrow, short window: the page scroll must bring it in.
    #[test]
    fn every_tab_stop_reaches_the_window() {
        use schnellui::testing::fuzz::tab_stops_reachable;
        let mut stops = 0;
        for scenario in Scenario::iter() {
            for (w, h) in [(640, 700), (640, 240), (640, 120)] {
                let runtime = PlaygroundRuntime::default();
                let cli = Cli::parse_from([
                    "playground",
                    "--headless-windowed",
                    "--width",
                    &w.to_string(),
                    "--height",
                    &h.to_string(),
                ]);
                let mut app = scenario_app(
                    &runtime,
                    MountState {
                        scenario,
                        theme: ThemeChoice::Light,
                        theme_open: false,
                    },
                    &cli,
                );
                stops += tab_stops_reachable(&mut app)
                    .unwrap_or_else(|e| panic!("{scenario:?} at {w}x{h}: {e}"));
            }
        }
        assert!(stops > 0, "no scenario has a Tab stop");
    }

    /// At a phone width no text or control is cut at a side with nothing to
    /// scroll it in: prose wraps and fixed-width content scrolls sideways.
    #[test]
    fn nothing_is_clipped_sideways_at_360() {
        use schnellui::testing::fuzz::sideways_clips;
        let mut faults = Vec::new();
        for scenario in Scenario::iter() {
            let runtime = PlaygroundRuntime::default();
            let cli = Cli::parse_from([
                "playground",
                "--headless-windowed",
                "--width",
                "360",
                "--height",
                "600",
            ]);
            let mut app = scenario_app(
                &runtime,
                MountState {
                    scenario,
                    theme: ThemeChoice::Light,
                    theme_open: false,
                },
                &cli,
            );
            for fault in sideways_clips(&mut app) {
                faults.push(format!("{scenario:?}: {fault}"));
            }
        }
        assert!(faults.is_empty(), "{faults:#?}");
    }

    fn mount(runtime: &PlaygroundRuntime, scenario: Scenario) -> App {
        let mut app = scenario_app(
            runtime,
            MountState {
                scenario,
                theme: ThemeChoice::Light,
                theme_open: false,
            },
            &live_cli(),
        );
        app.frame();
        app
    }

    /// Tab walks the page the way it reads: the page tab strip (one roving
    /// stop) and the Theme dropdown above the body come before any control of
    /// the body.
    #[test]
    fn first_tab_stops_are_the_page_tabs_and_theme() {
        let runtime = PlaygroundRuntime::default();
        for scenario in Scenario::iter() {
            let mut app = mount(&runtime, scenario);
            let mut stops = Vec::new();
            for _ in 0..2 {
                assert!(app.focus_step(false), "{scenario:?}: Tab moved nothing");
                let id = app.focused_widget().expect("a Tab stop is focused");
                let a = app.scene().a11y(id).unwrap();
                stops.push((a.role, a.name.clone().unwrap_or_default()));
            }
            assert_eq!(
                stops[0],
                (Role::Tab as u16, scenario.title().to_string()),
                "{scenario:?}: first Tab stop is the current page tab"
            );
            assert_eq!(
                stops[1],
                (Role::ComboBox as u16, "Theme".to_string()),
                "{scenario:?}: second Tab stop is Theme"
            );
        }
    }

    fn set_value(app: &mut App, role: Role, name: &str, value: &str) {
        let id = app.find_widget(role, Some(name)).expect("widget to set");
        app.dispatch_action(&ActionRequest {
            action: Action::SetValue,
            target_tree: TreeId::ROOT,
            target_node: to_access_id(id),
            data: Some(ActionData::Value(value.into())),
        });
    }

    /// Emulates the host's remount hook: drains every parked mount, rebuilds
    /// and carries the old app's state over. Returns how many remounts ran.
    fn remount(runtime: &PlaygroundRuntime, app: &mut App) -> usize {
        let mut count = 0;
        while let Some(state) = runtime.take_mount() {
            let mut next = scenario_app(runtime, state, &live_cli());
            next.inherit_remount_state(app);
            next.frame();
            *app = next;
            count += 1;
            assert!(count < 5, "remounts do not settle");
        }
        count
    }

    fn node(app: &App, role: &str, name: &str) -> A11yNodeDump {
        let tree = a11y::dump_tree(app.scene());
        find_by_role_name(&tree, role, Some(name))
            .unwrap_or_else(|| panic!("no {role} named {name:?}"))
            .clone()
    }

    fn selected(app: &App, role: &str, name: &str) -> bool {
        has_state(&node(app, role, name), "selected")
    }

    fn status_has(app: &App, needle: &str) -> bool {
        let tree = a11y::dump_tree(app.scene());
        all_with_role(&tree, "status")
            .iter()
            .any(|s| s.value.as_deref().is_some_and(|v| v.contains(needle)))
    }

    #[test]
    fn gallery_state_survives_opening_density() {
        let runtime = PlaygroundRuntime::default();
        let mut app = mount(&runtime, Scenario::Gallery);
        set_value(&mut app, Role::Slider, "Volume", "100");
        click(&mut app, Role::Radio, Some("Weekly"));
        click(&mut app, Role::Switch, Some("Sync"));
        click(&mut app, Role::CheckBox, Some("Email updates"));
        click(&mut app, Role::Button, Some("Increment"));
        set_value(&mut app, Role::TextInput, "Owner", "Ada Lovelace");
        click(&mut app, Role::Tab, Some("About"));
        click(&mut app, Role::ListItem, Some("Archive"));
        click(&mut app, Role::Cell, Some("Grace Hopper"));
        app.frame();
        click(&mut app, Role::ComboBox, Some("Density"));
        assert_eq!(remount(&runtime, &mut app), 1);

        assert_eq!(node(&app, "slider", "Volume").value.as_deref(), Some("100"));
        let on = |role: &str, name: &str| {
            let n = node(&app, role, name);
            has_state(&n, "checked") || has_state(&n, "selected")
        };
        assert!(on("radio", "Weekly") && !on("radio", "Daily"));
        assert!(!on("switch", "Sync"));
        assert!(!on("checkbox", "Email updates"));
        assert!(status_has(&app, "Clicked once"));
        assert_eq!(
            node(&app, "text_input", "Owner").value.as_deref(),
            Some("Ada Lovelace")
        );
        assert!(selected(&app, "tab", "About") && !selected(&app, "tab", "General"));
        assert!(selected(&app, "list_item", "Archive"));
        let tree = a11y::dump_tree(app.scene());
        let rows = all_with_role(&tree, "row");
        let grace = rows
            .iter()
            .find(|r| {
                r.children
                    .iter()
                    .any(|c| c.name.as_deref() == Some("Grace Hopper"))
            })
            .expect("grace row");
        assert!(has_state(grace, "selected"));
    }

    #[test]
    fn tabs_theme_dropdown_opens_once_and_keeps_the_tab() {
        let runtime = PlaygroundRuntime::default();
        let mut app = mount(&runtime, Scenario::TabsSecondSelected);
        assert!(selected(&app, "tab", "Privacy"));
        click(&mut app, Role::Tab, Some("About"));
        app.frame();
        click(&mut app, Role::ComboBox, Some("Theme"));
        assert_eq!(remount(&runtime, &mut app), 1);
        let tree = a11y::dump_tree(app.scene());
        assert!(!all_with_role(&tree, "list_box_option").is_empty());
        assert!(selected(&app, "tab", "About"));
        assert!(status_has(&app, "Showing About"));
    }

    #[test]
    fn button_count_survives_theme_open_and_close() {
        let runtime = PlaygroundRuntime::default();
        let mut app = mount(&runtime, Scenario::CounterClicked);
        assert!(status_has(&app, "Clicked 3 times"));
        click(&mut app, Role::ComboBox, Some("Theme"));
        assert_eq!(remount(&runtime, &mut app), 1);
        assert!(status_has(&app, "Clicked 3 times"));
        click(&mut app, Role::ComboBox, Some("Theme"));
        assert_eq!(remount(&runtime, &mut app), 1);
        assert!(status_has(&app, "Clicked 3 times"));
    }

    #[test]
    fn reentering_a_scenario_starts_from_its_defaults() {
        let runtime = PlaygroundRuntime::default();
        let mut app = mount(&runtime, Scenario::CounterClicked);
        click(&mut app, Role::Tab, Some("Tabs"));
        remount(&runtime, &mut app);
        click(&mut app, Role::Tab, Some("Button"));
        remount(&runtime, &mut app);
        assert!(status_has(&app, "Clicked 3 times"));
    }
}
