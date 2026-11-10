//! # grouped_tabs — grouped navigation in flat and tree presentations
//!
//! A one-shot/windowed project-navigator example for [`GroupedTabList`]. The same
//! configured tab model is rendered flat, as an expanded tree, with selected
//! branches collapsed, and with a nested tab selected through a real inbound
//! AccessKit action. The screenshot is the visual artifact; the accessibility
//! assertions are the behavioral oracle (SOUL §7).

use std::{cell::Cell, process::ExitCode, rc::Rc};

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, A11yNodeDump, A11yTreeDump, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::layout::EdgeInsets;
use schnellui::scene::Color;
use schnellui::signal::{create_signal, Signal};
use schnellui::testing::find_by_role_name;
use schnellui::theme::FOREST;
use schnellui::widgets::{
    Align, BuildCtx, Button, ButtonAppearance, Column, Divider, GroupedTabList, GroupedTabMode,
    InteractionStates, InteractionStyle, Justify, Pad, Row, Scroll, Stack, TabGroup, TabNode, Text,
    Theme, ThemeProvider, View, WrapMode,
};
use schnellui::App;
use schnellui_icons_md::{outlined, MdIcon};
use strum::IntoEnumIterator;

const PAGE_PADDING: f32 = 24.0;

/// Forest with a firmer hover wash and an ink focus ring, so focus on a
/// selected (green) row is not another shade of green.
const NAVIGATOR_THEME: Theme = Theme {
    interactions: InteractionStates {
        hover: InteractionStyle::background(Color::rgba(0x1d, 0x2a, 0x22, 0x24)),
        focus: InteractionStyle::border(Color::rgb(0x1d, 0x2a, 0x22)),
        active: InteractionStyle::background(Color::rgba(0x1d, 0x2a, 0x22, 0x40)),
    },
    ..FOREST
};

/// Secondary text: captions and metadata (5.2:1 on the page).
const MUTED_THEME: Theme = Theme {
    text: FOREST.text_muted,
    ..NAVIGATOR_THEME
};
const NAV_WIDTH: f32 = 224.0;

#[derive(Clone, Copy)]
struct BranchState {
    planning: bool,
    build: bool,
    archive: bool,
}

#[derive(Clone, Copy)]
enum Branch {
    Planning,
    Build,
    Archive,
}

/// Where a top-level page lives: in its group, under Archive, or gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    Active,
    Archived,
    Deleted,
}

/// The pages a user can archive or delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Planning,
    Build,
    Notes,
}

impl Page {
    fn label(self) -> &'static str {
        match self {
            Page::Planning => "Planning",
            Page::Build => "Build",
            Page::Notes => "Notes",
        }
    }

    /// The page and every page nested under it.
    fn contains(self, name: &str) -> bool {
        let children: &[&str] = match self {
            Page::Planning => &["Roadmap", "Research"],
            Page::Build => &["UI", "Engine"],
            Page::Notes => &[],
        };
        name == self.label() || children.contains(&name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pages {
    planning: Place,
    build: Place,
    notes: Place,
}

impl Pages {
    const ALL_ACTIVE: Pages = Pages {
        planning: Place::Active,
        build: Place::Active,
        notes: Place::Active,
    };

    fn place(self, page: Page) -> Place {
        match page {
            Page::Planning => self.planning,
            Page::Build => self.build,
            Page::Notes => self.notes,
        }
    }

    fn with(mut self, page: Page, place: Place) -> Pages {
        match page {
            Page::Planning => self.planning = place,
            Page::Build => self.build = place,
            Page::Notes => self.notes = place,
        }
        self
    }
}

#[derive(Clone)]
struct ExampleState {
    chosen: Signal<String>,
    last_action: Signal<String>,
    branches: Signal<BranchState>,
    pages: Signal<Pages>,
    /// The pages before the last archive, restore or delete, for Undo.
    undo: Signal<Option<Pages>>,
    remount_requested: Rc<Cell<bool>>,
}

impl ExampleState {
    fn new(scenario: Scenario) -> ExampleState {
        let open = !scenario.collapsed();
        ExampleState {
            chosen: create_signal(scenario.expected_selected().to_string()),
            last_action: create_signal("Nothing archived or deleted yet".to_string()),
            branches: create_signal(BranchState {
                planning: open,
                build: true,
                archive: open,
            }),
            pages: create_signal(Pages::ALL_ACTIVE),
            undo: create_signal(None),
            remount_requested: Rc::new(Cell::new(false)),
        }
    }

    /// Archives, restores or deletes `page`, keeping the old layout for Undo.
    /// A deleted page that was open hands the reading pane back to Overview.
    fn move_page(&self, page: Page, place: Place) {
        let before = self.pages.get();
        self.pages.set(before.with(page, place));
        self.undo.set(Some(before));
        let verb = match place {
            Place::Active => "Restored",
            Place::Archived => "Archived",
            Place::Deleted => "Deleted",
        };
        self.last_action.set(format!("{verb} {}", page.label()));
        if place == Place::Deleted && page.contains(&self.chosen.get()) {
            self.chosen.set("Overview".to_string());
        }
        self.remount_requested.set(true);
    }

    /// Puts the pages back as they were before the last change.
    fn undo(&self) {
        if let Some(before) = self.undo.get() {
            self.pages.set(before);
            self.undo.set(None);
            self.last_action.set("Undone".to_string());
            self.remount_requested.set(true);
        }
    }

    fn expanded(&self, branch: Branch) -> bool {
        let branches = self.branches.get();
        match branch {
            Branch::Planning => branches.planning,
            Branch::Build => branches.build,
            Branch::Archive => branches.archive,
        }
    }

    fn set_expanded(&self, branch: Branch, expanded: bool) {
        self.branches.update(|branches| match branch {
            Branch::Planning => branches.planning = expanded,
            Branch::Build => branches.build = expanded,
            Branch::Archive => branches.archive = expanded,
        });
        self.remount_requested.set(true);
    }

    fn take_remount(&self) -> bool {
        self.remount_requested.replace(false)
    }
}

/// Named, deterministic states exposed through `--list` and `--scenario`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// Recursive nodes are flattened depth-first; configured collapse is ignored.
    Flat,
    /// The complete hierarchy is shown with all branches expanded.
    Tree,
    /// Planning and Archive stay visible while their descendants are omitted.
    Collapsed,
    /// The expanded tree, driven from Overview to nested tab Engine.
    NestedSelected,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::Flat => "flat",
            Scenario::Tree => "tree",
            Scenario::Collapsed => "collapsed",
            Scenario::NestedSelected => "nested_selected",
        }
    }

    fn mode(self) -> GroupedTabMode {
        match self {
            Scenario::Flat => GroupedTabMode::Flat,
            Scenario::Tree | Scenario::Collapsed | Scenario::NestedSelected => GroupedTabMode::Tree,
        }
    }

    fn collapsed(self) -> bool {
        self == Scenario::Collapsed
    }

    fn expected_visible_tabs(self) -> usize {
        if self == Scenario::Collapsed {
            7
        } else {
            11
        }
    }

    fn expected_selected(self) -> &'static str {
        match self {
            Scenario::Collapsed => "Planning",
            Scenario::NestedSelected => "Engine",
            Scenario::Flat | Scenario::Tree => "Overview",
        }
    }
}

/// Standard multi-scenario example CLI (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "grouped_tabs",
    about = "schnellui grouped/tree tab-list example"
)]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 760)]
    width: u32,
    #[arg(long, default_value_t = 600)]
    height: u32,
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// Print scenario names, one per line.
    #[arg(long)]
    list: bool,
    /// Render every scenario into `--out-dir`.
    #[arg(long)]
    all: bool,
    #[arg(long)]
    out_dir: Option<String>,
    /// Write `[{scenario,path,width,height}]` for `--all`.
    #[arg(long)]
    manifest: Option<String>,
    /// Write the retained accessibility tree as JSON.
    #[arg(long)]
    dump_a11y: Option<String>,
    /// Reveal a named action button's hover label in a headless screenshot.
    #[arg(long)]
    hover_action: Option<String>,
    /// Run semantic and interaction assertions.
    #[arg(long)]
    assert: bool,
    /// Open a live native window instead of producing a PNG.
    #[arg(long)]
    windowed: bool,
    /// Run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
}

fn selectable(label: &'static str, state: ExampleState) -> TabNode {
    let chosen = state.chosen;
    TabNode::new(label)
        .selected(chosen.get() == label)
        .on_select(move || chosen.set(label.to_string()))
}

fn branch(
    label: &'static str,
    branch: Branch,
    state: ExampleState,
    children: impl IntoIterator<Item = TabNode>,
) -> TabNode {
    let expanded = state.expanded(branch);
    let toggle_state = state.clone();
    selectable(label, state)
        .expanded(expanded)
        .on_toggle(move |next| toggle_state.set_expanded(branch, next))
        .children(children)
}

/// A compact visual icon overlaid by a real semantic button. The SVG is
/// decorative in the accessibility tree; the button supplies the single,
/// descriptive screen-reader name and the matching hover label.
struct IconAction {
    icon: MdIcon,
    button: Button,
}

impl View for IconAction {
    fn build(
        self: Box<Self>,
        ctx: &mut BuildCtx,
        parent: Option<schnellui::scene::WidgetId>,
    ) -> schnellui::scene::WidgetId {
        let this = *self;
        let root = Box::new(
            Stack::new()
                .size(28.0, 28.0)
                .align(Align::Center)
                .justify(Justify::Center),
        )
        .build(ctx, parent);
        let icon = Box::new(this.icon).build(ctx, Some(root));
        let semantics = ctx.scene.a11y_mut(icon);
        semantics.role = Role::Group.as_u16();
        semantics.name = None;
        Box::new(this.button).build(ctx, Some(root));
        root
    }
}

fn icon_action(
    icon: MdIcon,
    label: String,
    state: ExampleState,
    page: Page,
    place: Place,
) -> IconAction {
    IconAction {
        icon: icon.size(18.0).color(FOREST.text_muted),
        button: Button::new(label.clone())
            .icon_only()
            .tooltip(label)
            .width(28.0)
            .height(28.0)
            .appearance(ButtonAppearance::Ghost)
            .on_click(move || state.move_page(page, place)),
    }
}

/// The row actions of a top-level page: Archive and Delete while it is in its
/// group, Restore once it sits under Archive. Every such page gets the same set.
fn page_actions(page: Page, state: &ExampleState) -> Vec<IconAction> {
    let name = page.label();
    match state.pages.get().place(page) {
        Place::Active => vec![
            icon_action(
                MdIcon::outlined("inventory_2", outlined::ICON_INVENTORY_2),
                format!("Archive {name}"),
                state.clone(),
                page,
                Place::Archived,
            ),
            icon_action(
                MdIcon::outlined("delete", outlined::ICON_DELETE),
                format!("Delete {name}"),
                state.clone(),
                page,
                Place::Deleted,
            ),
        ],
        Place::Archived => vec![icon_action(
            MdIcon::outlined("unarchive", outlined::ICON_UNARCHIVE),
            format!("Restore {name}"),
            state.clone(),
            page,
            Place::Active,
        )],
        Place::Deleted => Vec::new(),
    }
}

/// A top-level page's row, with its children and actions.
fn page_node(page: Page, state: &ExampleState) -> TabNode {
    let node = match page {
        Page::Planning => branch(
            "Planning",
            Branch::Planning,
            state.clone(),
            [
                selectable("Roadmap", state.clone()),
                selectable("Research", state.clone()),
            ],
        ),
        Page::Build => branch(
            "Build",
            Branch::Build,
            state.clone(),
            [
                selectable("UI", state.clone()),
                selectable("Engine", state.clone()),
            ],
        ),
        Page::Notes => selectable("Notes", state.clone()),
    };
    node.actions(page_actions(page, state))
}

/// Builds the example's data-driven navigation. Passing a different mode changes
/// presentation only; groups, tab labels, callbacks, and recursive data stay shared.
fn project_navigation(scenario: Scenario, state: ExampleState) -> GroupedTabList {
    let pages = state.pages.get();
    let in_place = |group: &[Page], place: Place| -> Vec<TabNode> {
        group
            .iter()
            .filter(|page| pages.place(**page) == place)
            .map(|page| page_node(*page, &state))
            .collect()
    };
    let mut project = TabGroup::new("PROJECT").tab(selectable("Overview", state.clone()));
    for node in in_place(&[Page::Planning, Page::Build], Place::Active) {
        project = project.tab(node);
    }
    let mut personal = TabGroup::new("PERSONAL");
    for node in in_place(&[Page::Notes], Place::Active) {
        personal = personal.tab(node);
    }
    let mut archived = in_place(&[Page::Planning, Page::Build, Page::Notes], Place::Archived);
    archived.push(selectable("2025", state.clone()));
    archived.push(selectable("2024", state.clone()));
    personal = personal.tab(branch("Archive", Branch::Archive, state.clone(), archived));

    GroupedTabList::new()
        .mode(scenario.mode())
        .group_gap(14.0)
        .tab_gap(2.0)
        .indent(18.0)
        .flat_indent(14.0)
        .group_label_size(12.0)
        .min_tab_width(NAV_WIDTH)
        .groups([project, personal])
}

/// What the reading pane shows for each page: a one-line status and the body.
fn document(name: &str) -> (&'static str, &'static str) {
    match name {
        "Overview" => (
            "Edited today by Ines",
            "Fieldwork collects the survey notes from the spring trip. Start with Planning for \
             the route, then Build for the tools we are writing to sort the samples.",
        ),
        "Planning" => (
            "3 open questions",
            "Where we go, when, and who carries what. Roadmap holds the dates; Research holds \
             the reading list and the open questions.",
        ),
        "Roadmap" => (
            "Next milestone: 14 May",
            "April: permits and transport. May: two weeks on the coast. June: sort, label and \
             publish the first set of samples.",
        ),
        "Research" => (
            "12 sources",
            "Tide tables for the bay, the 2019 survey report, and the field guide chapters on \
             lichens. Each source has a short note on why it matters.",
        ),
        "Build" => (
            "2 tools in progress",
            "Small programs that help in the field: a sample label printer and an offline map \
             of the survey squares.",
        ),
        "UI" => (
            "Draft",
            "The label printer has one screen: pick a square, type the species, print. Large \
             targets, because we use it with gloves on.",
        ),
        "Engine" => (
            "Tests passing",
            "The map engine tiles the survey area into 50 m squares and keeps the last week of \
             tracks on the device.",
        ),
        "Notes" => (
            "Private",
            "Loose thoughts that are not ready for the team yet. Archive or delete them from \
             the buttons next to the page name.",
        ),
        "Archive" => (
            "2 past seasons",
            "Read-only records of earlier trips. Open a year to see what we found and what we \
             would do differently.",
        ),
        "2025" => (
            "Read-only",
            "Autumn trip: 48 samples, two new sites, one broken tripod.",
        ),
        "2024" => (
            "Read-only",
            "First season: we learned to bring spare batteries and a second map.",
        ),
        _ => ("", ""),
    }
}

/// Secondary text in the muted ink.
fn caption(text: impl Into<std::borrow::Cow<'static, str>>, size: f32) -> impl View {
    ThemeProvider::new(MUTED_THEME, Text::new(text).size(size).wrap(WrapMode::Word))
}

/// A view that may be absent: builds `Some` as usual and `None` as an empty,
/// zero-size stack, so a row can drop a control without another view type.
struct Maybe<V>(Option<V>);

impl<V: View> View for Maybe<V> {
    fn build(
        self: Box<Self>,
        ctx: &mut BuildCtx,
        parent: Option<schnellui::scene::WidgetId>,
    ) -> schnellui::scene::WidgetId {
        match self.0 {
            Some(view) => Box::new(view).build(ctx, parent),
            None => Box::new(Stack::new()).build(ctx, parent),
        }
    }
}

/// A project navigator: grouped pages on the left and the chosen page on the
/// right. The Forest design system supplies the palette; all component
/// geometry stays theme-native.
fn example_view(scenario: Scenario, state: ExampleState) -> impl View {
    let chosen = state.chosen;
    let last_action = state.last_action;
    let undo = state.undo.get().is_some().then(|| {
        let state = state.clone();
        Button::new("Undo")
            .appearance(ButtonAppearance::Outline)
            .height(32.0)
            .on_click(move || state.undo())
    });
    let navigation = project_navigation(scenario, state);
    let hint = match scenario.mode() {
        GroupedTabMode::Flat => "Every page in one list",
        GroupedTabMode::Tree => "Click an open section again to fold it",
    };

    // A window shorter than the page scrolls it, so every row action can be
    // shown.
    // `fit_width` hands the page the window's width, so the reading pane
    // wraps under the navigation in a narrow window instead of running off it.
    Scroll::new().fill().fit_width().scrollbar(true).child(
        Pad::all(PAGE_PADDING).child(
            Column::new()
                .gap(14.0)
                .align(Align::Stretch)
                .child(
                    Row::new()
                        .width(712.0)
                        .fit_parent()
                        .gap(12.0)
                        .justify(Justify::SpaceBetween)
                        .align(Align::End)
                        .child(Text::new("Fieldwork").size(28.0).bold())
                        // 13px text descends 4px less than 28px text; lift it onto
                        // the title's baseline.
                        .child(
                            Pad::insets(EdgeInsets {
                                top: 0.0,
                                right: 0.0,
                                bottom: 4.0,
                                left: 0.0,
                            })
                            .child(caption(hint, 13.0)),
                        ),
                )
                .child(Divider::new())
                .child(
                    Row::new()
                        .gap(30.0)
                        .wrap()
                        .align(Align::Start)
                        .child(navigation)
                        .child(
                            // The pane's box is its 8px padding plus a 420px column, and
                            // never wider than the window lets it be.
                            Column::new().width(436.0).fit_parent().child(
                                Pad::all(8.0).child(
                                    Column::new()
                                        .align(Align::Stretch)
                                        .gap(10.0)
                                        .child(
                                            Text::dynamic(move || chosen.get())
                                                .size(30.0)
                                                .bold()
                                                .role(Role::Status),
                                        )
                                        .child(ThemeProvider::new(
                                            MUTED_THEME,
                                            Text::dynamic(move || {
                                                document(&chosen.get()).0.to_string()
                                            })
                                            .size(13.0),
                                        ))
                                        .child(Divider::new())
                                        .child(
                                            Text::dynamic(move || {
                                                document(&chosen.get()).1.to_string()
                                            })
                                            .size(16.0)
                                            .wrap(WrapMode::Word),
                                        )
                                        .child(Divider::new())
                                        .child(
                                            Row::new()
                                                .justify(Justify::SpaceBetween)
                                                .align(Align::Center)
                                                .child(
                                                    Column::new()
                                                        .gap(2.0)
                                                        .child(caption("Last change", 12.0))
                                                        .child(
                                                            Text::dynamic(move || {
                                                                last_action.get()
                                                            })
                                                            .size(14.0)
                                                            .role(Role::Status),
                                                        ),
                                                )
                                                .child(Maybe(undo)),
                                        ),
                                ),
                            ),
                        ),
                ),
        ),
    )
}

fn mount_scenario(
    scenario: Scenario,
    width: u32,
    height: u32,
    scale: f32,
    state: ExampleState,
) -> App {
    let view_state = state.clone();
    App::mount_themed_with_size_scaled(
        NAVIGATOR_THEME,
        move || example_view(scenario, view_state.clone()),
        width,
        height,
        scale,
    )
}

fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> (App, ExampleState) {
    let state = ExampleState::new(scenario);
    let mut app = mount_scenario(scenario, width, height, scale, state.clone());
    if scenario == Scenario::NestedSelected {
        drive_click(&mut app, "Engine");
    }
    (app, state)
}

/// Drives a tab through the same inbound path used by a screen reader.
fn drive_click(app: &mut App, name: &str) {
    if !drive_named_click(app, Role::Tab, name) {
        eprintln!("drive: missing tab {name:?}");
    }
}

fn drive_named_click(app: &mut App, role: Role, name: &str) -> bool {
    let Some(id) = app.find_widget(role, Some(name)) else {
        return false;
    };
    let request = ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    };
    app.dispatch_action(&request)
}

fn all_with_role<'a>(tree: &'a A11yTreeDump, role: &str) -> Vec<&'a A11yNodeDump> {
    fn walk<'a>(node: &'a A11yNodeDump, role: &str, out: &mut Vec<&'a A11yNodeDump>) {
        if node.role == role {
            out.push(node);
        }
        for child in &node.children {
            walk(child, role, out);
        }
    }

    let mut nodes = Vec::new();
    if let Some(root) = &tree.root {
        walk(root, role, &mut nodes);
    }
    nodes
}

fn has_state(node: &A11yNodeDump, state: &str) -> bool {
    node.state.iter().any(|candidate| candidate == state)
}

fn assert_tab_state(
    tree: &A11yTreeDump,
    name: &str,
    state: &str,
    expected: bool,
) -> Result<(), String> {
    let tab = find_by_role_name(tree, "tab", Some(name))
        .ok_or_else(|| format!("missing tab {name:?}"))?;
    let actual = has_state(tab, state);
    if actual != expected {
        return Err(format!(
            "tab {name:?} state {state:?} was {actual}, expected {expected}"
        ));
    }
    Ok(())
}

fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    find_by_role_name(&tree, "tab_list", None).ok_or("missing semantic tab list")?;
    find_by_role_name(&tree, "group", Some("PROJECT")).ok_or("missing PROJECT group")?;
    find_by_role_name(&tree, "group", Some("PERSONAL")).ok_or("missing PERSONAL group")?;
    for action in [
        "Archive Planning",
        "Delete Planning",
        "Archive Build",
        "Delete Build",
        "Archive Notes",
        "Delete Notes",
    ] {
        find_by_role_name(&tree, "button", Some(action))
            .ok_or_else(|| format!("missing content action {action:?}"))?;
    }

    let tabs = all_with_role(&tree, "tab");
    if tabs.len() != scenario.expected_visible_tabs() {
        return Err(format!(
            "visible tab count was {}, expected {}",
            tabs.len(),
            scenario.expected_visible_tabs()
        ));
    }

    let selected: Vec<_> = tabs
        .iter()
        .copied()
        .filter(|tab| has_state(tab, "selected"))
        .collect();
    if selected.len() != 1 || selected[0].name.as_deref() != Some(scenario.expected_selected()) {
        return Err(format!(
            "selected tabs were {:?}, expected exactly {:?}",
            selected
                .iter()
                .map(|tab| tab.name.as_deref())
                .collect::<Vec<_>>(),
            scenario.expected_selected()
        ));
    }
    if !all_with_role(&tree, "status").iter().any(|status| {
        status
            .value
            .as_deref()
            .is_some_and(|value| value.contains(scenario.expected_selected()))
    }) {
        return Err(format!(
            "no status reports active tab {:?}",
            scenario.expected_selected()
        ));
    }

    match scenario {
        Scenario::Flat => {
            for name in ["Planning", "Build", "Archive"] {
                assert_tab_state(&tree, name, "expanded", false)?;
            }
        }
        Scenario::Tree | Scenario::NestedSelected => {
            for name in ["Planning", "Build", "Archive"] {
                assert_tab_state(&tree, name, "expanded", true)?;
            }
        }
        Scenario::Collapsed => {
            assert_tab_state(&tree, "Planning", "expanded", false)?;
            assert_tab_state(&tree, "Build", "expanded", true)?;
            assert_tab_state(&tree, "Archive", "expanded", false)?;
            for hidden in ["Roadmap", "Research", "2025", "2024"] {
                if find_by_role_name(&tree, "tab", Some(hidden)).is_some() {
                    return Err(format!("collapsed descendant {hidden:?} remained visible"));
                }
            }
        }
    }
    Ok(())
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let (mut app, _state) = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame();
    if let Some(name) = &cli.hover_action {
        let Some(action) = app.find_widget(Role::Button, Some(name)) else {
            eprintln!("hover-action: missing button {name:?}");
            return ExitCode::FAILURE;
        };
        let rect = app.scene().layout(action).unwrap().rect;
        app.update_pointer_proximity(schnellui::scene::Point {
            x: rect.x + rect.width * 0.5,
            y: rect.y + rect.height * 0.5,
        });
    }

    if let Some(path) = &cli.dump_a11y {
        if let Err(error) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    if cli.assert {
        if let Err(error) = run_assertions(scenario, &app) {
            eprintln!("assertion failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    if let Err(error) = app.render_to_png(out) {
        eprintln!("render failed: {error}");
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
        for scenario in Scenario::iter() {
            println!("{}", scenario.name());
        }
        return ExitCode::SUCCESS;
    }

    if cli.all {
        let directory = cli.out_dir.clone().unwrap_or_else(|| ".".to_string());
        if let Err(error) = std::fs::create_dir_all(&directory) {
            eprintln!("could not create out-dir {directory:?}: {error}");
            return ExitCode::FAILURE;
        }
        let physical_width = (cli.width as f32 * cli.scale).round().max(1.0) as u32;
        let physical_height = (cli.height as f32 * cli.scale).round().max(1.0) as u32;
        let mut manifest = Vec::new();
        for scenario in Scenario::iter() {
            let out = format!("{directory}/{}.png", scenario.name());
            let code = render_one(scenario, &cli, &out);
            if code != ExitCode::SUCCESS {
                return code;
            }
            manifest.push(manifest_entry(
                scenario.name(),
                &out,
                physical_width,
                physical_height,
            ));
        }
        if let Some(path) = &cli.manifest {
            if let Err(error) = std::fs::write(path, format!("[{}]", manifest.join(","))) {
                eprintln!("manifest write failed: {error}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    let Some(scenario) = cli.scenario else {
        eprintln!("one of --scenario, --list, or --all is required");
        return ExitCode::FAILURE;
    };
    if cli.windowed || cli.headless_windowed {
        let (mut app, state) = scenario_app(scenario, cli.width, cli.height, cli.scale);
        app.set_escape_exits(true);
        let remount_state = state.clone();
        return match app.run_windowed_with("Grouped tabs / Project navigator", move || {
            remount_state.take_remount().then(|| {
                mount_scenario(
                    scenario,
                    cli.width,
                    cli.height,
                    cli.scale,
                    remount_state.clone(),
                )
            })
        }) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("windowed run failed: {error}");
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

fn manifest_entry(name: &str, path: &str, width: u32, height: u32) -> String {
    format!("{{\"scenario\":\"{name}\",\"path\":\"{path}\",\"width\":{width},\"height\":{height}}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::testing::fuzz::tab_stops_reachable;

    /// At a phone width no text or control is cut at a side with nothing to
    /// scroll it in: prose wraps and fixed-width content reflows or scrolls.
    #[test]
    fn nothing_is_clipped_sideways_at_360() {
        use schnellui::testing::fuzz::sideways_clips;
        let mut faults = Vec::new();
        for scenario in Scenario::iter() {
            let (mut app, _state) = scenario_app(scenario, 360, 600, 1.0);
            for fault in sideways_clips(&mut app) {
                faults.push(format!("{}: {fault}", scenario.name()));
            }
        }
        assert!(faults.is_empty(), "{faults:#?}");
    }

    /// Every Tab stop of every scenario can be shown, at the default size and
    /// in a narrow, short window: something must scroll it in.
    #[test]
    fn every_tab_stop_reaches_the_window() {
        let mut stops = 0;
        for scenario in Scenario::iter() {
            for (w, h) in [(760, 600), (760, 240), (760, 120), (360, 240)] {
                let (mut app, _state) = scenario_app(scenario, w, h, 1.0);
                stops += tab_stops_reachable(&mut app)
                    .unwrap_or_else(|e| panic!("{} at {w}x{h}: {e}", scenario.name()));
            }
        }
        assert!(stops > 0, "no scenario has a Tab stop");
    }

    #[test]
    fn every_grouped_tab_scenario_satisfies_its_semantic_oracle() {
        for scenario in Scenario::iter() {
            let (mut app, _state) = scenario_app(scenario, 760, 600, 1.0);
            app.frame();
            run_assertions(scenario, &app)
                .unwrap_or_else(|error| panic!("{}: {error}", scenario.name()));
        }
    }

    #[test]
    fn activating_a_branch_collapses_it_and_preserves_selection_after_remount() {
        let (mut app, state) = scenario_app(Scenario::Tree, 760, 600, 1.0);
        app.frame();

        // The first click selects the open branch and leaves it open.
        drive_click(&mut app, "Planning");
        assert!(!state.take_remount());
        assert_eq!(state.chosen.get(), "Planning");
        // Clicking the selected branch folds it.
        drive_click(&mut app, "Planning");
        assert!(state.take_remount());
        assert_eq!(state.chosen.get(), "Planning");

        let mut app = mount_scenario(Scenario::Tree, 760, 600, 1.0, state);
        app.frame();
        let tree = a11y::dump_tree(app.scene());
        assert_tab_state(&tree, "Planning", "collapsed", true).unwrap();
        assert_tab_state(&tree, "Planning", "selected", true).unwrap();
        assert!(find_by_role_name(&tree, "tab", Some("Roadmap")).is_none());
        assert!(find_by_role_name(&tree, "tab", Some("Research")).is_none());
    }

    #[test]
    fn multiple_content_actions_run_without_selecting_their_tab() {
        let (mut app, state) = scenario_app(Scenario::Tree, 760, 600, 1.0);
        app.frame();
        let notes = app.find_widget(Role::Tab, Some("Notes")).unwrap();
        let archive = app
            .find_widget(Role::Button, Some("Archive Notes"))
            .unwrap();
        let delete = app.find_widget(Role::Button, Some("Delete Notes")).unwrap();
        let notes_rect = app.scene().layout(notes).unwrap().rect;
        let archive_rect = app.scene().layout(archive).unwrap().rect;
        let delete_rect = app.scene().layout(delete).unwrap().rect;
        assert!(notes_rect.right() <= archive_rect.x);
        assert!(archive_rect.right() <= delete_rect.x);
        assert!(delete_rect.right() <= PAGE_PADDING + NAV_WIDTH);

        assert!(drive_named_click(&mut app, Role::Button, "Archive Notes"));
        assert_eq!(state.last_action.get(), "Archived Notes");
        assert_eq!(state.chosen.get(), "Overview");
        assert!(state.take_remount());
        let mut app = mount_scenario(Scenario::Tree, 760, 600, 1.0, state.clone());
        app.frame();
        // Notes moved under Archive and offers Restore instead.
        assert!(app
            .find_widget(Role::Button, Some("Archive Notes"))
            .is_none());
        assert!(app
            .find_widget(Role::Button, Some("Restore Notes"))
            .is_some());
        let tree = a11y::dump_tree(app.scene());
        assert_tab_state(&tree, "Overview", "selected", true).unwrap();
        assert_tab_state(&tree, "Notes", "selected", false).unwrap();
    }

    #[test]
    fn deleting_a_page_removes_its_row_and_undo_brings_it_back() {
        let (mut app, state) = scenario_app(Scenario::NestedSelected, 760, 600, 1.0);
        app.frame();
        assert_eq!(state.chosen.get(), "Engine");
        assert!(app.find_widget(Role::Button, Some("Undo")).is_none());

        assert!(drive_named_click(&mut app, Role::Button, "Delete Build"));
        assert_eq!(state.last_action.get(), "Deleted Build");
        // The open page was inside Build, so the pane falls back to Overview.
        assert_eq!(state.chosen.get(), "Overview");
        assert!(state.take_remount());
        let mut app = mount_scenario(Scenario::NestedSelected, 760, 600, 1.0, state.clone());
        app.frame();
        for gone in ["Build", "UI", "Engine"] {
            assert!(
                app.find_widget(Role::Tab, Some(gone)).is_none(),
                "{gone} still shows"
            );
        }

        assert!(drive_named_click(&mut app, Role::Button, "Undo"));
        assert!(state.take_remount());
        let mut app = mount_scenario(Scenario::NestedSelected, 760, 600, 1.0, state.clone());
        app.frame();
        assert!(app.find_widget(Role::Tab, Some("Build")).is_some());
        assert!(app.find_widget(Role::Button, Some("Undo")).is_none());
        assert_eq!(state.last_action.get(), "Undone");
    }

    #[test]
    fn flat_content_actions_remain_inside_the_navigation_column() {
        let (mut app, _state) = scenario_app(Scenario::Flat, 760, 600, 1.0);
        app.frame();
        let planning = app.find_widget(Role::Tab, Some("Planning")).unwrap();
        let archive = app
            .find_widget(Role::Button, Some("Archive Planning"))
            .unwrap();
        let planning_rect = app.scene().layout(planning).unwrap().rect;
        let archive_rect = app.scene().layout(archive).unwrap().rect;
        assert!(
            planning_rect.right() <= archive_rect.x
                && archive_rect.right() <= PAGE_PADDING + NAV_WIDTH,
            "planning={planning_rect:?}, archive={archive_rect:?}"
        );
        for id in [planning, archive] {
            for primitive in &app.scene().paint(id).unwrap().primitives {
                let (right, visible) = match primitive {
                    schnellui::scene::Primitive::SolidRect { rect, color, .. }
                    | schnellui::scene::Primitive::StrokeRect { rect, color, .. }
                    | schnellui::scene::Primitive::GlyphQuad { rect, color, .. } => {
                        (rect.right(), color.a != 0)
                    }
                    schnellui::scene::Primitive::ImageQuad { rect, tint, .. } => {
                        (rect.right(), tint.a != 0)
                    }
                    schnellui::scene::Primitive::Line {
                        from,
                        to,
                        width,
                        color,
                    } => (from.x.max(to.x) + width * 0.5, color.a != 0),
                };
                assert!(
                    !visible || right <= PAGE_PADDING + NAV_WIDTH,
                    "paint escaped navigation: {primitive:?}"
                );
            }
        }
    }

    #[test]
    fn icon_actions_have_one_accessible_name_and_reveal_hover_text() {
        let (mut app, _state) = scenario_app(Scenario::Tree, 760, 600, 1.0);
        app.frame();
        let action = app.find_widget(Role::Button, Some("Delete Notes")).unwrap();
        let action_rect = app.scene().layout(action).unwrap().rect;
        let notes = app.find_widget(Role::Tab, Some("Notes")).unwrap();
        let notes_rect = app.scene().layout(notes).unwrap().rect;
        assert!(
            (action_rect.y - notes_rect.y).abs() < 1.0,
            "notes={notes_rect:?}, action={action_rect:?}"
        );
        let visible_colors = |app: &App| {
            app.scene()
                .paint(action)
                .unwrap()
                .primitives
                .iter()
                .filter(|primitive| match primitive {
                    schnellui::scene::Primitive::SolidRect { color, .. }
                    | schnellui::scene::Primitive::StrokeRect { color, .. }
                    | schnellui::scene::Primitive::GlyphQuad { color, .. }
                    | schnellui::scene::Primitive::Line { color, .. } => color.a != 0,
                    schnellui::scene::Primitive::ImageQuad { tint, .. } => tint.a != 0,
                })
                .count()
        };
        assert_eq!(visible_colors(&app), 0);

        assert!(app.update_pointer_proximity(schnellui::scene::Point {
            x: action_rect.x + action_rect.width * 0.5,
            y: action_rect.y + action_rect.height * 0.5,
        }));
        assert!(visible_colors(&app) > 1);

        let tree = a11y::dump_tree(app.scene());
        assert_eq!(
            all_with_role(&tree, "button")
                .into_iter()
                .filter(|node| node.name.as_deref() == Some("Delete Notes"))
                .count(),
            1
        );
        assert!(all_with_role(&tree, "image").is_empty());
    }
}
