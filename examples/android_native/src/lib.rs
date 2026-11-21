//! Native Android views from a schnellui template.
//!
//! A gallery of the base component set. Every control is a real
//! `android.widget` view and every value lives in a signal. Build the `cdylib`
//! for an Android target and load it from the `android/` demo app. The same
//! view runs on the host through [`AndroidRenderer`], which is how the tests
//! here drive it.

use std::time::Duration;

use schnellui::render_android::AndroidRenderer;
use schnellui::signal::{create_signal, Signal};
use schnellui::template::{
    badge, column, dialog, divider, drag_handle, dropdown, icon, image, link, list, list_item,
    loading_spinner, progress_bar, radio, rich_text, row, scroll, svg, switch, tab, tab_bar, table,
    table_row, text_area, theme_provider, Align, Button, Checkbox, ComponentRef, ContainerKind,
    ContainerProps, Pad, ResponsiveQuery, Slider, Spacer, Template, TemplateRenderer, Text,
    TextInput,
};

/// A 1 by 1 red PNG, scaled up by the image view.
const PIXEL_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

/// A red square with a blue disc, rasterised by Rust into a PNG.
const BADGE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 48 48"><rect x="2" y="2" width="44" height="44" rx="8" fill="#d32f2f"/><circle cx="24" cy="24" r="12" fill="#1976d2"/></svg>"##;

/// Width of the box a size query looks at, when it is narrow and when it is wide.
const NARROW: f32 = 120.0;
const WIDE: f32 = 260.0;
/// The text of a query shows from this content width on.
const SHOW_FROM: f32 = 200.0;

const COLORS: [&str; 3] = ["Red", "Green", "Blue"];
const TABS: [&str; 3] = ["One", "Two", "Three"];
const FRUITS: [&str; 3] = ["Apple", "Pear", "Plum"];

/// All state of the gallery.
#[derive(Clone, Copy)]
pub struct Gallery {
    clicks: Signal<i32>,
    enabled: Signal<bool>,
    level: Signal<f32>,
    name: Signal<String>,
    order: Signal<Vec<u32>>,
    wifi: Signal<bool>,
    choice: Signal<usize>,
    color: Signal<String>,
    notes: Signal<String>,
    dialog_open: Signal<bool>,
    tab: Signal<usize>,
    fruit: Signal<usize>,
    row_picked: Signal<bool>,
    themed_on: Signal<bool>,
    parent_wide: Signal<bool>,
    component_wide: Signal<bool>,
    seconds: Signal<u32>,
    /// The reference the component query below looks for.
    target: ComponentRef,
}

impl Gallery {
    pub fn new() -> Self {
        Self {
            clicks: create_signal(0),
            enabled: create_signal(true),
            level: create_signal(40.0),
            name: create_signal("Ada".to_string()),
            order: create_signal(vec![1, 2, 3, 4]),
            wifi: create_signal(false),
            choice: create_signal(0),
            color: create_signal("Green".to_string()),
            notes: create_signal(String::new()),
            dialog_open: create_signal(false),
            tab: create_signal(0),
            fruit: create_signal(1),
            row_picked: create_signal(false),
            themed_on: create_signal(true),
            parent_wide: create_signal(false),
            component_wide: create_signal(false),
            seconds: create_signal(0),
            target: ComponentRef::new(),
        }
    }

    /// Starts a thread that adds a second to the counter, once per second.
    /// The render does not know about it: the renderer notices the change
    /// itself and asks the host for an update.
    pub fn start_ticker(&self) {
        let seconds = self.seconds;
        std::thread::spawn(move || run_ticker(seconds, Duration::from_secs(1), None));
    }

    /// The whole screen. Call it once per render.
    pub fn view(&self) -> impl Template {
        let Self {
            clicks,
            enabled,
            level,
            name,
            order,
            wifi,
            choice,
            color,
            notes,
            dialog_open,
            tab: tab_sel,
            fruit,
            row_picked,
            themed_on,
            parent_wide,
            component_wide,
            seconds,
            target,
        } = *self;

        let counter = column()
            .gap(8.0)
            .child(Text::new("Counter").size(20.0))
            .child(Text::dynamic(move || {
                format!("Clicked {} times", clicks.get())
            }))
            .child(Button::new("Increment").on_click(move || clicks.update(|n| *n += 1)));

        let checkbox = column()
            .gap(8.0)
            .child(Text::new("Checkbox").size(20.0))
            .child(
                Checkbox::new(enabled.get())
                    .name("Enabled")
                    .on_toggle(move |on| enabled.set(on)),
            )
            .child(Text::dynamic(move || {
                if enabled.get() { "Enabled" } else { "Disabled" }.to_string()
            }));

        let slider = column()
            .gap(8.0)
            .child(Text::new("Slider").size(20.0))
            .child(
                Slider::new(level.get(), 0.0, 100.0)
                    .step(1.0)
                    .name("Level")
                    .on_change(move |v| level.set(v)),
            )
            .child(Text::dynamic(move || format!("Level {:.0}", level.get())));

        let input = column()
            .gap(8.0)
            .child(Text::new("Text input").size(20.0))
            .child(
                TextInput::new(name.get())
                    .label("Name")
                    .on_input(move |text| name.set(text.to_string())),
            )
            .child(Text::dynamic(move || format!("Hello, {}", name.get())));

        let keyed_list = column()
            .gap(8.0)
            .child(Text::new("Keyed list").size(20.0))
            .child(
                row()
                    .gap(8.0)
                    .child(Button::new("Reverse").on_click(move || {
                        order.update(|items| items.reverse());
                    }))
                    .child(Button::new("Rotate").on_click(move || {
                        order.update(|items| {
                            if !items.is_empty() {
                                items.rotate_left(1);
                            }
                        });
                    })),
            )
            .child(KeyedItems(order));

        let scrolling = column()
            .gap(8.0)
            .child(Text::new("Scroll container").size(20.0))
            .child(scroll().height(120.0).child(Lines(24)));

        let switches = column()
            .gap(8.0)
            .child(Text::new("Switch and radio").size(20.0))
            .child(
                switch()
                    .label("Wi-Fi")
                    .checked(wifi.get())
                    .on_toggle(move |on| wifi.set(on)),
            )
            .child(
                radio()
                    .label("Small")
                    .selected(choice.get() == 0)
                    .on_click(move || choice.set(0)),
            )
            .child(
                radio()
                    .label("Large")
                    .selected(choice.get() == 1)
                    .on_click(move || choice.set(1)),
            )
            .child(Text::dynamic(move || {
                format!(
                    "Wi-Fi {}, {}",
                    if wifi.get() { "on" } else { "off" },
                    if choice.get() == 0 { "small" } else { "large" }
                )
            }));

        let dropdown_section = column()
            .gap(8.0)
            .child(Text::new("Dropdown").size(20.0))
            .child(
                COLORS
                    .iter()
                    .fold(dropdown().label("Color"), |d, c| d.item(*c))
                    .value(color.get())
                    .on_input(move |v| color.set(v.to_string())),
            )
            .child(Text::dynamic(move || format!("Color {}", color.get())));

        let progress = column()
            .gap(8.0)
            .child(Text::new("Progress and spinner").size(20.0))
            .child(
                progress_bar()
                    .label("Level")
                    .number(level.get())
                    .range(0.0, 100.0),
            )
            .child(loading_spinner().label("Loading"));

        let area = column()
            .gap(8.0)
            .child(Text::new("Text area").size(20.0))
            .child(
                text_area()
                    .label("Notes")
                    .value(notes.get())
                    .on_input(move |t| notes.set(t.to_string())),
            )
            .child(Text::dynamic(move || {
                format!("{} characters", notes.get().chars().count())
            }));

        let pictures = column()
            .gap(8.0)
            .child(Text::new("Image, divider, badge and link").size(20.0))
            .child(image().label("Red pixel").value(PIXEL_PNG).size(48.0, 48.0))
            .child(divider())
            .child(
                row()
                    .gap(8.0)
                    .child(Text::new("Clicks"))
                    .child(badge().label(format!("{}", clicks.get()))),
            )
            .child(
                link()
                    .label("schnellui on GitHub")
                    .value("https://github.com/codefionn/schnellui"),
            );

        let make_tab = move |i: usize| {
            tab()
                .label(TABS[i])
                .selected(tab_sel.get() == i)
                .on_click(move || tab_sel.set(i))
        };
        let tabs = column()
            .gap(8.0)
            .child(Text::new("Tabs").size(20.0))
            .child(
                tab_bar()
                    .label("Sections")
                    .child(make_tab(0))
                    .child(make_tab(1))
                    .child(make_tab(2)),
            )
            .child(Text::dynamic(move || {
                format!("Tab {}", TABS[tab_sel.get()])
            }));

        let make_item = move |i: usize| {
            list_item()
                .label(FRUITS[i])
                .selected(fruit.get() == i)
                .on_click(move || fruit.set(i))
        };
        let items = column()
            .gap(8.0)
            .child(Text::new("List").size(20.0))
            .child(
                list()
                    .label("Fruit")
                    .child(make_item(0))
                    .child(make_item(1))
                    .child(make_item(2)),
            )
            .child(Text::dynamic(move || {
                format!("Picked {}", FRUITS[fruit.get()])
            }));

        let tables = column()
            .gap(8.0)
            .child(Text::new("Table").size(20.0))
            .child(
                table()
                    .label("People")
                    .child(table_row().label("Header").item("Name").item("Role"))
                    .child(
                        table_row()
                            .label("Ada")
                            .item("Ada")
                            .item("Engineer")
                            .selected(row_picked.get())
                            .on_click(move || row_picked.update(|on| *on = !*on)),
                    ),
            )
            .child(Text::dynamic(move || {
                format!(
                    "Row {}",
                    if row_picked.get() {
                        "selected"
                    } else {
                        "not selected"
                    }
                )
            }));

        let rich = column()
            .gap(8.0)
            .child(Text::new("Rich text, icon, drag handle and svg").size(20.0))
            .child(
                rich_text()
                    .label("Release notes")
                    .value("Tables, themes and size queries are native views now."),
            )
            .child(
                row()
                    .gap(12.0)
                    .align(Align::Center)
                    .child(icon().detail("\u{2605}").label("Star"))
                    .child(drag_handle().label("Reorder"))
                    .child(svg().value(BADGE_SVG).size(48.0, 48.0).label("Red square")),
            );

        let themed = column()
            .gap(8.0)
            .child(Text::new("Theme provider").size(20.0))
            .child(
                theme_provider()
                    .value("dark")
                    .child(Text::new("Always dark"))
                    .child(Button::new("Dark button"))
                    .child(
                        switch()
                            .label("Dark switch")
                            .checked(themed_on.get())
                            .on_toggle(move |on| themed_on.set(on)),
                    ),
            )
            .child(Text::dynamic(move || {
                format!("Dark switch {}", if themed_on.get() { "on" } else { "off" })
            }));

        let width_of = |wide: bool| if wide { WIDE } else { NARROW };
        let parent_query = column()
            .gap(8.0)
            .child(Text::new("Parent query").size(20.0))
            .child(Button::new("Toggle parent width").on_click(move || {
                parent_wide.update(|w| *w = !*w);
            }))
            .child(
                column().width(width_of(parent_wide.get())).child(
                    Text::new("The parent is wide")
                        .show_when(ResponsiveQuery::parent().min_width(SHOW_FROM)),
                ),
            );
        let component_query = column()
            .gap(8.0)
            .child(Text::new("Component query").size(20.0))
            .child(Button::new("Toggle component width").on_click(move || {
                component_wide.update(|w| *w = !*w);
            }))
            .child(
                column()
                    .width(width_of(component_wide.get()))
                    .child(
                        row()
                            .child(Text::new("The component is wide").show_when(
                                ResponsiveQuery::component(target).min_width(SHOW_FROM),
                            )),
                    )
                    .with_ref(target),
            );

        let ticker = column()
            .gap(8.0)
            .child(Text::new("Background thread").size(20.0))
            .child(Text::dynamic(move || {
                format!("Running for {} s", seconds.get())
            }));

        scroll().fill().child(
            Pad::all(16.0).child(
                column()
                    .gap(24.0)
                    .align(Align::Stretch)
                    .child(counter)
                    .child(checkbox)
                    .child(slider)
                    .child(input)
                    .child(keyed_list)
                    .child(scrolling)
                    .child(switches)
                    .child(dropdown_section)
                    .child(progress)
                    .child(area)
                    .child(pictures)
                    .child(DialogSection(dialog_open))
                    .child(tabs)
                    .child(items)
                    .child(tables)
                    .child(rich)
                    .child(themed)
                    .child(parent_query)
                    .child(component_query)
                    .child(ticker)
                    .child(Spacer::new()),
            ),
        )
    }
}

impl Default for Gallery {
    fn default() -> Self {
        Self::new()
    }
}

/// A button that opens a dialog. The dialog exists only while it is open, and
/// DISMISS (back, or a tap outside) leaves it to the app to close it.
struct DialogSection(Signal<bool>);

impl Template for DialogSection {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let open = self.0;
        let mut nodes = vec![
            Text::new("Dialog").size(20.0).render(renderer),
            Button::new("Open dialog")
                .on_click(move || open.set(true))
                .render(renderer),
        ];
        if open.get() {
            nodes.push(
                dialog()
                    .label("Hello")
                    .child(Text::new("This is a dialog."))
                    .child(Button::new("Close").on_click(move || open.set(false)))
                    .on_click(move || open.set(false))
                    .render(renderer),
            );
        }
        let mut props = ContainerProps::new(ContainerKind::Column);
        props.gap = 8.0;
        renderer.container(props, nodes)
    }
}

/// One keyed row per id, so a reorder moves views instead of rebuilding them.
struct KeyedItems(Signal<Vec<u32>>);

impl Template for KeyedItems {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let items = self.0.get();
        let nodes = items
            .into_iter()
            .map(|id| {
                Text::new(format!("Item {id}"))
                    .key(format!("item-{id}"))
                    .render(renderer)
            })
            .collect();
        let mut props = ContainerProps::new(ContainerKind::Column);
        props.gap = 4.0;
        renderer.container(props, nodes)
    }
}

/// A column of numbered lines, tall enough to scroll.
struct Lines(u32);

impl Template for Lines {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let nodes = (1..=self.0)
            .map(|n| Text::new(format!("Line {n}")).render(renderer))
            .collect();
        let mut props = ContainerProps::new(ContainerKind::Column);
        props.gap = 4.0;
        renderer.container(props, nodes)
    }
}

/// Adds a second to `seconds` every `period`, `limit` times or forever.
fn run_ticker(seconds: Signal<u32>, period: Duration, limit: Option<u32>) {
    let mut done = 0;
    while limit.is_none_or(|limit| done < limit) {
        std::thread::sleep(period);
        seconds.update(|s| *s += 1);
        done += 1;
    }
}

/// Mounts the gallery. The Android entry point and the tests both use it.
pub fn mount(viewport: schnellui::render_android::Viewport) -> AndroidRenderer {
    let gallery = Gallery::new();
    AndroidRenderer::new(viewport, move || gallery.view())
}

schnellui::android_app!({
    let gallery = Gallery::new();
    gallery.start_ticker();
    move || gallery.view()
});

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::render_android::ops::{kind, prop};
    use schnellui::render_android::{decode, Event, Op, Viewport};

    fn text_id(ops: &[Op], needle: &str) -> Option<u32> {
        ops.iter().find_map(|op| match op {
            Op::SetStr { id, value, .. } if value == needle => Some(*id),
            _ => None,
        })
    }

    fn kind_ids(ops: &[Op], k: u8) -> Vec<u32> {
        ops.iter()
            .filter_map(|op| match op {
                Op::Create { id, kind } if *kind == k => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn set_text(diff: &[u8], id: u32) -> Option<String> {
        decode(diff).unwrap().into_iter().find_map(|op| match op {
            Op::SetStr {
                id: i,
                prop: prop::TEXT,
                value,
            } if i == id => Some(value),
            _ => None,
        })
    }

    #[test]
    fn gallery_renders_and_the_counter_responds() {
        let mut app = mount(Viewport::new(360.0, 800.0, 2.0, false));
        let ops = decode(&app.render()).unwrap();
        assert!(matches!(ops.last(), Some(Op::SetRoot { .. })));
        let label = text_id(&ops, "Clicked 0 times").expect("counter text");
        let button = text_id(&ops, "Increment").expect("button");
        let diff = app.dispatch(button, Event::Click).expect("diff");
        let diff = decode(&diff).unwrap();
        // The counter text and the badge both follow the counter.
        assert!(diff.contains(&Op::SetStr {
            id: label,
            prop: prop::TEXT,
            value: "Clicked 1 times".into(),
        }));
        let badge = kind_ids(&ops, kind::BADGE)[0];
        assert!(ops.contains(&Op::SetStr {
            id: badge,
            prop: prop::TEXT,
            value: "0".into(),
        }));
        assert!(diff.contains(&Op::SetStr {
            id: badge,
            prop: prop::TEXT,
            value: "1".into(),
        }));
    }

    #[test]
    fn native_widgets_follow_their_signals() {
        let mut app = mount(Viewport::new(360.0, 800.0, 2.0, false));
        let ops = decode(&app.render()).unwrap();
        for k in [
            kind::SWITCH,
            kind::RADIO,
            kind::DROPDOWN,
            kind::PROGRESS,
            kind::LOADING,
            kind::TEXT_AREA,
            kind::IMAGE,
            kind::DIVIDER,
            kind::BADGE,
            kind::LINK,
            kind::TAB_BAR,
            kind::TAB,
            kind::LIST_ITEM,
        ] {
            assert!(!kind_ids(&ops, k).is_empty(), "kind {k} missing");
        }
        assert!(kind_ids(&ops, kind::DIALOG).is_empty());

        // Switch and radio change the summary text.
        let summary = text_id(&ops, "Wi-Fi off, small").expect("summary");
        let sw = kind_ids(&ops, kind::SWITCH)[0];
        let diff = app.dispatch(sw, Event::Toggle(true)).expect("diff");
        assert_eq!(set_text(&diff, summary).as_deref(), Some("Wi-Fi on, small"));
        let radios = kind_ids(&ops, kind::RADIO);
        let diff = app.dispatch(radios[1], Event::Click).expect("diff");
        assert_eq!(set_text(&diff, summary).as_deref(), Some("Wi-Fi on, large"));

        // The dropdown reports the index and Rust maps it to the value.
        let color = text_id(&ops, "Color Green").expect("color");
        let dd = kind_ids(&ops, kind::DROPDOWN)[0];
        let diff = app.dispatch(dd, Event::Select(2)).expect("diff");
        assert_eq!(set_text(&diff, color).as_deref(), Some("Color Blue"));

        // The text area echoes its length.
        let count = text_id(&ops, "0 characters").expect("count");
        let ta = kind_ids(&ops, kind::TEXT_AREA)[0];
        let diff = app
            .dispatch(ta, Event::Input("héllo".into()))
            .expect("diff");
        assert_eq!(set_text(&diff, count).as_deref(), Some("5 characters"));

        // The progress bar follows the slider.
        let slider = kind_ids(&ops, kind::SLIDER)[0];
        let progress = kind_ids(&ops, kind::PROGRESS)[0];
        let diff = decode(&app.dispatch(slider, Event::Change(75.0)).expect("diff")).unwrap();
        assert!(diff.contains(&Op::SetF32 {
            id: progress,
            prop: prop::VALUE,
            value: 75.0
        }));

        // Tabs and list items.
        let tab_text = text_id(&ops, "Tab One").expect("tab text");
        let tabs = kind_ids(&ops, kind::TAB);
        let diff = app.dispatch(tabs[2], Event::Click).expect("diff");
        assert_eq!(set_text(&diff, tab_text).as_deref(), Some("Tab Three"));
        let picked = text_id(&ops, "Picked Pear").expect("picked");
        let items = kind_ids(&ops, kind::LIST_ITEM);
        let diff = app.dispatch(items[0], Event::Click).expect("diff");
        assert_eq!(set_text(&diff, picked).as_deref(), Some("Picked Apple"));

        // The dialog opens from the button and DISMISS closes it.
        let open = text_id(&ops, "Open dialog").expect("open button");
        let diff = decode(&app.dispatch(open, Event::Click).expect("diff")).unwrap();
        let dialog = kind_ids(&diff, kind::DIALOG);
        assert_eq!(dialog.len(), 1);
        // The Close button inside the dialog closes it.
        let close = text_id(&diff, "Close").expect("close button");
        let closed = decode(&app.dispatch(close, Event::Click).expect("diff")).unwrap();
        assert!(closed.contains(&Op::Remove { id: dialog[0] }));
        // So does DISMISS, on a dialog that was opened again.
        let diff = decode(&app.dispatch(open, Event::Click).expect("diff")).unwrap();
        let dialog = kind_ids(&diff, kind::DIALOG);
        assert_eq!(dialog.len(), 1);
        let diff = decode(&app.dispatch(dialog[0], Event::Dismiss).expect("diff")).unwrap();
        assert!(diff.contains(&Op::Remove { id: dialog[0] }));
    }

    fn set_i32(ops: &[Op], id: u32, p: u8) -> Option<i32> {
        ops.iter().find_map(|op| match op {
            Op::SetI32 { id: i, prop, value } if *i == id && *prop == p => Some(*value),
            _ => None,
        })
    }

    fn set_f32(ops: &[Op], id: u32, p: u8) -> Option<f32> {
        ops.iter().find_map(|op| match op {
            Op::SetF32 { id: i, prop, value } if *i == id && *prop == p => Some(*value),
            _ => None,
        })
    }

    #[test]
    fn tables_rich_text_icons_and_svg_render() {
        let mut app = mount(Viewport::new(360.0, 800.0, 2.0, false));
        let ops = decode(&app.render()).unwrap();
        assert_eq!(kind_ids(&ops, kind::TABLE).len(), 1);
        let rows = kind_ids(&ops, kind::TABLE_ROW);
        assert_eq!(rows.len(), 2);
        assert!(ops.contains(&Op::SetStr {
            id: rows[1],
            prop: prop::ITEMS,
            value: "Ada\u{1f}Engineer".into(),
        }));
        // The selectable row toggles and the text under the table follows.
        let status = text_id(&ops, "Row not selected").expect("status");
        let diff = decode(&app.dispatch(rows[1], Event::Click).expect("diff")).unwrap();
        assert_eq!(set_i32(&diff, rows[1], prop::SELECTED), Some(1));
        assert!(diff.contains(&Op::SetStr {
            id: status,
            prop: prop::TEXT,
            value: "Row selected".into(),
        }));
        // Rich text, icon, drag handle.
        assert!(text_id(&ops, "Release notes").is_some());
        assert!(text_id(&ops, "\u{2605}").is_some());
        assert!(text_id(&ops, "\u{22ee}\u{22ee}").is_some());
        // The svg is the second Image, with a PNG data URL.
        let images = kind_ids(&ops, kind::IMAGE);
        assert_eq!(images.len(), 2);
        let src = ops.iter().find_map(|op| match op {
            Op::SetStr { id, prop: p, value } if *id == images[1] && *p == prop::SRC => {
                Some(value.clone())
            }
            _ => None,
        });
        assert!(src
            .expect("svg src")
            .starts_with("data:image/png;base64,iVBORw0KGgo"));
    }

    #[test]
    fn dark_theme_provider_holds_its_children() {
        let mut app = mount(Viewport::new(360.0, 800.0, 2.0, false));
        let ops = decode(&app.render()).unwrap();
        let themed: Vec<(u32, u8)> = ops
            .iter()
            .filter_map(|op| match op {
                Op::CreateThemed { id, kind: k, theme } if *k == kind::THEME_PROVIDER => {
                    Some((*id, *theme))
                }
                _ => None,
            })
            .collect();
        assert_eq!(themed.len(), 1);
        assert_eq!(themed[0].1, 2);
        let themed_kinds: Vec<u8> = ops
            .iter()
            .filter_map(|op| match op {
                Op::CreateThemed { kind, theme: 2, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        for k in [kind::TEXT, kind::BUTTON, kind::SWITCH] {
            assert!(themed_kinds.contains(&k), "kind {k} not themed");
        }
        // Nothing else is themed: the provider, its text, button and switch.
        assert_eq!(themed_kinds.len(), 4);
        // The switch inside still reports to Rust.
        let switch = ops
            .iter()
            .find_map(|op| match op {
                Op::CreateThemed { id, kind: k, .. } if *k == kind::SWITCH => Some(*id),
                _ => None,
            })
            .unwrap();
        let status = text_id(&ops, "Dark switch on").expect("status");
        let diff = app.dispatch(switch, Event::Toggle(false)).expect("diff");
        assert_eq!(set_text(&diff, status).as_deref(), Some("Dark switch off"));
    }

    #[test]
    fn size_queries_follow_the_toggled_width() {
        let mut app = mount(Viewport::new(360.0, 800.0, 2.0, false));
        let ops = decode(&app.render()).unwrap();
        // Parent query: target 0, shown from 200 dp.
        let parent_text = text_id(&ops, "The parent is wide").expect("parent text");
        assert_eq!(set_i32(&ops, parent_text, prop::SHOW_TARGET), Some(0));
        assert_eq!(set_f32(&ops, parent_text, prop::SHOW_MIN_W), Some(200.0));
        // Component query: the target is the column that carries the width.
        let component_text = text_id(&ops, "The component is wide").expect("component text");
        let target = set_i32(&ops, component_text, prop::SHOW_TARGET).expect("target");
        assert_ne!(target, 0);
        assert_eq!(set_f32(&ops, target as u32, prop::WIDTH), Some(120.0));
        assert_eq!(set_f32(&ops, component_text, prop::SHOW_MIN_W), Some(200.0));
        // The buttons change the widths of the targets.
        let button = text_id(&ops, "Toggle component width").expect("button");
        let diff = decode(&app.dispatch(button, Event::Click).expect("diff")).unwrap();
        assert_eq!(set_f32(&diff, target as u32, prop::WIDTH), Some(260.0));
        let button = text_id(&ops, "Toggle parent width").expect("button");
        let diff = decode(&app.dispatch(button, Event::Click).expect("diff")).unwrap();
        let parent = ops
            .iter()
            .find_map(|op| match op {
                Op::Insert { parent, child, .. } if *child == parent_text => Some(*parent),
                _ => None,
            })
            .unwrap();
        assert_eq!(set_f32(&diff, parent, prop::WIDTH), Some(260.0));
    }

    #[test]
    fn a_background_thread_counts_seconds_and_wakes_the_host() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let gallery = Gallery::new();
        let mut app = AndroidRenderer::new(Viewport::new(360.0, 800.0, 2.0, false), move || {
            gallery.view()
        });
        let woken = Arc::new(AtomicUsize::new(0));
        let seen = woken.clone();
        app.set_wake(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let ops = decode(&app.render()).unwrap();
        let label = text_id(&ops, "Running for 0 s").expect("counter");
        app.start_wake_polling(Duration::from_millis(2));
        let seconds = gallery.seconds;
        std::thread::spawn(move || run_ticker(seconds, Duration::from_millis(5), Some(1)))
            .join()
            .unwrap();
        let start = std::time::Instant::now();
        while woken.load(Ordering::SeqCst) == 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 1);
        let diff = app.update().expect("diff");
        assert_eq!(set_text(&diff, label).as_deref(), Some("Running for 1 s"));
    }
}
