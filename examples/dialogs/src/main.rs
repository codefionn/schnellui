//! # dialogs — deterministic screenshots for every dialog behavior
//!
//! Every scenario uses the same editorial workspace so the dialog differences
//! stay obvious: viewport-fixed vs canvas-scoped, modal vs modeless, decorated
//! vs undecorated, ordinary dialog vs persistent alert dialog, stacked
//! modal/modeless ownership, and a desktop of independent modeless windows.
//!
//! Run:
//! `cargo run -p dialogs -- --scenario decorated_modal --assert --out dialog.png`
//! `cargo run -p dialogs -- --all --assert --out-dir shots/dialogs`

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y;
use schnellui::layout::Justify;
use schnellui::scene::{Color, Scene, WidgetId, WidgetKind};
use schnellui::view;
use schnellui::widgets::{
    Align, Badge, Button, Column, Dialog, DialogPosition, Divider, Image, Pad, Row, Shape, Spacer,
    Stack, Text, Theme, View, WrapMode,
};
use schnellui::{App, Context, State};
use schnellui_testing::find_by_role_name;
use strum::IntoEnumIterator;

/// A print-editorial palette: green cutting mat, warm paper, vermilion proof marks.
const EDITORIAL_THEME: Theme = Theme {
    text: Color::rgb(0x18, 0x22, 0x1c),
    text_muted: Color::rgb(0x5a, 0x6b, 0x61),
    surface: Color::rgb(0xff, 0xfc, 0xf2),
    surface_muted: Color::rgb(0xe5, 0xe9, 0xdf),
    separator: Color::rgb(0x9e, 0xaa, 0xa1),
    outline: Color::rgb(0x18, 0x22, 0x1c),
    accent: Color::rgb(0xc6, 0x49, 0x31),
    on_accent: Color::rgb(0xff, 0xfc, 0xf2),
    selection: Color::rgb(0xf1, 0xd8, 0xc9),
    interactions: schnellui::widgets::InteractionStates {
        hover: schnellui::widgets::InteractionStyle::all(
            Color::rgba(0xc6, 0x49, 0x31, 0x1c),
            Color::rgb(0x18, 0x22, 0x1c),
            Color::rgb(0xc6, 0x49, 0x31),
        ),
        focus: schnellui::widgets::InteractionStyle::border(Color::rgb(0xc6, 0x49, 0x31)),
        active: schnellui::widgets::InteractionStyle::background(Color::rgb(0xf1, 0xd8, 0xc9)),
    },
    component_interactions: schnellui::widgets::ComponentInteractions::NONE,
    text_selection: Color::rgb(0xed, 0xb8, 0x9e),
    disabled: Color::rgb(0x8e, 0x99, 0x91),
    positive: Color::rgb(0x2d, 0x74, 0x4e),
    attention: Color::rgb(0xd7, 0x9a, 0x2d),
    media: Color::rgb(0xb8, 0xc2, 0xba),
    page: Color::rgb(0x8d, 0xa2, 0x95),
    shape: Shape {
        roundness: 0.45,
        density: 1.15,
        frame: 1.0,
        shadow: 5.0,
    },
};

const DESKTOP_ISSUES: u8 = 0b001;
const DESKTOP_PROOF: u8 = 0b010;
const DESKTOP_LOG: u8 = 0b100;
const DESKTOP_ALL_WINDOWS: u8 = DESKTOP_ISSUES | DESKTOP_PROOF | DESKTOP_LOG;

#[derive(Clone)]
struct DialogRuntime(State<DialogState>);

struct DialogState {
    stacked_modal_open: bool,
    stacked_remount_pending: bool,
    desktop_windows: u8,
    desktop_remount_pending: bool,
}

impl Default for DialogRuntime {
    fn default() -> Self {
        Self(State::new(DialogState {
            stacked_modal_open: true,
            stacked_remount_pending: false,
            desktop_windows: DESKTOP_ALL_WINDOWS,
            desktop_remount_pending: false,
        }))
    }
}

impl DialogRuntime {
    fn set_stacked_modal_open(&self, open: bool) {
        self.0.update(|state| {
            if state.stacked_modal_open != open {
                state.stacked_modal_open = open;
                state.stacked_remount_pending = true;
            }
        });
    }

    fn set_desktop_window(&self, mask: u8, open: bool) {
        self.0.update(|state| {
            let new = if open {
                state.desktop_windows | mask
            } else {
                state.desktop_windows & !mask
            };
            if new != state.desktop_windows {
                state.desktop_windows = new;
                state.desktop_remount_pending = true;
            }
        });
    }

    fn snapshot(&self) -> (bool, u8) {
        self.0
            .read(|state| (state.stacked_modal_open, state.desktop_windows))
    }

    fn take_remount(&self) -> bool {
        self.0.update(|state| {
            let pending = state.stacked_remount_pending || state.desktop_remount_pending;
            state.stacked_remount_pending = false;
            state.desktop_remount_pending = false;
            pending
        })
    }

    #[cfg(test)]
    fn reset_desktop(&self) {
        self.0.update(|state| {
            state.desktop_windows = DESKTOP_ALL_WINDOWS;
            state.desktop_remount_pending = false;
        });
    }

    #[cfg(test)]
    fn desktop_windows(&self) -> u8 {
        self.0.read(|state| state.desktop_windows)
    }
}

fn close_stacked_modal(runtime: &DialogRuntime) {
    runtime.set_stacked_modal_open(false);
}

fn open_stacked_modal(runtime: &DialogRuntime) {
    runtime.set_stacked_modal_open(true);
}

fn set_desktop_window(runtime: &DialogRuntime, mask: u8, open: bool) {
    runtime.set_desktop_window(mask, open);
}

fn open_issues(runtime: &DialogRuntime) {
    set_desktop_window(runtime, DESKTOP_ISSUES, true);
}

fn close_issues(runtime: &DialogRuntime) {
    set_desktop_window(runtime, DESKTOP_ISSUES, false);
}

fn open_proof(runtime: &DialogRuntime) {
    set_desktop_window(runtime, DESKTOP_PROOF, true);
}

fn close_proof(runtime: &DialogRuntime) {
    set_desktop_window(runtime, DESKTOP_PROOF, false);
}

fn open_log(runtime: &DialogRuntime) {
    set_desktop_window(runtime, DESKTOP_LOG, true);
}

fn close_log(runtime: &DialogRuntime) {
    set_desktop_window(runtime, DESKTOP_LOG, false);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// Default viewport-fixed modal with title chrome.
    DecoratedModal,
    /// A chrome-free, top-positioned command palette.
    UndecoratedModal,
    /// A right-aligned inspector that leaves the canvas interactive.
    ModelessInspector,
    /// A modal whose scrim and positioning stay inside the workspace canvas.
    ScopedNonFixed,
    /// Persistent urgent dialog with `AlertDialog` semantics.
    PersistentAlert,
    /// A focus-grabbing modal above a later-declared modeless peer.
    StackedDialogs,
    /// Three independently closable modeless windows over a desktop workspace.
    DesktopWorkspace,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::DecoratedModal => "decorated_modal",
            Scenario::UndecoratedModal => "undecorated_modal",
            Scenario::ModelessInspector => "modeless_inspector",
            Scenario::ScopedNonFixed => "scoped_non_fixed",
            Scenario::PersistentAlert => "persistent_alert",
            Scenario::StackedDialogs => "stacked_dialogs",
            Scenario::DesktopWorkspace => "desktop_workspace",
        }
    }

    fn dialog_name(self) -> &'static str {
        match self {
            Scenario::DecoratedModal => "Publish edition",
            Scenario::UndecoratedModal => "Quick command",
            Scenario::ModelessInspector => "Page inspector",
            Scenario::ScopedNonFixed => "Canvas note",
            Scenario::PersistentAlert => "Unsaved proof",
            Scenario::StackedDialogs => "Final approval",
            Scenario::DesktopWorkspace => "Issue board",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "dialogs", about = "schnellui dialog screenshot gallery")]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 900)]
    width: u32,
    #[arg(long, default_value_t = 620)]
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
    manifest: Option<String>,
    #[arg(long)]
    dump_a11y: Option<String>,
    #[arg(long)]
    assert: bool,
    #[arg(long)]
    windowed: bool,
}

/// The shared fake publishing workspace behind every dialog.
fn workspace(dialog: impl View, width: u32, height: u32) -> impl View {
    let canvas_width = (width as f32 - 72.0).max(420.0);
    let canvas_height = (height as f32 - 128.0).max(300.0);

    Column::new()
        .fill()
        .child(
            Pad::all(24.0).child(
                Column::new()
                    .align(Align::Center)
                    .gap(14.0)
                    .child(
                        Row::new()
                            .width(canvas_width)
                            .align(Align::Center)
                            .gap(10.0)
                            .child(Badge::new("FIELD NOTES / 07"))
                            .child(Text::new("Linden Press").size(22.0))
                            .child(Spacer::new())
                            .child(Text::new("Friday, 16:40").size(13.0)),
                    )
                    .child(
                        Stack::new()
                            .size(canvas_width, canvas_height)
                .child(
                    Image::new("editorial workspace")
                        .alt("muted workspace canvas")
                        .size(canvas_width, canvas_height),
                )
                .child(
                    Pad::all(24.0).child(
                        Column::new()
                            .gap(16.0)
                            .child(
                                Row::new()
                                    .align(Align::Center)
                                    .gap(10.0)
                                    .child(Text::new("Edition 04").size(28.0))
                                    .child(Badge::new("IN REVIEW"))
                                    .child(Spacer::new())
                                    .child(Button::new("Share proof")),
                            )
                            .child(Divider::new())
                            .child(
                                Row::new()
                                    .gap(28.0)
                                    .child(
                                        Column::new()
                                            .gap(7.0)
                                            .child(Text::new("OPEN NOTES").size(12.0))
                                            .child(Text::new("12").size(30.0))
                                            .child(Text::new("3 require response").size(13.0)),
                                    )
                                    .child(
                                        Column::new()
                                            .gap(7.0)
                                            .child(Text::new("PAGES READY").size(12.0))
                                            .child(Text::new("18 / 24").size(30.0))
                                            .child(Text::new("last proof 16:32").size(13.0)),
                                    )
                                    .child(
                                        Column::new()
                                            .gap(7.0)
                                            .child(Text::new("INK COVERAGE").size(12.0))
                                            .child(Text::new("87%").size(30.0))
                                            .child(Text::new("within target").size(13.0)),
                                    ),
                            )
                            .child(Divider::new())
                            .child(Text::new(
                                "The field guide pairs quiet observation with decisive marks.",
                            ))
                            .child(Text::new(
                                "Proofing remains open while editors resolve the final margin notes.",
                            )),
                    ),
                )
                // Fixed dialogs portal from here to the viewport root. A
                // non-fixed dialog deliberately remains bounded by this canvas.
                .child(dialog),
                    ),
            ),
        )
}

fn decorated_modal(width: u32, height: u32) -> impl View {
    workspace(
        view! {
            dialog(title = "Publish edition", width = 440.0, gap = 14.0) {
                badge { "REVIEW REQUIRED" }
                text(wrap = word) {
                    "Three margin notes remain unresolved. Publish this edition anyway?"
                }
                divider
                row(gap = 10.0) {
                    spacer
                    button { "Keep editing" }
                    button { "Publish changes" }
                }
            }
        },
        width,
        height,
    )
}

fn undecorated_modal(width: u32, height: u32) -> impl View {
    workspace(
        view! {
            dialog(
                title = "Quick command",
                undecorated,
                position = top,
                width = 500.0,
                padding = 18.0,
                gap = 10.0
            ) {
                text(size = 20.0) { "Jump to a publishing task" }
                divider
                button { "Review unresolved notes" }
                button { "Export press-ready PDF" }
                button { "Invite another editor" }
            }
        },
        width,
        height,
    )
}

fn modeless_inspector(width: u32, height: u32) -> impl View {
    workspace(
        view! {
            dialog(
                title = "Page inspector",
                modeless,
                non_fixed,
                position = right,
                width = 300.0,
                padding = 18.0,
                gap = 10.0
            ) {
                badge { "PAGE 18" }
                text { "Trim: 148 × 210 mm" }
                text { "Bleed: 3 mm" }
                text { "Profile: FOGRA39" }
                divider
                button { "Open preflight" }
            }
        },
        width,
        height,
    )
}

fn scoped_non_fixed(width: u32, height: u32) -> impl View {
    workspace(
        view! {
            dialog(
                title = "Canvas note",
                non_fixed,
                position = bottom_left,
                width = 360.0,
                padding = 18.0
            ) {
                badge { "SCOPED MODAL" }
                text(wrap = word) {
                    "This scrim ends at the workspace edge; the page header stays outside it."
                }
                button { "Acknowledge" }
            }
        },
        width,
        height,
    )
}

fn persistent_alert(width: u32, height: u32) -> impl View {
    workspace(
        view! {
            dialog(
                title = "Unsaved proof",
                alert,
                persistent,
                position = center,
                width = 430.0
            ) {
                badge { "ACTION NEEDED" }
                text(wrap = word) {
                    "The press profile changed after your last export. Choose how to continue."
                }
                divider
                row(gap = 10.0) {
                    button { "Discard export" }
                    button { "Rebuild proof" }
                }
            }
        },
        width,
        height,
    )
}

fn stacked_dialogs(runtime: DialogRuntime, width: u32, height: u32, modal_open: bool) -> impl View {
    // The focus-grabbing modal is intentionally declared before the modeless
    // inspector. Explicit dialog stack levels keep the modal visually and
    // semantically above that later peer.
    let mut dialogs = Column::new();
    if modal_open {
        let return_runtime = runtime.clone();
        let approve_runtime = runtime.clone();
        let dismiss_runtime = runtime.clone();
        dialogs = dialogs.child(
            Dialog::new("Final approval")
                .width(420.0)
                .child(Badge::new("FOCUS CAPTURED"))
                .child(
                    Text::new(
                        "Only this top dialog is exposed to assistive technology until it closes.",
                    )
                    .wrap(WrapMode::Word),
                )
                .child(Divider::new())
                .child(
                    Row::new()
                        .gap(10.0)
                        .child(Spacer::new())
                        .child(
                            Button::new("Return")
                                .on_click(move || close_stacked_modal(&return_runtime)),
                        )
                        .child(
                            Button::new("Approve edition")
                                .on_click(move || close_stacked_modal(&approve_runtime)),
                        ),
                )
                // Escape and backdrop clicks use the same structural close path
                // as the visible actions.
                .on_dismiss(move || close_stacked_modal(&dismiss_runtime)),
        );
    }
    dialogs = dialogs.child(
        Dialog::new("Layer notes")
            .modeless()
            .position(DialogPosition::Right)
            .width(280.0)
            .padding(18.0)
            .child(Badge::new("MODELESS PEER"))
            .child(Text::new(
                "This inspector can coexist beside the workspace.",
            ))
            .child(Button::new("Reopen approval").on_click(move || open_stacked_modal(&runtime))),
    );

    workspace(dialogs, width, height)
}

fn desktop_workspace(runtime: DialogRuntime, windows: u8) -> impl View {
    let open_issues_runtime = runtime.clone();
    let open_proof_runtime = runtime.clone();
    let open_log_runtime = runtime.clone();
    let desktop = Pad::all(24.0).child(
        Column::new()
            // The desktop derives both axes from the live viewport. Unlike a
            // startup-time pixel size, this is recomputed by App::resize.
            .fill()
            .align(Align::Stretch)
            .child(
                Row::new()
                    .align(Align::Center)
                    .gap(10.0)
                    .child(Badge::new("LINDEN DESK / 04"))
                    .child(Text::new("Edition control room").size(22.0))
                    .child(Spacer::new())
                    .child(Badge::new("PRESS LINK: ONLINE")),
            )
            .child(
                Pad::all(18.0).child(
                    Row::new()
                        .gap(18.0)
                        .child(
                            Column::new()
                                .width(150.0)
                                .gap(6.0)
                                .child(Badge::new("18 / 24"))
                                .child(Text::new("Pages ready").size(17.0))
                                .child(Text::new("Final signatures pending").size(12.0)),
                        )
                        .child(
                            Column::new()
                                .width(150.0)
                                .gap(6.0)
                                .child(Badge::new("3 NOTES"))
                                .child(Text::new("Open issues").size(17.0))
                                .child(Text::new("Two assigned to proofing").size(12.0)),
                        ),
                ),
            )
            .child(Spacer::new())
            .child(
                Row::new()
                    .justify(Justify::Center)
                    .align(Align::Center)
                    .gap(8.0)
                    .child(
                        Button::new("Open issue board")
                            .on_click(move || open_issues(&open_issues_runtime)),
                    )
                    .child(
                        Button::new("Open proof preview")
                            .on_click(move || open_proof(&open_proof_runtime)),
                    )
                    .child(
                        Button::new("Open press log").on_click(move || open_log(&open_log_runtime)),
                    ),
            ),
    );

    let mut dialogs = Column::new();
    if windows & DESKTOP_ISSUES != 0 {
        let button_runtime = runtime.clone();
        let dismiss_runtime = runtime.clone();
        dialogs = dialogs.child(
            Dialog::new("Issue board")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .at(28.0, 92.0)
                .size(310.0, 210.0)
                .padding(18.0)
                .gap(9.0)
                .child(Badge::new("3 OPEN / 1 BLOCKING"))
                .child(
                    Text::new("Margin note 12 · confirm the caption credit before export.")
                        .wrap(WrapMode::Word),
                )
                .child(Divider::new())
                .child(
                    Row::new().gap(8.0).child(Button::new("Assign")).child(
                        Button::new("Close issue board")
                            .on_click(move || close_issues(&button_runtime)),
                    ),
                )
                .on_dismiss(move || close_issues(&dismiss_runtime)),
        );
    }
    if windows & DESKTOP_PROOF != 0 {
        let button_runtime = runtime.clone();
        let dismiss_runtime = runtime.clone();
        dialogs = dialogs.child(
            Dialog::new("Proof preview")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .at(430.0, 58.0)
                .size(370.0, 235.0)
                .padding(18.0)
                .gap(9.0)
                .child(Badge::new("PAGE 18 / V7"))
                .child(Text::new("FIELD GUIDE").size(25.0))
                .child(
                    Text::new("Quiet observations, decisive marks, and one final image clearance.")
                        .wrap(WrapMode::Word),
                )
                .child(Divider::new())
                .child(
                    Row::new().gap(8.0).child(Button::new("Zoom 100%")).child(
                        Button::new("Close proof preview")
                            .on_click(move || close_proof(&button_runtime)),
                    ),
                )
                .on_dismiss(move || close_proof(&dismiss_runtime)),
        );
    }
    if windows & DESKTOP_LOG != 0 {
        let button_runtime = runtime.clone();
        let dismiss_runtime = runtime.clone();
        dialogs = dialogs.child(
            Dialog::new("Press log")
                .modeless()
                .non_fixed()
                .movable()
                .resizable()
                .at(245.0, 330.0)
                .size(430.0, 170.0)
                .padding(18.0)
                .gap(8.0)
                .child(
                    Row::new()
                        .align(Align::Center)
                        .gap(8.0)
                        .child(Badge::new("LIVE"))
                        .child(Text::new("16:40 · profile validation complete")),
                )
                .child(Text::new("16:42 · waiting for two editorial signatures"))
                .child(Row::new().gap(8.0).child(Spacer::new()).child(
                    Button::new("Close press log").on_click(move || close_log(&button_runtime)),
                ))
                .on_dismiss(move || close_log(&dismiss_runtime)),
        );
    }

    Stack::new()
        .fill()
        .child(desktop)
        // A viewport-filling padded host owns every desktop window. Its content
        // box is re-derived on native window resize, so non-fixed dialog movement
        // and resizing clamp to the current work area rather than the launch size.
        .child(Pad::all(24.0).child(dialogs.fill()))
}

fn scenario_app(
    runtime: &DialogRuntime,
    scenario: Scenario,
    width: u32,
    height: u32,
    scale: f32,
) -> App {
    let mut app = match scenario {
        Scenario::DecoratedModal => App::mount_with_theme_size_scaled(
            EDITORIAL_THEME,
            decorated_modal(width, height),
            width,
            height,
            scale,
        ),
        Scenario::UndecoratedModal => App::mount_with_theme_size_scaled(
            EDITORIAL_THEME,
            undecorated_modal(width, height),
            width,
            height,
            scale,
        ),
        Scenario::ModelessInspector => App::mount_with_theme_size_scaled(
            EDITORIAL_THEME,
            modeless_inspector(width, height),
            width,
            height,
            scale,
        ),
        Scenario::ScopedNonFixed => App::mount_with_theme_size_scaled(
            EDITORIAL_THEME,
            scoped_non_fixed(width, height),
            width,
            height,
            scale,
        ),
        Scenario::PersistentAlert => App::mount_with_theme_size_scaled(
            EDITORIAL_THEME,
            persistent_alert(width, height),
            width,
            height,
            scale,
        ),
        Scenario::StackedDialogs => {
            let modal_open = runtime.snapshot().0;
            let context = Context::new()
                .provide(EDITORIAL_THEME)
                .provide(runtime.clone());
            App::mount_with_context_size_scaled(
                context,
                |context| {
                    stacked_dialogs(
                        context.require::<DialogRuntime>(),
                        width,
                        height,
                        modal_open,
                    )
                },
                width,
                height,
                scale,
            )
        }
        Scenario::DesktopWorkspace => {
            let windows = runtime.snapshot().1;
            let context = Context::new()
                .provide(EDITORIAL_THEME)
                .provide(runtime.clone());
            App::mount_with_context_size_scaled(
                context,
                |context| desktop_workspace(context.require::<DialogRuntime>(), windows),
                width,
                height,
                scale,
            )
        }
    };
    app.set_clear_color(EDITORIAL_THEME.page);
    app
}

fn find_kind(scene: &Scene, root: WidgetId, kind: WidgetKind) -> Option<WidgetId> {
    if scene.node(root).is_some_and(|node| node.kind == kind) {
        return Some(root);
    }
    for &child in &scene.node(root)?.children {
        if let Some(found) = find_kind(scene, child, kind) {
            return Some(found);
        }
    }
    None
}

fn find_name(scene: &Scene, root: WidgetId, name: &str) -> Option<WidgetId> {
    if scene.a11y(root).and_then(|node| node.name.as_deref()) == Some(name) {
        return Some(root);
    }
    for &child in &scene.node(root)?.children {
        if let Some(found) = find_name(scene, child, name) {
            return Some(found);
        }
    }
    None
}

fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let role = if scenario == Scenario::PersistentAlert {
        "alert_dialog"
    } else {
        "dialog"
    };
    let dialog = find_by_role_name(&tree, role, Some(scenario.dialog_name()))
        .ok_or_else(|| format!("missing {role} named {:?}", scenario.dialog_name()))?;
    let modal = dialog.state.iter().any(|state| state == "modal");
    let expected_modal = !matches!(
        scenario,
        Scenario::ModelessInspector | Scenario::DesktopWorkspace
    );
    if modal != expected_modal {
        return Err(format!("modal state was {modal} for {}", scenario.name()));
    }

    // Decorated variants expose a visible title label; the undecorated command
    // palette keeps only the semantic dialog name.
    let visible_title = find_by_role_name(&tree, "label", Some(scenario.dialog_name())).is_some();
    if visible_title != (scenario != Scenario::UndecoratedModal) {
        return Err(format!(
            "visible title state was {visible_title} for {}",
            scenario.name()
        ));
    }

    let root = app
        .scene()
        .root()
        .ok_or_else(|| "scene has no root".to_string())?;
    let layer = find_kind(app.scene(), root, WidgetKind::DialogLayer)
        .ok_or_else(|| "missing dialog layer".to_string())?;
    let layer_rect = app
        .scene()
        .layout(layer)
        .ok_or_else(|| "dialog layer was not laid out".to_string())?
        .rect;
    let is_scoped = matches!(
        scenario,
        Scenario::ModelessInspector | Scenario::ScopedNonFixed | Scenario::DesktopWorkspace
    );
    if is_scoped == (layer_rect.width >= app.size().width - 1.0) {
        return Err(format!(
            "dialog layer width {} does not match scoped={is_scoped}",
            layer_rect.width
        ));
    }

    // Long dialog prose opts into word wrapping. Guard its computed height so a
    // future example edit cannot silently regress to overflowing one-line text.
    let wrapping_copy = match scenario {
        Scenario::DecoratedModal => {
            Some("Three margin notes remain unresolved. Publish this edition anyway?")
        }
        Scenario::ScopedNonFixed => {
            Some("This scrim ends at the workspace edge; the page header stays outside it.")
        }
        Scenario::PersistentAlert => {
            Some("The press profile changed after your last export. Choose how to continue.")
        }
        Scenario::StackedDialogs => {
            Some("Only this top dialog is exposed to assistive technology until it closes.")
        }
        Scenario::DesktopWorkspace => {
            Some("Margin note 12 · confirm the caption credit before export.")
        }
        _ => None,
    };
    if let Some(copy) = wrapping_copy {
        let text = find_name(app.scene(), root, copy)
            .ok_or_else(|| format!("missing wrapping dialog copy {copy:?}"))?;
        let rect = app
            .scene()
            .layout(text)
            .ok_or_else(|| "wrapping dialog copy was not laid out".to_string())?
            .rect;
        if rect.height <= 24.0 {
            return Err(format!(
                "dialog copy did not wrap: height={} for {}",
                rect.height,
                scenario.name()
            ));
        }
    }
    if scenario == Scenario::StackedDialogs {
        if find_by_role_name(&tree, "dialog", Some("Layer notes")).is_some() {
            return Err("modeless peer leaked into the active modal accessibility tree".into());
        }
        let focused = app
            .focused_widget()
            .and_then(|id| app.scene().a11y(id))
            .and_then(|node| node.name.as_deref());
        if focused != Some("Return") {
            return Err(format!(
                "top modal did not grab initial focus; focused={focused:?}"
            ));
        }
    }
    if scenario == Scenario::DesktopWorkspace {
        for name in ["Issue board", "Proof preview", "Press log"] {
            let window = find_by_role_name(&tree, "dialog", Some(name))
                .ok_or_else(|| format!("desktop is missing window {name:?}"))?;
            if window.state.iter().any(|state| state == "modal") {
                return Err(format!("desktop window {name:?} must remain modeless"));
            }
        }
        if app.focused_widget().is_some() {
            return Err("modeless desktop should not grab initial focus".into());
        }
    }
    Ok(())
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let runtime = DialogRuntime::default();
    let mut app = scenario_app(&runtime, scenario, cli.width, cli.height, cli.scale);
    app.frame();

    if let Some(path) = &cli.dump_a11y {
        if let Err(error) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    if cli.assert {
        if let Err(error) = run_assertions(scenario, &app) {
            eprintln!("assertion failed ({}): {error}", scenario.name());
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
        let dir = cli.out_dir.clone().unwrap_or_else(|| ".".to_string());
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("could not create out-dir {dir:?}: {error}");
            return ExitCode::FAILURE;
        }
        let physical_width = (cli.width as f32 * cli.scale).round().max(1.0) as u32;
        let physical_height = (cli.height as f32 * cli.scale).round().max(1.0) as u32;
        let mut manifest = Vec::new();
        for scenario in Scenario::iter() {
            let out = format!("{dir}/{}.png", scenario.name());
            let code = render_one(scenario, &cli, &out);
            if code != ExitCode::SUCCESS {
                return code;
            }
            manifest.push(format!(
                "{{\"scenario\":\"{}\",\"path\":\"{out}\",\"width\":{physical_width},\"height\":{physical_height}}}",
                scenario.name()
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
        let (width, height, scale) = (cli.width, cli.height, cli.scale);
        let runtime = DialogRuntime::default();
        let app = scenario_app(&runtime, scenario, width, height, scale);
        let remount_runtime = runtime.clone();
        return match app.run_windowed_with("schnellui dialogs", move || {
            remount_runtime
                .take_remount()
                .then(|| scenario_app(&remount_runtime, scenario, width, height, scale))
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

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::a11y::accesskit_reexport::{Action, ActionRequest, TreeId};
    use schnellui::UiKey;

    fn click(app: &mut App, name: &str) -> bool {
        let id = app
            .find_widget(a11y::Role::Button, Some(name))
            .unwrap_or_else(|| panic!("missing button {name:?}"));
        app.dispatch_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: a11y::to_access_id(id),
            data: None,
        })
    }

    #[test]
    fn stacked_modal_closes_reveals_its_peer_and_can_be_reopened() {
        let runtime = DialogRuntime::default();
        runtime.set_stacked_modal_open(true);
        let _ = runtime.take_remount();

        let mut open = scenario_app(&runtime, Scenario::StackedDialogs, 900, 620, 1.0);
        open.frame();
        assert!(click(&mut open, "Return"));
        assert!(runtime.take_remount());

        let mut closed = scenario_app(&runtime, Scenario::StackedDialogs, 900, 620, 1.0);
        closed.frame();
        let tree = a11y::dump_tree(closed.scene());
        assert!(find_by_role_name(&tree, "dialog", Some("Final approval")).is_none());
        assert!(find_by_role_name(&tree, "dialog", Some("Layer notes")).is_some());
        assert!(click(&mut closed, "Reopen approval"));
        assert!(runtime.take_remount());

        let mut reopened = scenario_app(&runtime, Scenario::StackedDialogs, 900, 620, 1.0);
        reopened.frame();
        assert!(reopened.dispatch_key(UiKey::Escape));
        assert!(runtime.take_remount());

        // Leave deterministic screenshot state intact for any subsequent test.
        runtime.set_stacked_modal_open(true);
        let _ = runtime.take_remount();
    }

    #[test]
    fn desktop_windows_close_independently_and_reopen_from_the_taskbar() {
        let runtime = DialogRuntime::default();
        runtime.reset_desktop();
        let _ = runtime.take_remount();

        let mut desktop = scenario_app(&runtime, Scenario::DesktopWorkspace, 900, 620, 1.0);
        desktop.frame();
        assert!(click(&mut desktop, "Close issue board"));
        assert!(runtime.take_remount());

        let mut without_issues = scenario_app(&runtime, Scenario::DesktopWorkspace, 900, 620, 1.0);
        without_issues.frame();
        let tree = a11y::dump_tree(without_issues.scene());
        assert!(find_by_role_name(&tree, "dialog", Some("Issue board")).is_none());
        assert!(find_by_role_name(&tree, "dialog", Some("Proof preview")).is_some());
        assert!(find_by_role_name(&tree, "dialog", Some("Press log")).is_some());

        assert!(click(&mut without_issues, "Open issue board"));
        assert!(runtime.take_remount());

        // Escape belongs to the last/topmost modeless window: Press log.
        let mut restored = scenario_app(&runtime, Scenario::DesktopWorkspace, 900, 620, 1.0);
        restored.frame();
        assert!(restored.dispatch_key(UiKey::Escape));
        assert!(runtime.take_remount());
        assert_eq!(
            runtime.desktop_windows(),
            DESKTOP_ISSUES | DESKTOP_PROOF,
            "Escape closes only the top desktop window"
        );

        runtime.reset_desktop();
        let _ = runtime.take_remount();
    }

    #[test]
    fn desktop_dialogs_are_parent_scoped_movable_and_resizable() {
        let runtime = DialogRuntime::default();
        runtime.reset_desktop();
        let mut desktop = scenario_app(&runtime, Scenario::DesktopWorkspace, 900, 620, 1.0);
        desktop.frame();

        let panel = desktop
            .find_widget(a11y::Role::Dialog, Some("Issue board"))
            .expect("issue dialog");
        let layer = desktop
            .scene()
            .node(panel)
            .and_then(|node| node.parent)
            .and_then(|stage| desktop.scene().node(stage).and_then(|node| node.parent))
            .expect("dialog layer");
        assert_ne!(
            desktop.scene().node(layer).and_then(|node| node.parent),
            desktop.scene().root(),
            "desktop dialogs are non-fixed and stay in their workspace parent"
        );

        let before = desktop.scene().layout(panel).unwrap().rect;
        let title_press = schnellui::scene::Point {
            x: before.x + 24.0,
            y: before.y + 20.0,
        };
        assert!(desktop.begin_dialog_pointer(title_press));
        assert!(desktop.update_dialog_pointer(schnellui::scene::Point {
            x: title_press.x + 40.0,
            y: title_press.y - 28.0,
        }));
        assert!(desktop.end_dialog_pointer());
        desktop.frame();
        let moved = desktop.scene().layout(panel).unwrap().rect;
        assert!((moved.x - before.x - 40.0).abs() < 0.1);
        assert!((moved.y - before.y + 28.0).abs() < 0.1);
        assert_eq!((moved.width, moved.height), (before.width, before.height));

        let resize_press = schnellui::scene::Point {
            x: moved.right() - 4.0,
            y: moved.bottom() - 4.0,
        };
        assert!(desktop.begin_dialog_pointer(resize_press));
        assert!(desktop.update_dialog_pointer(schnellui::scene::Point {
            x: resize_press.x + 48.0,
            y: resize_press.y + 36.0,
        }));
        assert!(desktop.end_dialog_pointer());
        desktop.frame();
        let resized = desktop.scene().layout(panel).unwrap().rect;
        assert!(
            (resized.width - moved.width - 48.0).abs() < 0.1,
            "moved={moved:?}, resized={resized:?}"
        );
        assert!(
            (resized.height - moved.height - 36.0).abs() < 0.1,
            "moved={moved:?}, resized={resized:?}"
        );
    }

    #[test]
    fn desktop_workspace_tracks_the_live_window_size() {
        let runtime = DialogRuntime::default();
        runtime.reset_desktop();
        let mut desktop = scenario_app(&runtime, Scenario::DesktopWorkspace, 900, 620, 1.0);
        desktop.frame();

        let panel = desktop
            .find_widget(a11y::Role::Dialog, Some("Issue board"))
            .expect("issue dialog");
        let mut layer = panel;
        while desktop.scene().node(layer).unwrap().kind != WidgetKind::DialogLayer {
            layer = desktop
                .scene()
                .node(layer)
                .and_then(|node| node.parent)
                .expect("dialog layer ancestor");
        }
        let host = desktop.scene().node(layer).unwrap().parent.unwrap();
        let before = desktop.scene().layout(host).unwrap().rect;
        assert_eq!(
            before,
            schnellui::scene::Rect::new(24.0, 24.0, 852.0, 572.0)
        );

        desktop.resize(1120.0, 760.0);
        desktop.frame();
        let after = desktop.scene().layout(host).unwrap().rect;
        assert_eq!(
            after,
            schnellui::scene::Rect::new(24.0, 24.0, 1072.0, 712.0)
        );
        assert_eq!(after.width - before.width, 220.0);
        assert_eq!(after.height - before.height, 140.0);
    }

    #[test]
    fn focused_desktop_dialog_is_foreground_for_paint_and_input() {
        let runtime = DialogRuntime::default();
        fn layer_for(app: &App, mut id: WidgetId) -> WidgetId {
            loop {
                let node = app.scene().node(id).expect("live ancestor");
                if node.kind == WidgetKind::DialogLayer {
                    return id;
                }
                id = node.parent.expect("dialog layer ancestor");
            }
        }

        runtime.reset_desktop();
        let mut desktop = scenario_app(&runtime, Scenario::DesktopWorkspace, 900, 620, 1.0);
        desktop.frame();
        let issue = desktop
            .find_widget(a11y::Role::Dialog, Some("Issue board"))
            .unwrap();
        let proof = desktop
            .find_widget(a11y::Role::Dialog, Some("Proof preview"))
            .unwrap();
        let issue_layer = layer_for(&desktop, issue);
        let proof_layer = layer_for(&desktop, proof);
        assert_eq!(
            desktop.scene().overlay_level(issue_layer),
            desktop.scene().overlay_level(proof_layer),
            "desktop windows share one modeless overlay plane"
        );

        // Move Issue board over Proof preview. Pressing its title raises it first.
        let issue_rect = desktop.scene().layout(issue).unwrap().rect;
        let title = schnellui::scene::Point {
            x: issue_rect.x + 20.0,
            y: issue_rect.y + 18.0,
        };
        assert!(desktop.begin_dialog_pointer(title));
        assert!(desktop.update_dialog_pointer(schnellui::scene::Point {
            x: title.x + 344.0,
            y: title.y + 50.0,
        }));
        desktop.end_dialog_pointer();
        desktop.frame();
        assert!(
            desktop.scene().overlay_order(issue_layer) > desktop.scene().overlay_order(proof_layer)
        );

        let issue_rect = desktop.scene().layout(issue).unwrap().rect;
        let proof_rect = desktop.scene().layout(proof).unwrap().rect;
        let overlap = issue_rect.intersect(&proof_rect);
        assert!(!overlap.is_empty(), "test windows overlap after drag");
        let overlap_point = schnellui::scene::Point {
            x: overlap.x + overlap.width * 0.5,
            y: overlap.y + overlap.height * 0.5,
        };
        let hit = desktop.hit_test(overlap_point).unwrap();
        assert_eq!(layer_for(&desktop, hit), issue_layer);

        // Focus in the obscured peer raises that whole window. Input at the same
        // overlap now resolves into Proof preview, matching the visual foreground.
        let zoom = desktop
            .find_widget(a11y::Role::Button, Some("Zoom 100%"))
            .unwrap();
        assert!(desktop.focus(Some(zoom)));
        assert!(
            desktop.scene().overlay_order(proof_layer) > desktop.scene().overlay_order(issue_layer)
        );
        let hit = desktop.hit_test(overlap_point).unwrap();
        assert_eq!(layer_for(&desktop, hit), proof_layer);

        // Issue board's title bar now sits partly beneath Proof preview's body.
        // Covered chrome must not steal the drag from the foreground pixel.
        let covered_issue_title = schnellui::scene::Point {
            x: proof_rect.x + 24.0,
            y: issue_rect.y + 18.0,
        };
        assert_ne!(
            desktop.cursor_at(covered_issue_title),
            schnellui::widgets::CursorIcon::Grab,
            "covered title chrome must not leak its grab cursor through the foreground dialog"
        );
        assert!(!desktop.begin_dialog_pointer(covered_issue_title));

        // Its still-visible title-bar strip is clickable, raises the window, and
        // then captures movement normally.
        let visible_issue_title = schnellui::scene::Point {
            x: issue_rect.x + 18.0,
            y: issue_rect.y + 18.0,
        };
        assert_eq!(
            desktop.cursor_at(visible_issue_title),
            schnellui::widgets::CursorIcon::Grab
        );
        assert!(desktop.begin_dialog_pointer(visible_issue_title));
        assert_eq!(
            desktop.cursor_at(visible_issue_title),
            schnellui::widgets::CursorIcon::Grabbing
        );
        assert!(desktop.end_dialog_pointer());
        assert!(
            desktop.scene().overlay_order(issue_layer) > desktop.scene().overlay_order(proof_layer)
        );
        let hit = desktop.hit_test(overlap_point).unwrap();
        assert_eq!(layer_for(&desktop, hit), issue_layer);
    }
}
