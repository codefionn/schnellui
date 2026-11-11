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
    Align, Badge, Button, ButtonAppearance, Checkbox, Column, Divider, Flex, Form, Justify, Pad,
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
    EmptyTitleRejected,
    CompletedView,
    Cleared,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::DayPlan => "day_plan",
            Self::TaskCompleted => "task_completed",
            Self::TaskAdded => "task_added",
            Self::EmptyTitleRejected => "empty_title_rejected",
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

    let mut task_list = Column::new().gap(6.0).align(Align::Stretch);
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
                .align(Align::Stretch)
                .child(
                    Row::new()
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

    let submit_runtime = runtime.clone();
    // A submit button: with an empty title it focuses the field instead of
    // doing nothing. The button's height includes its 3px block shadow.
    let add_button = Button::new("Add task")
        .height(ENTRY_HEIGHT + TODO_THEME.shape.shadow)
        .submit();

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
            // Rows take the viewport's width: all of it while the list fits, and
            // what the scrollbar's gutter leaves once it overflows.
            .fit_width()
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
            // Enter in the field submits the form too, so the Add button and
            // Enter share one path and one answer to an empty title.
            Form::new()
                .label("New task")
                .width(772.0)
                .on_submit(move || add_draft(&submit_runtime))
                .child(
                    Row::new()
                        .width(772.0)
                        .gap(12.0)
                        .align(Align::Center)
                        .child(
                            TextInput::new(state.draft)
                                .hint("Add a next step…")
                                .required(true)
                                .width(360.0)
                                .height(ENTRY_HEIGHT)
                                .on_input(move |value| {
                                    runtime.update(|store| store.draft = value.to_string())
                                }),
                        )
                        .child(add_button),
                ),
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
        // The board is a fixed size, so a window shorter than it scrolls it: the
        // Scroll sits in the centering column and shrinks to the window, so the
        // board stays centred when there is room.
        .child(
            Scroll::new().child(
                Stack::new()
                    .size(840.0, 600.0)
                    .child(BoardBackground)
                    .child(Pad::all(34.0).child(panel)),
            ),
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

/// The windowed remount hook: a requested remount rebuilds the board at the
/// window's live logical size, not the size the window was launched with.
fn remount_hook(
    runtime: TodoRuntime,
    scale: f32,
) -> impl FnMut(schnellui::scene::Size) -> Option<App> {
    move |viewport| {
        runtime.take_remount().then(|| {
            let (width, height) = (
                viewport.width.round().max(1.0) as u32,
                viewport.height.round().max(1.0) as u32,
            );
            mount_todo(runtime.clone(), width, height, scale)
        })
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
        Scenario::EmptyTitleRejected => {
            // Add with nothing typed: the form blocks it and focuses the field.
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
        Scenario::EmptyTitleRejected => {
            let input = find_by_role_name(&tree, "text_input", Some("Add a next step…"))
                .ok_or("missing task input")?;
            for state in ["required", "invalid"] {
                if !input.state.iter().any(|s| s == state) {
                    return Err(format!("task input is not {state}"));
                }
            }
            if tree.focus != Some(input.id) {
                return Err("the empty task input did not take focus".into());
            }
            if find_by_role_name(&tree, "form", Some("New task")).is_none() {
                return Err("missing the task form".into());
            }
            // Nothing was added.
            if !has_status_value(root, "1 landed  ·  4 in flight") {
                return Err("an empty title changed the task list".into());
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
        let (runtime, mut app) = scenario_app(scenario, cli.width, cli.height, cli.scale);
        app.set_escape_exits(true);
        return match app.run_windowed_with_viewport("todo", remount_hook(runtime, cli.scale)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::testing::fuzz::tab_stops_reachable;

    /// Resize the live window, then complete a task (a remount): the board is
    /// laid out as a fresh mount at the new size lays it out, at either scale.
    #[test]
    fn a_remount_after_a_live_resize_keeps_the_window_size() {
        use schnellui::scene::Size;
        use schnellui::testing::fuzz::{layout_diff, HeadlessHost};
        use schnellui::{Remount, WindowUpdate};
        for scale in [1.0, 1.5] {
            let (runtime, mut app) = scenario_app(Scenario::DayPlan, 880, 640, scale);
            app.frame();
            let mut host = HeadlessHost::new(app);
            let mut hook = remount_hook(runtime, scale);
            host.set_remount(move |size: Size, _| {
                hook(size).map(|app| WindowUpdate::Remount(Remount::new(app, "test")))
            });
            host.resize(700.0, 500.0);
            let id = host
                .app()
                .find_widget(Role::CheckBox, Some("Complete Book train tickets"))
                .expect("the seeded task");
            assert!(host.dispatch_action(&ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: to_access_id(id),
                data: None,
            }));
            assert_eq!(host.remounts(), 1, "completing a task remounts");
            let (_, mut fresh) = scenario_app(Scenario::TaskCompleted, 700, 500, scale);
            assert_eq!(
                layout_diff(host.app_mut(), &mut fresh),
                None,
                "scale {scale}"
            );
        }
    }

    /// Escape exits the window, before and after the board remounts: the
    /// replacement the hook builds does not repeat `set_escape_exits`.
    #[test]
    fn escape_still_exits_after_adding_a_task() {
        use schnellui::scene::Size;
        use schnellui::testing::fuzz::{HeadlessHost, HostKey};
        use schnellui::{Remount, WindowUpdate};
        let (runtime, mut app) = scenario_app(Scenario::DayPlan, 880, 640, 1.0);
        app.set_escape_exits(true);
        app.frame();
        let mut host = HeadlessHost::new(app);
        let mut hook = remount_hook(runtime, 1.0);
        host.set_remount(move |size: Size, _| {
            hook(size).map(|app| WindowUpdate::Remount(Remount::new(app, "test")))
        });
        let input = host
            .app()
            .find_widget(Role::TextInput, Some("Add a next step…"))
            .expect("the task input");
        assert!(host.dispatch_action(&ActionRequest {
            action: Action::Focus,
            target_tree: TreeId::ROOT,
            target_node: to_access_id(input),
            data: None,
        }));
        host.key(&HostKey::Text("Water the ferns".into()), false, false);
        host.key(&HostKey::Enter, false, false);
        assert_eq!(host.remounts(), 1, "adding a task remounts");
        assert!(
            host.app()
                .find_widget(Role::CheckBox, Some("Complete Water the ferns"))
                .is_some(),
            "the task was added"
        );
        assert!(host.press_escape(), "Escape exits after the remount");
    }

    /// At a phone width no text or control is cut at a side with nothing to
    /// scroll it in: prose wraps and fixed-width content reflows or scrolls.
    #[ignore = "the board is a fixed size that cannot reflow, and a Scroll scrolls only down: open design item #12"]
    #[test]
    fn nothing_is_clipped_sideways_at_360() {
        use schnellui::testing::fuzz::sideways_clips;
        let mut faults = Vec::new();
        for scenario in Scenario::iter() {
            let (_runtime, mut app) = scenario_app(scenario, 360, 600, 1.0);
            for fault in sideways_clips(&mut app) {
                faults.push(format!("{}: {fault}", scenario.name()));
            }
        }
        assert!(faults.is_empty(), "{faults:#?}");
    }

    /// The right edge of the task rows in the "Tasks" list, and the viewport's:
    /// `(content_right, viewport_right, bar_left)`. `bar_left` is the viewport's
    /// right edge less the scrollbar's gutter, and `None` while nothing overflows.
    fn task_list_edges(app: &App) -> (f32, f32, Option<f32>) {
        let scene = app.scene();
        let viewport = app
            .find_widget(Role::ScrollView, Some("Tasks"))
            .expect("the task list");
        let right = |id| scene.layout(id).map_or(f32::MIN, |l| l.rect.right());
        // The Remove buttons sit at the right end of their rows.
        let content = scene
            .preorder()
            .into_iter()
            .filter(|&id| {
                scene
                    .a11y(id)
                    .and_then(|a| a.name.as_deref())
                    .is_some_and(|name| name.starts_with("Remove "))
            })
            .map(right)
            .fold(f32::MIN, f32::max);
        let bar = schnellui::testing::fuzz::scrollbars(app)
            .into_iter()
            .find(|bar| bar.viewport == viewport)
            .map(|bar| bar.track.x);
        (content, right(viewport), bar)
    }

    /// An overflowing list ends where the scrollbar's gutter begins, less the
    /// 4px the list keeps for the checkbox focus ring. The list used to stop a
    /// further 14px short, on top of the gutter.
    #[test]
    fn an_overflowing_task_list_runs_up_to_the_scrollbar_gutter() {
        let (runtime, _) = scenario_app(Scenario::DayPlan, 940, 700, 1.0);
        runtime.update(|todo| {
            for id in 6..12 {
                todo.tasks.push(Task {
                    id,
                    title: format!("Extra task {id}"),
                    area: "HOME".into(),
                    done: false,
                });
            }
        });
        let mut app = mount_todo(runtime, 940, 700, 1.0);
        app.frame();
        let (content, viewport, bar) = task_list_edges(&app);
        let bar = bar.expect("eleven tasks overflow the 304px list");
        assert!(
            (viewport - bar - schnellui::layout::SCROLLBAR_GUTTER).abs() <= 1.0,
            "gutter {} vs viewport {viewport}",
            bar
        );
        assert!(
            (content - (bar - 4.0)).abs() <= 1.0,
            "rows end at {content}, the gutter begins at {bar}"
        );
    }

    /// A list that fits has no scrollbar and no strip: rows run to the viewport's
    /// edge less the focus-ring padding.
    #[test]
    fn a_fitting_task_list_runs_to_the_full_width() {
        let (_runtime, mut app) = scenario_app(Scenario::CompletedView, 940, 700, 1.0);
        app.frame();
        let (content, viewport, bar) = task_list_edges(&app);
        assert!(bar.is_none(), "one task fits");
        assert!(
            (content - (viewport - 4.0)).abs() <= 1.0,
            "rows end at {content}, the viewport at {viewport}"
        );
    }

    /// Every Tab stop of every scenario can be shown, at the default size and
    /// in a narrow, short window: something must scroll it in.
    #[test]
    fn every_tab_stop_reaches_the_window() {
        let mut stops = 0;
        for scenario in Scenario::iter() {
            for (w, h) in [(940, 700), (940, 240), (940, 120)] {
                let (_runtime, mut app) = scenario_app(scenario, w, h, 1.0);
                stops += tab_stops_reachable(&mut app)
                    .unwrap_or_else(|e| panic!("{} at {w}x{h}: {e}", scenario.name()));
            }
        }
        assert!(stops > 0, "no scenario has a Tab stop");
    }
}
