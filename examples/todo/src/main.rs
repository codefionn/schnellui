//! A small but complete todo app with add, complete, remove, and clear actions.
//!
//! Run it interactively with:
//! `cargo run -p todo -- --scenario day_plan --windowed`

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, A11yNodeDump, Role};
use schnellui::accesskit_action::{Action, ActionData, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::{Color, Primitive, Rect, Size, WidgetId, WidgetKind};
use schnellui::testing::find_by_role_name;
use schnellui::widgets::{
    Align, Badge, Button, ButtonAppearance, Checkbox, Column, Divider, Flex, Justify, Pad,
    ProgressBar, Row, Scroll, Shape, Stack, Tab, TabBar, Text, TextInput, Theme, ThemeProvider,
    View,
};
use schnellui::{App, Context, State};
use strum::IntoEnumIterator;

const TODO_THEME: Theme = Theme {
    text: Color::rgb(0x24, 0x28, 0x23),
    text_muted: Color::rgb(0x76, 0x70, 0x63),
    surface: Color::rgb(0xff, 0xfb, 0xee),
    surface_muted: Color::rgb(0xf2, 0xe8, 0xd3),
    separator: Color::rgb(0xc9, 0xb9, 0x9d),
    outline: Color::rgb(0x24, 0x28, 0x23),
    // Deep enough for the cream button labels to reach 4.5:1.
    accent: Color::rgb(0xc0, 0x45, 0x2b),
    on_accent: Color::rgb(0xff, 0xfb, 0xee),
    selection: Color::rgb(0xf7, 0xdc, 0x72),
    interactions: schnellui::widgets::InteractionStates {
        // Hover lays a faint ink wash and press a stronger one; neither
        // recolours a label, so hover never reads as pressed.
        hover: schnellui::widgets::InteractionStyle::background(Color::rgba(
            0x24, 0x28, 0x23, 0x22,
        )),
        focus: schnellui::widgets::InteractionStyle::border(Color::rgb(0x24, 0x28, 0x23)),
        active: schnellui::widgets::InteractionStyle::background(Color::rgba(
            0x24, 0x28, 0x23, 0x3a,
        )),
    },
    component_interactions: schnellui::widgets::ComponentInteractions {
        // A filled button needs a deeper wash before its change reads.
        button: Some(schnellui::widgets::InteractionStates {
            hover: schnellui::widgets::InteractionStyle::background(Color::rgba(
                0x24, 0x28, 0x23, 0x48,
            )),
            focus: schnellui::widgets::InteractionStyle::border(Color::rgb(0x24, 0x28, 0x23)),
            active: schnellui::widgets::InteractionStyle::background(Color::rgba(
                0x24, 0x28, 0x23, 0x70,
            )),
        }),
        ..schnellui::widgets::ComponentInteractions::NONE
    },
    text_selection: Color::rgb(0xf6, 0xc9, 0x79),
    disabled: Color::rgb(0xec, 0xe3, 0xcf),
    positive: Color::rgb(0x1d, 0x70, 0x5d),
    attention: Color::rgb(0xf2, 0xb8, 0x44),
    media: Color::rgb(0xdc, 0xd0, 0xba),
    page: Color::rgb(0xe5, 0xd8, 0xbf),
    shape: Shape {
        roundness: 0.65,
        density: 1.1,
        frame: 1.0,
        shadow: 3.0,
    },
};

/// The height of the task entry and its Add button, so they line up.
const ENTRY_HEIGHT: f32 = 40.0;
/// A task row leaves a gutter at the list's right edge for the scrollbar.
const TASK_ROW_WIDTH: f32 = 516.0;

/// A finished task's words: the board theme with muted ink.
const DONE_THEME: Theme = Theme {
    text: Color::rgb(0x6b, 0x67, 0x5c),
    ..TODO_THEME
};

struct BoardBackground;

impl View for BoardBackground {
    fn build(
        self: Box<Self>,
        ctx: &mut schnellui::widgets::BuildCtx,
        parent: Option<WidgetId>,
    ) -> WidgetId {
        let id = ctx.scene.insert(WidgetKind::Image, parent);
        ctx.scene.a11y_mut(id).role = Role::Group.as_u16();
        ctx.scene.paint_mut(id).primitives.extend([
            Primitive::SolidRect {
                rect: Rect::new(10.0, 10.0, 826.0, 586.0),
                color: TODO_THEME.text,
                corner_radius: 5.0,
            },
            Primitive::SolidRect {
                rect: Rect::new(2.0, 2.0, 826.0, 586.0),
                color: TODO_THEME.surface,
                corner_radius: 5.0,
            },
            Primitive::SolidRect {
                rect: Rect::new(2.0, 2.0, 826.0, 10.0),
                color: TODO_THEME.accent,
                corner_radius: 0.0,
            },
        ]);
        ctx.layout.set_measure(
            id,
            Box::new(|_| Size {
                width: 840.0,
                height: 600.0,
            }),
        );
        id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    DayPlan,
    TaskCompleted,
    TaskAdded,
    CompletedView,
    Cleared,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::DayPlan => "day_plan",
            Self::TaskCompleted => "task_completed",
            Self::TaskAdded => "task_added",
            Self::CompletedView => "completed_view",
            Self::Cleared => "cleared",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "todo", about = "A schnellui todo app")]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 940)]
    width: u32,
    #[arg(long, default_value_t = 700)]
    height: u32,
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    #[arg(long)]
    list: bool,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    out_dir: Option<String>,
    #[arg(long)]
    dump_a11y: Option<String>,
    #[arg(long)]
    assert: bool,
    #[arg(long)]
    windowed: bool,
    /// Run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
}

#[derive(Clone)]
struct Task {
    id: u64,
    title: String,
    area: String,
    done: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Open,
    Completed,
}

#[derive(Clone, Default)]
struct TodoState {
    tasks: Vec<Task>,
    draft: String,
    /// Set by adding a task, so the next mount scrolls the list to it.
    reveal_end: bool,
    next_id: u64,
    filter: Filter,
}

#[derive(Clone)]
struct TodoRuntime(State<TodoRuntimeState>);

#[derive(Default)]
struct TodoRuntimeState {
    todo: TodoState,
    pending_remount: bool,
}

impl TodoRuntime {
    fn seeded() -> Self {
        Self(State::new(TodoRuntimeState {
            todo: TodoState {
                tasks: vec![
                    Task {
                        id: 1,
                        title: "Send the revised brief".into(),
                        area: "WORK".into(),
                        done: true,
                    },
                    Task {
                        id: 2,
                        title: "Book train tickets".into(),
                        area: "ERRAND".into(),
                        done: false,
                    },
                    Task {
                        id: 3,
                        title: "Water the rosemary".into(),
                        area: "HOME".into(),
                        done: false,
                    },
                    Task {
                        id: 4,
                        title: "Review July expenses".into(),
                        area: "MONEY".into(),
                        done: false,
                    },
                    Task {
                        id: 5,
                        title: "Call Mara about Sunday".into(),
                        area: "PEOPLE".into(),
                        done: false,
                    },
                ],
                draft: String::new(),
                reveal_end: false,
                next_id: 6,
                filter: Filter::All,
            },
            pending_remount: false,
        }))
    }

    fn update<R>(&self, update: impl FnOnce(&mut TodoState) -> R) -> R {
        self.0.update(|state| update(&mut state.todo))
    }

    fn request_remount(&self) {
        self.0.update(|state| state.pending_remount = true);
    }

    fn take_remount(&self) -> bool {
        self.0
            .update(|state| std::mem::take(&mut state.pending_remount))
    }
}

fn tasks_for_filter(state: &TodoState) -> Vec<Task> {
    state
        .tasks
        .iter()
        .filter(|task| match state.filter {
            Filter::All => true,
            Filter::Open => !task.done,
            Filter::Completed => task.done,
        })
        .cloned()
        .collect()
}

/// Adds the drafted task, as the Add task button and Enter in the field do.
fn add_draft(runtime: &TodoRuntime) {
    let added = runtime.update(|store| {
        let title = store.draft.trim().to_string();
        if title.is_empty() {
            return false;
        }
        let id = store.next_id;
        store.next_id += 1;
        store.tasks.push(Task {
            id,
            title,
            area: "INBOX".into(),
            done: false,
        });
        store.draft.clear();
        store.reveal_end = true;
        true
    });
    if added {
        runtime.request_remount();
    }
}

fn select_filter(runtime: &TodoRuntime, filter: Filter) {
    runtime.update(|store| store.filter = filter);
    runtime.request_remount();
}

fn todo_view(runtime: TodoRuntime) -> impl View {
    let state = runtime.update(|store| {
        let state = store.clone();
        store.reveal_end = false;
        state
    });
    let completed = state.tasks.iter().filter(|task| task.done).count();
    let open = state.tasks.len() - completed;
    let total = state.tasks.len();
    let visible_tasks = tasks_for_filter(&state);
    let visible_count = visible_tasks.len();

    let mut task_list = Column::new().gap(6.0).width(TASK_ROW_WIDTH);
    if visible_tasks.is_empty() {
        task_list = task_list
            .child(Text::new("NOTHING HERE — A RARE AND EXCELLENT SIGHT.").size(12.0))
            .child(Text::new("Choose another view or capture a new task.").size(15.0));
    }
    for task in visible_tasks {
        let toggle_id = task.id;
        let remove_id = task.id;
        let toggle_runtime = runtime.clone();
        let remove_runtime = runtime.clone();
        let title = Column::new()
            .gap(2.0)
            .child(Text::new(task.title.clone()).size(16.0))
            .child(
                Text::new(if task.done {
                    "FINISHED / NICE WORK"
                } else {
                    "READY WHEN YOU ARE"
                })
                .size(11.0),
            );
        // A finished task steps back: its words take the muted ink.
        let title: Box<dyn View> = if task.done {
            Box::new(ThemeProvider::new(DONE_THEME, title))
        } else {
            Box::new(title)
        };
        task_list = task_list.child(
            Column::new()
                .gap(8.0)
                .width(TASK_ROW_WIDTH)
                .child(
                    Row::new()
                        .width(TASK_ROW_WIDTH)
                        .gap(12.0)
                        .align(Align::Center)
                        .child(
                            Checkbox::new(task.done)
                                .name(format!("Complete {}", task.title))
                                .on_toggle(move |done| {
                                    toggle_runtime.update(|store| {
                                        if let Some(task) =
                                            store.tasks.iter_mut().find(|task| task.id == toggle_id)
                                        {
                                            task.done = done;
                                        }
                                    });
                                    toggle_runtime.request_remount();
                                }),
                        )
                        .child(title)
                        .child(Flex::new().grow(1.0))
                        .child(Badge::new(task.area))
                        .child(
                            Button::new(format!("Remove {}", task.title))
                                .display("Remove")
                                .appearance(ButtonAppearance::Outline)
                                .height(30.0)
                                .text_size(13.0)
                                .on_click(move || {
                                    remove_runtime.update(|store| {
                                        store.tasks.retain(|task| task.id != remove_id)
                                    });
                                    remove_runtime.request_remount();
                                }),
                        ),
                )
                .child(Divider::new()),
        );
    }

    let add_runtime = runtime.clone();
    // The button's height includes its 3px block shadow.
    let add_button = Button::new("Add task")
        .height(ENTRY_HEIGHT + TODO_THEME.shape.shadow)
        .on_click(move || add_draft(&add_runtime));
    let submit_runtime = runtime.clone();

    let clear_runtime = runtime.clone();
    let clear_button = Button::new("Clear completed")
        .appearance(ButtonAppearance::Outline)
        .disabled(completed == 0)
        .on_click(move || {
            clear_runtime.update(|store| store.tasks.retain(|task| !task.done));
            clear_runtime.request_remount();
        });

    let all_runtime = runtime.clone();
    let open_runtime = runtime.clone();
    let completed_runtime = runtime.clone();
    let filters = TabBar::new()
        .gap(4.0)
        .child(
            Tab::new(format!("ALL  {total}"))
                .selected(state.filter == Filter::All)
                .on_select(move || select_filter(&all_runtime, Filter::All)),
        )
        .child(
            Tab::new(format!("OPEN  {open}"))
                .selected(state.filter == Filter::Open)
                .on_select(move || select_filter(&open_runtime, Filter::Open)),
        )
        .child(
            Tab::new(format!("DONE  {completed}"))
                .selected(state.filter == Filter::Completed)
                .on_select(move || select_filter(&completed_runtime, Filter::Completed)),
        );

    let progress = if total == 0 {
        0.0
    } else {
        completed as f32 / total as f32 * 100.0
    };
    let filter_note = match state.filter {
        Filter::All => "THE WHOLE DAY",
        Filter::Open => "WHAT'S NEXT",
        Filter::Completed => "THE GOOD STUFF",
    };

    let sidebar = Column::new()
        .width(200.0)
        .gap(13.0)
        .child(Text::new("RHYTHM").size(11.0))
        .child(Text::new(format!("{progress:.0}%")).size(38.0))
        .child(ProgressBar::new(progress, 0.0, 100.0))
        .child(
            Text::new(format!("{completed} landed  ·  {open} in flight"))
                .size(12.0)
                .role(Role::Status),
        )
        .child(Divider::new())
        .child(Text::new("THE RULE").size(11.0))
        .child(
            Text::new("Make it clear.\nMake it small.\nMake it done.")
                .size(15.0)
                .wrap(schnellui::widgets::WrapMode::Word),
        );

    let task_panel = Column::new().width(540.0).gap(10.0).child(filters).child(
        Scroll::new()
            .size(540.0, 304.0)
            .label("Tasks")
            .scrollbar(true)
            // A task just added lands at the end of the list: show it.
            .follow_end(state.reveal_end)
            // Room for the checkbox focus ring at the list edge.
            .child(Pad::all(4.0).child(task_list)),
    );

    let panel = Column::new()
        .width(772.0)
        .gap(12.0)
        .child(
            Row::new()
                .width(772.0)
                .justify(Justify::SpaceBetween)
                .align(Align::Center)
                .child(
                    Column::new()
                        .gap(3.0)
                        .child(Text::new("DAILY DISPATCH").size(30.0))
                        .child(Text::new("SATURDAY / JUL 25  ·  KEEP THE DAY LIGHT").size(11.0)),
                )
                .child(
                    Column::new()
                        .gap(4.0)
                        .align(Align::End)
                        .child(Text::new("NO. 025").size(11.0))
                        .child(Text::new(format!("{open} OPEN  ·  {total} IN ALL")).size(12.0)),
                ),
        )
        .child(Divider::new())
        .child(
            Row::new()
                .width(772.0)
                .gap(12.0)
                .align(Align::Center)
                .child(
                    TextInput::new(state.draft)
                        .hint("Add a next step…")
                        .width(360.0)
                        .height(ENTRY_HEIGHT)
                        .on_input(move |value| {
                            runtime.update(|store| store.draft = value.to_string())
                        })
                        .on_submit(move || add_draft(&submit_runtime)),
                )
                .child(add_button),
        )
        .child(
            Row::new()
                .width(772.0)
                .gap(28.0)
                .child(sidebar)
                .child(task_panel),
        )
        .child(Divider::new())
        .child(
            Row::new()
                .width(772.0)
                .justify(Justify::SpaceBetween)
                .align(Align::Center)
                .child(
                    Text::new(format!(
                        "{}  /  SHOWING {} OF {}",
                        filter_note, visible_count, total
                    ))
                    .size(11.0),
                )
                .child(clear_button),
        );

    Column::new()
        .fill()
        .align(Align::Center)
        .justify(Justify::Center)
        .child(
            Stack::new()
                .size(840.0, 600.0)
                .child(BoardBackground)
                .child(Pad::all(34.0).child(panel)),
        )
}

fn mount_todo(runtime: TodoRuntime, width: u32, height: u32, scale: f32) -> App {
    let context = Context::new().provide(TODO_THEME).provide(runtime.clone());
    let mut app = App::mount_with_context_size_scaled(
        context,
        |context| todo_view(context.require::<TodoRuntime>()),
        width,
        height,
        scale,
    );
    app.set_clear_color(TODO_THEME.page);
    app
}

fn click(app: &mut App, role: Role, name: &str) {
    let Some(id) = app.find_widget(role, Some(name)) else {
        eprintln!("drive: missing {role:?} named {name:?}");
        return;
    };
    app.dispatch_action(&ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    });
}

fn remount_if_requested(runtime: &TodoRuntime, app: &mut App, width: u32, height: u32, scale: f32) {
    if runtime.take_remount() {
        *app = mount_todo(runtime.clone(), width, height, scale);
    }
}

fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> (TodoRuntime, App) {
    let runtime = TodoRuntime::seeded();
    let mut app = mount_todo(runtime.clone(), width, height, scale);
    match scenario {
        Scenario::DayPlan => {}
        Scenario::TaskCompleted => {
            click(&mut app, Role::CheckBox, "Complete Book train tickets");
            remount_if_requested(&runtime, &mut app, width, height, scale);
        }
        Scenario::TaskAdded => {
            if let Some(id) = app.find_widget(Role::TextInput, Some("Add a next step…")) {
                app.dispatch_action(&ActionRequest {
                    action: Action::SetValue,
                    target_tree: TreeId::ROOT,
                    target_node: to_access_id(id),
                    data: Some(ActionData::Value("Pick up oat milk".into())),
                });
            }
            click(&mut app, Role::Button, "Add task");
            remount_if_requested(&runtime, &mut app, width, height, scale);
        }
        Scenario::CompletedView => {
            click(&mut app, Role::Tab, "DONE  1");
            remount_if_requested(&runtime, &mut app, width, height, scale);
        }
        Scenario::Cleared => {
            click(&mut app, Role::Button, "Clear completed");
            remount_if_requested(&runtime, &mut app, width, height, scale);
        }
    }
    (runtime, app)
}

fn has_status_value(node: &A11yNodeDump, needle: &str) -> bool {
    (node.role == "status"
        && node
            .value
            .as_deref()
            .is_some_and(|value| value.contains(needle)))
        || node
            .children
            .iter()
            .any(|child| has_status_value(child, needle))
}

fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let root = tree.root.as_ref().ok_or("empty a11y tree")?;
    find_by_role_name(&tree, "text_input", Some("Add a next step…")).ok_or("missing task input")?;
    find_by_role_name(&tree, "button", Some("Add task")).ok_or("missing add button")?;
    match scenario {
        Scenario::DayPlan => {
            if !has_status_value(root, "1 landed  ·  4 in flight") {
                return Err("day-plan summary is incorrect".into());
            }
            Ok(())
        }
        Scenario::TaskCompleted => {
            let checkbox =
                find_by_role_name(&tree, "checkbox", Some("Complete Book train tickets"))
                    .ok_or("missing driven task")?;
            if !checkbox.state.iter().any(|state| state == "checked") {
                return Err("driven task is not checked".into());
            }
            if !has_status_value(root, "2 landed  ·  3 in flight") {
                return Err("completed-task summary is incorrect".into());
            }
            Ok(())
        }
        Scenario::TaskAdded => {
            find_by_role_name(&tree, "label", Some("Pick up oat milk"))
                .ok_or("new task is missing")?;
            if !has_status_value(root, "1 landed  ·  5 in flight") {
                return Err("added-task summary is incorrect".into());
            }
            Ok(())
        }
        Scenario::CompletedView => {
            find_by_role_name(&tree, "label", Some("Send the revised brief"))
                .ok_or("completed task is missing")?;
            if find_by_role_name(&tree, "label", Some("Book train tickets")).is_some() {
                return Err("open task is visible in the completed filter".into());
            }
            Ok(())
        }
        Scenario::Cleared => {
            if find_by_role_name(&tree, "label", Some("Send the revised brief")).is_some() {
                return Err("completed task was not cleared".into());
            }
            if !has_status_value(root, "0 landed  ·  4 in flight") {
                return Err("cleared-task summary is incorrect".into());
            }
            Ok(())
        }
    }
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let (_runtime, mut app) = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame();
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
        let dir = cli.out_dir.clone().unwrap_or_else(|| ".".into());
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("could not create out-dir: {error}");
            return ExitCode::FAILURE;
        }
        for scenario in Scenario::iter() {
            let out = format!("{dir}/{}.png", scenario.name());
            if render_one(scenario, &cli, &out) != ExitCode::SUCCESS {
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
        let (width, height, scale) = (cli.width, cli.height, cli.scale);
        let (runtime, mut app) = scenario_app(scenario, width, height, scale);
        app.set_escape_exits(true);
        let remount_runtime = runtime.clone();
        return match app.run_windowed_with("todo", move || {
            remount_runtime
                .take_remount()
                .then(|| mount_todo(remount_runtime.clone(), width, height, scale))
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
