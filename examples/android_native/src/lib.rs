//! Native Android views from a schnellui template.
//!
//! A gallery of the base component set. Every control is a real
//! `android.widget` view and every value lives in a signal. Build the `cdylib`
//! for an Android target and load it from the `android/` demo app. The same
//! view runs on the host through [`AndroidRenderer`], which is how the tests
//! here drive it.

use schnellui::render_android::AndroidRenderer;
use schnellui::signal::{create_signal, Signal};
use schnellui::template::{
    badge, column, dialog, divider, dropdown, image, link, list, list_item, loading_spinner,
    progress_bar, radio, row, scroll, switch, tab, tab_bar, text_area, Align, Button, Checkbox,
    ContainerKind, ContainerProps, Pad, Slider, Spacer, Template, TemplateRenderer, Text,
    TextInput,
};

/// A 1 by 1 red PNG, scaled up by the image view.
const PIXEL_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";

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
        }
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

/// Mounts the gallery. The Android entry point and the tests both use it.
pub fn mount(viewport: schnellui::render_android::Viewport) -> AndroidRenderer {
    let gallery = Gallery::new();
    AndroidRenderer::new(viewport, move || gallery.view())
}

schnellui::android_app!({
    let gallery = Gallery::new();
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
        let diff = decode(&app.dispatch(dialog[0], Event::Dismiss).expect("diff")).unwrap();
        assert!(diff.contains(&Op::Remove { id: dialog[0] }));
    }
}
