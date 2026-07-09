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
use schnellui::signal::{create_signal, Signal};
use schnellui::theme::FOREST;
use schnellui::widgets::{
    Align, Badge, BuildCtx, Button, ButtonAppearance, Column, Divider, GroupedTabList,
    GroupedTabMode, Justify, Pad, Row, Stack, TabGroup, TabNode, Text, View, WrapMode,
};
use schnellui::App;
use schnellui_icons_md::{outlined, MdIcon};
use schnellui_testing::find_by_role_name;
use strum::IntoEnumIterator;

const PAGE_PADDING: f32 = 24.0;
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

#[derive(Clone)]
struct ExampleState {
    chosen: Signal<String>,
    last_action: Signal<String>,
    branches: Signal<BranchState>,
    remount_requested: Rc<Cell<bool>>,
}

impl ExampleState {
    fn new(scenario: Scenario) -> ExampleState {
        let open = !scenario.collapsed();
        ExampleState {
            chosen: create_signal(scenario.expected_selected().to_string()),
            last_action: create_signal("No document action yet".to_string()),
            branches: create_signal(BranchState {
                planning: open,
                build: true,
                archive: open,
            }),
            remount_requested: Rc::new(Cell::new(false)),
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

    fn badge(self) -> &'static str {
        match self {
            Scenario::Flat => "FLAT LIST",
            Scenario::Tree => "TREE / OPEN",
            Scenario::Collapsed => "TREE / FOLDED",
            Scenario::NestedSelected => "TREE / DRIVEN",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Scenario::Flat => {
                "The hierarchy is retained in configuration but presented as one depth-first list."
            }
            Scenario::Tree => {
                "Parent and child tabs keep their relationships, with a consistent indented edge."
            }
            Scenario::Collapsed => {
                "Planning and Archive are folded; their tabs remain configured but are not built."
            }
            Scenario::NestedSelected => {
                "An AccessKit Click selected Engine, clearing Overview across groups and depths."
            }
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
    label: &'static str,
    message: &'static str,
    state: ExampleState,
) -> IconAction {
    let last_action = state.last_action;
    IconAction {
        icon: icon.size(18.0).color(FOREST.text),
        button: Button::new(label)
            .icon_only()
            .tooltip(label)
            .width(28.0)
            .height(28.0)
            .appearance(ButtonAppearance::Ghost)
            .on_click(move || last_action.set(message.to_string())),
    }
}

/// Builds the example's data-driven navigation. Passing a different mode changes
/// presentation only; groups, tab labels, callbacks, and recursive data stay shared.
fn project_navigation(scenario: Scenario, state: ExampleState) -> GroupedTabList {
    GroupedTabList::new()
        .mode(scenario.mode())
        .group_gap(14.0)
        .tab_gap(2.0)
        .indent(18.0)
        .group_label_size(11.0)
        .min_tab_width(NAV_WIDTH)
        .groups([
            TabGroup::new("PROJECT")
                .tab(selectable("Overview", state.clone()))
                .tab(
                    branch(
                        "Planning",
                        Branch::Planning,
                        state.clone(),
                        [
                            selectable("Roadmap", state.clone()),
                            selectable("Research", state.clone()),
                        ],
                    )
                    .action(icon_action(
                        MdIcon::outlined("archive", outlined::ICON_ARCHIVE),
                        "Archive Planning",
                        "Archived Planning",
                        state.clone(),
                    )),
                )
                .tab(branch(
                    "Build",
                    Branch::Build,
                    state.clone(),
                    [
                        selectable("UI", state.clone()),
                        selectable("Engine", state.clone()),
                    ],
                )),
            TabGroup::new("PERSONAL")
                .tab(selectable("Notes", state.clone()).actions([
                    icon_action(
                        MdIcon::outlined("archive", outlined::ICON_ARCHIVE),
                        "Archive Notes",
                        "Archived Notes",
                        state.clone(),
                    ),
                    icon_action(
                        MdIcon::outlined("delete", outlined::ICON_DELETE),
                        "Delete Notes",
                        "Deleted Notes",
                        state.clone(),
                    ),
                ]))
                .tab(branch(
                    "Archive",
                    Branch::Archive,
                    state.clone(),
                    [
                        selectable("2025", state.clone()),
                        selectable("2024", state.clone()),
                    ],
                )),
        ])
}

/// An editorial, utilitarian project navigator: dense navigation on the left and
/// a calm reading pane on the right. The Forest design system provides the warm
/// field-notebook palette while all component geometry remains theme-native.
fn example_view(scenario: Scenario, state: ExampleState) -> impl View {
    let chosen = state.chosen;
    let last_action = state.last_action;
    let navigation = project_navigation(scenario, state);

    Pad::all(PAGE_PADDING).child(
        Column::new()
            .gap(14.0)
            .child(
                Row::new()
                    .gap(12.0)
                    .align(Align::Center)
                    .child(Text::new("Fieldwork / Navigator").size(28.0))
                    .child(Badge::new(scenario.badge())),
            )
            .child(
                Text::new(scenario.description())
                    .size(14.0)
                    .wrap(WrapMode::Word),
            )
            .child(Divider::new())
            .child(
                Row::new()
                    .gap(30.0)
                    .align(Align::Start)
                    .child(navigation)
                    .child(
                        Pad::all(18.0).child(
                            Column::new()
                                .width(360.0)
                                .gap(12.0)
                                .child(Text::new("ACTIVE DOCUMENT").size(11.0))
                                .child(
                                    Text::dynamic(move || chosen.get())
                                        .size(30.0)
                                        .role(Role::Status),
                                )
                                .child(Divider::new())
                                .child(
                                    Text::new(
                                        "Grouped tabs share one selection scope. A nested tab \
                                             can be selected without losing its place in the tree, \
                                             and every group remains available to assistive tools.",
                                    )
                                    .size(16.0)
                                    .wrap(WrapMode::Word),
                                )
                                .child(Text::new("TRY IT").size(11.0))
                                .child(
                                    Text::new(
                                        "Select any row, or activate a branch to fold and reopen it. \
                                             Rows can expose multiple independent actions. Hover an icon \
                                             for its label; assistive tools receive the same descriptive \
                                             button name.",
                                    )
                                    .size(14.0)
                                    .wrap(WrapMode::Word),
                                )
                                .child(Text::new("LAST ACTION").size(11.0))
                                .child(
                                    Text::dynamic(move || last_action.get())
                                        .size(14.0)
                                        .role(Role::Status),
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
        FOREST,
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
    for action in ["Archive Planning", "Archive Notes", "Delete Notes"] {
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
    if cli.windowed {
        let (app, state) = scenario_app(scenario, cli.width, cli.height, cli.scale);
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
        app.frame();
        assert_eq!(state.last_action.get(), "Archived Notes");
        assert_eq!(state.chosen.get(), "Overview");
        assert!(!state.take_remount());

        assert!(drive_named_click(&mut app, Role::Button, "Delete Notes"));
        app.frame();
        assert_eq!(state.last_action.get(), "Deleted Notes");
        assert_eq!(state.chosen.get(), "Overview");

        let tree = a11y::dump_tree(app.scene());
        assert_tab_state(&tree, "Overview", "selected", true).unwrap();
        assert_tab_state(&tree, "Notes", "selected", false).unwrap();
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
