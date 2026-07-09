//! Native-DOM component gallery and driven screenshot scenario.
//!
//! Every public widget-facing component is represented through the generic
//! template seam. Chromium dispatches semantic DOM events back into Rust, Rust
//! callbacks update signals, and the expected state is diffed into the live DOM
//! before the screenshot.

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::process::ExitCode;

#[cfg(not(target_arch = "wasm32"))]
use clap::{Parser, ValueEnum};
use schnellui_render_html::HtmlRenderer;
use schnellui_signal::{create_signal, Signal};
#[cfg(not(target_arch = "wasm32"))]
use schnellui_template::DriveAction;
use schnellui_template::{
    badge, column, dialog, divider, dock_area, drag_handle, dropdown, dropdown_option,
    grouped_tab_list, icon, image, link, list, list_item, loading_spinner, progress_bar, radio,
    rich_text, row, scroll, stack, svg, switch, tab, tab_bar, tab_group, tab_node, table,
    table_row, text_area, theme_provider, Button, Checkbox, Flex, Pad, Role, Slider, Spacer, Text,
    TextInput,
};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    #[default]
    Initial,
    Driven,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Parser)]
#[command(
    name = "html_native",
    about = "complete native HTML widget gallery rendered and driven by Chromium"
)]
struct Cli {
    #[arg(long, default_value = "html-native.png")]
    out: PathBuf,
    #[arg(long, default_value_t = 1180)]
    width: u32,
    #[arg(long, default_value_t = 980)]
    height: u32,
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    #[arg(long, value_enum, default_value_t)]
    scenario: Scenario,
    #[arg(long)]
    list: bool,
    /// Optional Chrome/Chromium executable; automatic discovery is the default.
    #[arg(long)]
    chrome: Option<PathBuf>,
}

#[derive(Clone, Copy)]
struct GalleryState {
    clicks: Signal<i32>,
    counter: Signal<i32>,
    enabled: Signal<bool>,
    level: Signal<f32>,
    name: Signal<String>,
    notes: Signal<String>,
}

impl GalleryState {
    fn new() -> Self {
        Self {
            clicks: create_signal(0),
            counter: create_signal(0),
            enabled: create_signal(true),
            level: create_signal(42.0),
            name: create_signal("Ada".to_string()),
            notes: create_signal("Native text area".to_string()),
        }
    }
}

/// A deliberately shape-changing subtree that exercises keyed DOM insertion.
struct CallbackHistory(Signal<i32>);

impl schnellui_template::Template for CallbackHistory {
    fn render<R: schnellui_template::TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        if self.0.get() > 0 {
            schnellui_template::Template::render(
                list()
                    .label("DOM diff history")
                    .child(list_item().label("Inserted after Rust callback"))
                    .child(list_item().label("Stable node")),
                renderer,
            )
        } else {
            schnellui_template::Template::render(
                list()
                    .label("DOM diff history")
                    .child(list_item().label("Stable node")),
                renderer,
            )
        }
    }
}

fn card<C: schnellui_template::TemplateChildren>(
    title: &'static str,
    contents: schnellui_template::Container<C>,
) -> impl schnellui_template::Template {
    column()
        .gap(9.0)
        .child(Text::new(title).size(19.0))
        .child(contents)
}

fn example_view(state: GalleryState) -> impl schnellui_template::Template {
    let click_state = state.clicks;
    let counter_state = state.counter;
    let checkbox_state = state.enabled;
    let slider_state = state.level;
    let input_state = state.name;
    let switch_state = state.enabled;
    let radio_state = state.enabled;
    let drag_state = state.clicks;
    let tab_state = state.clicks;
    let list_state = state.clicks;
    let dropdown_state = state.name;
    let area_state = state.notes;
    let table_state = state.clicks;

    Pad::all(20.0).child(scroll().fill().child(
        column()
            .gap(18.0)
            .child(
                row()
                    .gap(12.0)
                    .child(icon().label("SchnellUI").detail("S"))
                    .child(
                        column()
                            .child(Text::new("Native HTML — complete widget gallery").size(28.0))
                            .child(Text::new(
                                "Semantic elements, Rust callbacks, Chromium scenario driving; no canvas.",
                            )),
                    )
                    .child(Spacer::new())
                    .child(badge().label("39 / 39")),
            )
            .child(divider())
            .child(
                row()
                    .gap(18.0)
                    .wrap()
                    .child(card(
                        "Base controls",
                        column()
                            .gap(9.0)
                            .child(
                                row()
                                    .gap(9.0)
                                    .child(
                                        Button::new("Run callback").on_click(move || {
                                            click_state.update(|value| *value += 1)
                                        }),
                                    )
                                    .child(
                                        Checkbox::new(state.enabled.get())
                                            .name("Native checkbox")
                                            .on_toggle(move |value| checkbox_state.set(value)),
                                    )
                                    .child(Text::new("Native checkbox")),
                            )
                            .child(
                                row()
                                    .gap(9.0)
                                    .child(
                                        Button::new("Increment counter").on_click(move || {
                                            counter_state.update(|value| *value += 1)
                                        }),
                                    )
                                    .child(
                                        Text::dynamic(move || {
                                            format!("Counter: {}", state.counter.get())
                                        })
                                        .role(Role::Status),
                                    ),
                            )
                            .child(
                                Slider::new(state.level.get(), 0.0, 100.0)
                                    .name("Level")
                                    .step(1.0)
                                    .on_change(move |value| slider_state.set(value)),
                            )
                            .child(
                                TextInput::new(state.name.get())
                                    .label("Name")
                                    .on_input(move |value| input_state.set(value.to_string())),
                            )
                            .child(
                                row()
                                    .gap(8.0)
                                    .child(Flex::new().grow(1.0).child(Text::new(
                                        "Flex child occupies available room",
                                    )))
                                    .child(stack().size(110.0, 28.0).child(Text::new(
                                        "Stack overlay",
                                    ))),
                            ),
                    ))
                    .child(card(
                        "Status and choices",
                        column()
                            .gap(10.0)
                            .child(
                                progress_bar()
                                    .label("Build progress")
                                    .number(state.level.get())
                                    .range(0.0, 100.0),
                            )
                            .child(
                                row()
                                    .gap(12.0)
                                    .child(loading_spinner().label("Loading"))
                                    .child(
                                        switch()
                                            .label("Live updates")
                                            .checked(state.enabled.get())
                                            .on_toggle(move |value| switch_state.set(value)),
                                    )
                                    .child(
                                        radio()
                                            .label("Primary choice")
                                            .selected(state.enabled.get())
                                            .on_click(move || radio_state.set(true)),
                                    ),
                            )
                            .child(
                                row()
                                    .gap(10.0)
                                    .child(
                                        link()
                                            .label("Documentation")
                                            .value("#docs")
                                            .on_click(move || {
                                                click_state.update(|value| *value += 1)
                                            }),
                                    )
                                    .child(
                                        drag_handle()
                                            .label("Reorder")
                                            .on_click(move || {
                                                drag_state.update(|value| *value += 1)
                                            }),
                                    ),
                            ),
                    ))
                    .child(card(
                        "Selection",
                        column()
                            .gap(10.0)
                            .child(
                                tab_bar()
                                    .label("Views")
                                    .child(
                                        tab()
                                            .label("Overview")
                                            .selected(true)
                                            .on_click(move || {
                                                tab_state.update(|value| *value += 1)
                                            }),
                                    )
                                    .child(tab().label("Details")),
                            )
                            .child(
                                list()
                                    .label("Files")
                                    .child(
                                        list_item()
                                            .label("README.md")
                                            .selected(true)
                                            .on_click(move || {
                                                list_state.update(|value| *value += 1)
                                            }),
                                    )
                                    .child(list_item().label("Cargo.toml")),
                            )
                            .child(
                                dropdown()
                                    .label("Renderer")
                                    .on_input(move |value| {
                                        dropdown_state.set(value.to_string())
                                    })
                                    .child(
                                        dropdown_option()
                                            .label("Native HTML")
                                            .value("Native HTML")
                                            .selected(true),
                                    )
                                    .child(
                                        dropdown_option()
                                            .label("WGPU")
                                            .value("WGPU"),
                                    ),
                            ),
                    )),
            )
            .child(
                row()
                    .gap(18.0)
                    .wrap()
                    .child(card(
                        "Structured navigation",
                        column()
                            .gap(8.0)
                            .child(
                                grouped_tab_list()
                                    .label("Workspace")
                                    .child(
                                        tab_group()
                                            .label("Project")
                                            .child(
                                                tab_node()
                                                    .label("Sources")
                                                    .child(tab().label("main.rs")),
                                            ),
                                    ),
                            )
                            .child(
                                dock_area()
                                    .label("Editor dock")
                                    .child(Text::new("Dock target: center")),
                            ),
                    ))
                    .child(card(
                        "Data and documents",
                        column()
                            .gap(10.0)
                            .child(
                                table()
                                    .label("Build matrix")
                                    .child(
                                        table_row()
                                            .label("Header")
                                            .item("Backend")
                                            .item("Status"),
                                    )
                                    .child(
                                        table_row()
                                            .label("HTML row")
                                            .item("HTML")
                                            .item("Passing")
                                            .selected(true)
                                            .on_click(move || {
                                                table_state.update(|value| *value += 1)
                                            }),
                                    ),
                            )
                            .child(
                                rich_text()
                                    .label("RichText")
                                    .value("Formatted document content rendered as a native article."),
                            )
                            .child(
                                text_area()
                                    .label("Notes")
                                    .detail("Write notes")
                                    .value(state.notes.get())
                                    .on_input(move |value| area_state.set(value.to_string())),
                            ),
                    ))
                    .child(card(
                        "Media and theming",
                        column()
                            .gap(10.0)
                            .child(
                                row()
                                    .gap(10.0)
                                    .child(
                                        image()
                                            .label("Gradient sample")
                                            .value("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='120' height='64'%3E%3Crect width='120' height='64' rx='8' fill='%235b5bd6'/%3E%3C/svg%3E")
                                            .size(120.0, 64.0),
                                    )
                                    .child(
                                        svg()
                                            .label("Native vector")
                                            .value("<svg viewBox=\"0 0 64 64\" aria-hidden=\"true\"><circle cx=\"32\" cy=\"32\" r=\"26\" fill=\"#5b5bd6\"/><path d=\"M19 33l8 8 18-20\" fill=\"none\" stroke=\"white\" stroke-width=\"6\"/></svg>")
                                            .size(64.0, 64.0),
                                    ),
                            )
                            .child(
                                theme_provider()
                                    .value("dark")
                                    .child(Text::new("ThemeProvider subtree")),
                            ),
                    )),
            )
            .child(
                dialog()
                    .label("Native dialog")
                    .child(Text::new("Dialog content remains part of normal gallery flow.")),
            )
            .child(CallbackHistory(state.clicks))
            .child(Text::dynamic(move || {
                format!(
                    "Rust state — callbacks: {}, enabled: {}, level: {:.0}, name: {}, notes: {}",
                    state.clicks.get(),
                    state.enabled.get(),
                    state.level.get(),
                    state.name.get(),
                    state.notes.get()
                )
            }).role(Role::Status)),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn driven_actions() -> Vec<DriveAction> {
    vec![
        DriveAction::click(Role::Button, "Run callback"),
        DriveAction::click(Role::Button, "Increment counter"),
        DriveAction::click(Role::CheckBox, "Native checkbox"),
        DriveAction::set_value(Role::Slider, "Level", "73"),
        DriveAction::set_value(Role::TextInput, "Name", "Driven by Chromium"),
        DriveAction::set_value(
            Role::MultilineTextInput,
            "Notes",
            "Browser event reached Rust",
        ),
    ]
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.list {
        println!("initial\ndriven");
        return ExitCode::SUCCESS;
    }

    let state = GalleryState::new();
    let mut renderer = HtmlRenderer::new(cli.width, cli.height).with_scale(cli.scale);
    if let Some(chrome) = cli.chrome {
        renderer.set_chrome_executable(chrome);
    }
    let actions = match cli.scenario {
        Scenario::Initial => Vec::new(),
        Scenario::Driven => driven_actions(),
    };
    match renderer
        .render_scenario(|| example_view(state), &actions, &cli.out)
        .await
    {
        Ok(_) => {
            println!("wrote {}", cli.out.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("HTML render failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    let width = js_sys::eval("Math.max(1, Math.floor(window.innerWidth))")?
        .as_f64()
        .unwrap_or(1180.0) as u32;
    let height = js_sys::eval("Math.max(1, Math.floor(window.innerHeight))")?
        .as_f64()
        .unwrap_or(980.0) as u32;
    let state = GalleryState::new();
    let mount = HtmlRenderer::new(width, height).mount(move || example_view(state))?;
    mount.forget();
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(test)]
mod tests {
    use schnellui_template::ComponentKind;

    use super::*;

    #[test]
    fn gallery_covers_every_registered_widget_kind_without_canvas() {
        let html = HtmlRenderer::new(1180, 980)
            .render(example_view(GalleryState::new()))
            .into_string();
        for kind in ComponentKind::ALL {
            let marker = format!(r#"data-sui-component="{}""#, kind.as_str());
            assert!(
                html.contains(&marker),
                "gallery is missing {}",
                kind.as_str()
            );
        }
        assert!(html.contains("Increment counter"));
        assert!(html.contains("Counter: 0"));
        assert!(!html.contains("<canvas"));
    }
}
