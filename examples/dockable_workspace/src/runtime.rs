use super::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkspaceMetrics {
    outer_padding: f32,
    sidebar_width: f32,
    content_height: f32,
    column_gap: f32,
    pub(crate) canvas_width: f32,
    pub(crate) canvas_height: f32,
}

pub(crate) fn workspace_metrics(viewport_width: f32, viewport_height: f32) -> WorkspaceMetrics {
    let outer_padding = (viewport_width.min(viewport_height) * 0.04).clamp(16.0, 24.0);
    let column_gap = (viewport_width * 0.018).clamp(14.0, 24.0);
    let content_width = (viewport_width - outer_padding * 2.0).max(620.0);
    let sidebar_width = (viewport_width * 0.185)
        .clamp(210.0, 260.0)
        .min(content_width * 0.34);
    let content_height = (viewport_height - outer_padding * 2.0).max(420.0);
    let canvas_width = (content_width - sidebar_width - column_gap).max(380.0);
    let canvas_height = (content_height - 104.0).max(300.0);
    WorkspaceMetrics {
        outer_padding,
        sidebar_width,
        content_height,
        column_gap,
        canvas_width,
        canvas_height,
    }
}

pub(crate) fn workspace_view(
    runtime: WorkspaceRuntime,
    viewport_width: f32,
    viewport_height: f32,
) -> impl View {
    let state = runtime.read(Clone::clone);
    let metrics = workspace_metrics(viewport_width, viewport_height);
    let panes = LayoutTreeView::new(
        runtime.clone(),
        state.clone(),
        state.layout.clone(),
        metrics.canvas_width,
        metrics.canvas_height,
    );

    let status = if let Some(tab_id) = state.dragging {
        format!(
            "DRAGGING {}",
            tab_by_id(&state, tab_id).title.to_uppercase()
        )
    } else {
        "CANVAS READY".to_string()
    };

    let main = Column::new()
        .width(metrics.canvas_width)
        .gap(16.0)
        .child(
            Row::new()
                .width(metrics.canvas_width)
                .justify(Justify::SpaceBetween)
                .align(Align::Center)
                .child(
                    Column::new()
                        .gap(2.0)
                        .child(Text::new("MY DAY, MY WAY").size(34.0))
                        .child(Text::new("A modular home screen that moves with you.").size(13.0)),
                )
                .child(Badge::new(status)),
        )
        .child(panes)
        .child(
            Row::new()
                .width(metrics.canvas_width)
                .justify(Justify::SpaceBetween)
                .child(
                    Text::new("PANE EDGE TO SPLIT  ·  CENTER TO JOIN  ·  GUTTER TO INSERT")
                        .size(10.0),
                )
                .child(
                    Text::new(format!("{} PANES  /  EDGE DOCKING ON", state.panes.len()))
                        .size(10.0),
                ),
        );

    Column::new().fill().child(
        Pad::all(metrics.outer_padding).child(
            Row::new()
                .gap(metrics.column_gap)
                .child(sidebar(
                    runtime,
                    &state,
                    metrics.sidebar_width,
                    metrics.content_height,
                ))
                .child(main),
        ),
    )
}

pub(crate) fn mount_workspace(
    runtime: WorkspaceRuntime,
    width: u32,
    height: u32,
    scale: f32,
) -> App {
    let context = Context::new()
        .provide(WORKSPACE_THEME)
        .provide(runtime.clone());
    let mut app = App::mount_with_context_size_scaled(
        context,
        |context| {
            workspace_view(
                context.require::<WorkspaceRuntime>(),
                width as f32,
                height as f32,
            )
        },
        width,
        height,
        scale,
    );
    app.set_clear_color(PAPER);
    app
}

pub(crate) fn point_in(
    app: &App,
    role: Role,
    name: &str,
    x_fraction: f32,
    y_fraction: f32,
) -> Result<Point, String> {
    let id = app
        .find_widget(role, Some(name))
        .ok_or_else(|| format!("missing {role:?} named {name:?}"))?;
    let rect = app
        .scene()
        .layout(id)
        .ok_or_else(|| format!("missing layout for {name:?}"))?
        .rect;
    Ok(Point {
        x: rect.x + rect.width * x_fraction,
        y: rect.y + rect.height * y_fraction,
    })
}

pub(crate) fn drag(
    app: &mut App,
    source: &str,
    target_role: Role,
    target: &str,
    x_fraction: f32,
    y_fraction: f32,
    release: bool,
) -> Result<(), String> {
    let from = point_in(app, Role::Tab, source, 0.5, 0.5)?;
    let to = point_in(app, target_role, target, x_fraction, y_fraction)?;
    if !app.begin_drag(from) {
        return Err(format!("{source:?} did not begin a drag"));
    }
    if !app.update_drag(to) {
        return Err(format!("{target:?} did not become a drop preview"));
    }
    if release && !matches!(app.end_drag(to), DragRelease::Drop { accepted: true }) {
        return Err(format!("{target:?} did not accept the drop"));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
pub(crate) enum Scenario {
    Starter,
    RightPreview,
    BottomPreview,
    SplitRight,
    SplitBottom,
    TabMoved,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Starter => "starter",
            Self::RightPreview => "right_preview",
            Self::BottomPreview => "bottom_preview",
            Self::SplitRight => "split_right",
            Self::SplitBottom => "split_bottom",
            Self::TabMoved => "tab_moved",
        }
    }
}

pub(crate) fn scenario_app(
    runtime: &WorkspaceRuntime,
    scenario: Scenario,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<App, String> {
    let mut app = mount_workspace(runtime.clone(), width, height, scale);
    app.frame();
    match scenario {
        Scenario::Starter => {}
        Scenario::RightPreview => {
            drag(
                &mut app,
                "02 Focus",
                Role::Group,
                "Dock Notes",
                0.94,
                0.5,
                false,
            )?;
        }
        Scenario::BottomPreview => {
            drag(
                &mut app,
                "02 Focus",
                Role::Group,
                "Dock Notes",
                0.5,
                0.94,
                false,
            )?;
        }
        Scenario::TabMoved => {
            drag(&mut app, "01 Pulse", Role::Tab, "03 Notes", 0.5, 0.5, true)?;
            if runtime.take_remount() {
                app = mount_workspace(runtime.clone(), width, height, scale);
                app.frame();
            }
        }
        Scenario::SplitRight | Scenario::SplitBottom => {
            let (x, y) = if scenario == Scenario::SplitRight {
                (0.94, 0.5)
            } else {
                (0.5, 0.94)
            };
            drag(&mut app, "02 Focus", Role::Group, "Dock Notes", x, y, true)?;
            if runtime.take_remount() {
                app = mount_workspace(runtime.clone(), width, height, scale);
                app.frame();
            }
        }
    }
    Ok(app)
}

pub(crate) fn run_assertions(
    runtime: &WorkspaceRuntime,
    scenario: Scenario,
    app: &App,
) -> Result<(), String> {
    for tab in ["01 Pulse", "02 Focus", "03 Notes", "04 Weather"] {
        if app.find_widget(Role::Tab, Some(tab)).is_none() {
            return Err(format!("missing accessible tab {tab:?}"));
        }
    }
    for dock in ["Dock Pulse", "Dock Notes"] {
        if app.find_widget(Role::Group, Some(dock)).is_none()
            && matches!(
                scenario,
                Scenario::Starter | Scenario::RightPreview | Scenario::BottomPreview
            )
        {
            return Err(format!("missing implicit pane target {dock:?}"));
        }
    }
    let expected_panes = if matches!(scenario, Scenario::SplitRight | Scenario::SplitBottom) {
        3
    } else {
        2
    };
    let actual_panes = runtime.read(|store| store.panes.len());
    if actual_panes != expected_panes {
        return Err(format!(
            "expected {expected_panes} panes, found {actual_panes}"
        ));
    }
    if scenario == Scenario::TabMoved {
        let pulse_pane = runtime.read(|store| {
            store
                .panes
                .iter()
                .find(|pane| pane.tabs.contains(&1))
                .map(|pane| pane.id)
        });
        if pulse_pane != Some(2) {
            return Err(format!(
                "Pulse should be docked in pane 2, found {pulse_pane:?}"
            ));
        }
    }
    if matches!(scenario, Scenario::SplitRight | Scenario::SplitBottom) {
        let expected_axis = if scenario == Scenario::SplitRight {
            SplitAxis::Horizontal
        } else {
            SplitAxis::Vertical
        };
        fn has_split(node: &LayoutNode, axis: SplitAxis, first: u64, second: u64) -> bool {
            match node {
                LayoutNode::Split {
                    axis: found,
                    first: left,
                    second: right,
                } => {
                    (*found == axis
                        && **left == LayoutNode::Pane(first)
                        && **right == LayoutNode::Pane(second))
                        || has_split(left, axis, first, second)
                        || has_split(right, axis, first, second)
                }
                LayoutNode::Pane(_) => false,
            }
        }
        let correct_split = runtime.read(|store| has_split(&store.layout, expected_axis, 2, 3));
        if !correct_split {
            return Err(format!(
                "expected pane 3 docked {expected_axis:?} after pane 2"
            ));
        }
    }
    Ok(())
}

#[derive(Parser, Debug)]
#[command(
    name = "dockable_workspace",
    about = "A configurable dockable SchnellUI workspace"
)]
pub(crate) struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 1210)]
    width: u32,
    #[arg(long, default_value_t = 720)]
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
}

pub(crate) fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let runtime = WorkspaceRuntime::default();
    let mut app = match scenario_app(&runtime, scenario, cli.width, cli.height, cli.scale) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("scenario failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    app.frame();
    if cli.assert {
        if let Err(error) = run_assertions(&runtime, scenario, &app) {
            eprintln!("assertion failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    if let Some(path) = &cli.dump_a11y {
        if let Err(error) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    match app.render_to_png(out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("render failed: {error}");
            ExitCode::FAILURE
        }
    }
}

pub fn main() -> ExitCode {
    let cli = Cli::parse();
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
    if cli.windowed {
        let runtime = WorkspaceRuntime::default();
        let (width, height, scale) = (cli.width, cli.height, cli.scale);
        let app = mount_workspace(runtime.clone(), width, height, scale);
        let mut mounted_size = (width, height);
        let remount_runtime = runtime.clone();
        return match app.run_windowed_with_viewport(
            "My Day · Dockable Workspace",
            move |viewport| {
                let current_size = (
                    viewport.width.round().max(1.0) as u32,
                    viewport.height.round().max(1.0) as u32,
                );
                let should_remount = remount_runtime.take_remount() || current_size != mounted_size;
                should_remount.then(|| {
                    mounted_size = current_size;
                    mount_workspace(
                        remount_runtime.clone(),
                        current_size.0,
                        current_size.1,
                        scale,
                    )
                })
            },
        ) {
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
