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
    column, row, scroll, Align, Button, Checkbox, ContainerKind, ContainerProps, Pad, Slider,
    Spacer, Template, TemplateRenderer, Text, TextInput,
};

/// All state of the gallery.
#[derive(Clone, Copy)]
pub struct Gallery {
    clicks: Signal<i32>,
    enabled: Signal<bool>,
    level: Signal<f32>,
    name: Signal<String>,
    order: Signal<Vec<u32>>,
}

impl Gallery {
    pub fn new() -> Self {
        Self {
            clicks: create_signal(0),
            enabled: create_signal(true),
            level: create_signal(40.0),
            name: create_signal("Ada".to_string()),
            order: create_signal(vec![1, 2, 3, 4]),
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

        let list = column()
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

        scroll().fill().child(
            Pad::all(16.0).child(
                column()
                    .gap(24.0)
                    .align(Align::Stretch)
                    .child(counter)
                    .child(checkbox)
                    .child(slider)
                    .child(input)
                    .child(list)
                    .child(scrolling)
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
    use schnellui::render_android::{decode, Op, Viewport};

    #[test]
    fn gallery_renders_and_the_counter_responds() {
        let mut app = mount(Viewport::new(360.0, 800.0, 2.0, false));
        let ops = decode(&app.render()).unwrap();
        assert!(matches!(ops.last(), Some(Op::SetRoot { .. })));
        let text = |ops: &[Op], needle: &str| {
            ops.iter().find_map(|op| match op {
                Op::SetStr { id, value, .. } if value == needle => Some(*id),
                _ => None,
            })
        };
        let label = text(&ops, "Clicked 0 times").expect("counter text");
        let button = text(&ops, "Increment").expect("button");
        let diff = app
            .dispatch(button, schnellui::render_android::Event::Click)
            .expect("diff");
        let diff = decode(&diff).unwrap();
        assert_eq!(
            diff,
            vec![Op::SetStr {
                id: label,
                prop: schnellui::render_android::ops::prop::TEXT,
                value: "Clicked 1 times".into(),
            }]
        );
    }
}
