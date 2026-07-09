use crate::renderer::*;
use crate::template::*;
use crate::*;

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use schnellui_scene::Color;
    use schnellui_template::{
        column, link, list, list_item, row, table, table_row, Button, Checkbox, PasswordInput,
        Slider, Text, TextInput,
    };

    use super::*;

    #[test]
    fn emits_native_semantic_html_without_canvas() {
        let view = column().child(Text::new("<hello>")).child(
            row()
                .gap(8.0)
                .child(Button::new("Continue"))
                .child(Checkbox::new(true))
                .child(Slider::new(4.0, 0.0, 10.0))
                .child(TextInput::new("Ada").label("Name")),
        );
        let html = HtmlRenderer::new(400, 200).render(view).into_string();
        assert!(html.contains("<button"));
        assert!(html.contains(r#"type="checkbox""#));
        assert!(html.contains(r#"type="range""#));
        assert!(html.contains(r#"type="text""#));
        assert!(html.contains("&lt;hello&gt;"));
        assert!(!html.contains("<canvas"));
    }

    #[test]
    fn password_input_uses_the_native_protected_control() {
        let html = HtmlRenderer::new(400, 200)
            .render(PasswordInput::new("secret").label("API key"))
            .into_string();
        assert!(html.contains(r#"data-sui-component="PasswordInput""#));
        assert!(html.contains(r#"data-sui-role="password-input""#));
        assert!(html.contains(r#"type="password""#));
    }

    #[test]
    fn document_keeps_the_requested_logical_viewport() {
        let html = HtmlRenderer::new(320, 180)
            .with_scale(2.0)
            .render(Text::new("scaled"))
            .into_string();
        assert!(html.contains("width: 320px; height: 180px"));
        assert!(!html.contains("transform: scale"));
    }

    #[test]
    fn scroll_configuration_controls_native_chrome_and_edge_scrolling() {
        use schnellui_template::scroll;

        let html = HtmlRenderer::new(320, 180)
            .render(
                scroll()
                    .size(200.0, 100.0)
                    .scrollbar(true)
                    .edge_auto_scroll(true)
                    .child(Text::new("content")),
            )
            .into_string();
        assert!(html.contains(r#"data-sui-scrollbar="true""#));
        assert!(html.contains(r#"data-sui-edge-auto-scroll="true""#));
        assert!(html.contains("scrollbar-color: var(--sui-outline)"));
        assert!(html.contains("requestAnimationFrame(tick)"));
    }

    #[test]
    fn responsive_templates_emit_media_and_container_queries() {
        use schnellui_template::{em, px, ComponentRef, Template as _};

        let card_ref = ComponentRef::new();
        let viewport = Text::new("wide").show_when(ResponsiveQuery::viewport().min_width(em(40.0)));
        let parent =
            Text::new("compact card").show_when(ResponsiveQuery::parent().max_width(px(320.0)));
        let referenced = Text::new("named card action")
            .show_when(ResponsiveQuery::component(card_ref).min_width(px(280.0)));
        let html = HtmlRenderer::new(800, 600)
            .render(
                column()
                    .size(500.0, 200.0)
                    .child(viewport)
                    .child(parent)
                    .child(referenced)
                    .with_ref(card_ref),
            )
            .into_string();

        assert!(html.contains("@media (min-width: 40em)"));
        assert!(html.contains("@container (max-width: 320px)"));
        assert!(html.contains(&format!(
            "@container sui-ref-{} (min-width: 280px)",
            card_ref.id()
        )));
        assert!(html.contains(&format!(r#"data-sui-ref="{}""#, card_ref.id())));
        assert!(html.contains("container-type: size"));
        assert!(html.contains("display: contents"));
    }

    #[test]
    fn an_unqueried_component_ref_does_not_change_html_layout() {
        use schnellui_template::{ComponentRef, Template as _};

        let reference = ComponentRef::new();
        let html = HtmlRenderer::new(320, 180)
            .render(Text::new("referenced").with_ref(reference))
            .into_string();

        assert!(html.contains(&format!(r#"data-sui-ref="{}""#, reference.id())));
        assert!(!html.contains(&format!("container-name: sui-ref-{}", reference.id())));
    }

    #[test]
    fn interactive_elements_expose_a_keyboard_focus_contract() {
        let html = HtmlRenderer::new(400, 240)
            .render(
                column()
                    .child(Button::new("Save"))
                    .child(
                        list()
                            .label("Files")
                            .child(list_item().label("README.md").on_click(|| {})),
                    )
                    .child(
                        table()
                            .label("Rows")
                            .child(table_row().label("Build row").item("Ready").on_click(|| {})),
                    )
                    .child(link().label("Disabled docs").disabled(true)),
            )
            .into_string();

        assert!(html.contains(":focus-visible"));
        assert!(html.contains(&format!(
            "--sui-focus: {}",
            color_css(Theme::default().focus_color())
        )));
        assert!(html.contains("outline-offset:-3px"));
        assert!(html.contains("@media (forced-colors: active)"));
        assert!(html.contains(
            r#"data-sui-name="README.md" tabindex="0" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();this.click()}""#
        ));
        assert!(html.contains(
            r#"data-sui-name="Build row" tabindex="0" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();this.click()}""#
        ));
        assert!(
            html.contains(r#"data-sui-name="Disabled docs" aria-disabled="true" tabindex="-1""#)
        );
    }

    #[test]
    fn interaction_css_keeps_channels_and_component_overrides_independent() {
        use schnellui_widgets::{ComponentInteractions, InteractionStates, InteractionStyle};

        let hover_background = Color::rgb(1, 2, 3);
        let hover_foreground = Color::rgb(4, 5, 6);
        let hover_border = Color::rgb(7, 8, 9);
        let focus_border = Color::rgb(10, 11, 12);
        let toggle_active = Color::rgb(13, 14, 15);
        let theme = Theme {
            component_interactions: ComponentInteractions {
                button: Some(InteractionStates {
                    hover: InteractionStyle::all(hover_background, hover_foreground, hover_border),
                    focus: InteractionStyle::border(focus_border),
                    active: InteractionStyle::NONE,
                }),
                toggle: Some(InteractionStates {
                    active: InteractionStyle::foreground(toggle_active),
                    ..InteractionStates::NONE
                }),
                ..ComponentInteractions::NONE
            },
            ..Theme::default()
        };
        let html = HtmlRenderer::new(400, 240)
            .with_theme(theme)
            .render(
                column()
                    .child(Button::new("Save"))
                    .child(Checkbox::new(false)),
            )
            .into_string();

        assert!(html.contains(&format!(
            "background-color:{};color:{};border-color:{};",
            color_css(hover_background),
            color_css(hover_foreground),
            color_css(hover_border)
        )));
        assert!(html.contains(&format!(
            "outline:3px solid {};outline-offset:-3px;",
            color_css(focus_border)
        )));
        assert!(html.contains(&format!("color:{};", color_css(toggle_active))));
    }

    #[test]
    fn rendered_handlers_own_and_invoke_rust_callbacks() {
        let clicks = Rc::new(Cell::new(0));
        let checked = Rc::new(Cell::new(false));
        let number = Rc::new(Cell::new(0.0));
        let text = Rc::new(RefCell::new(String::new()));
        let click_sink = clicks.clone();
        let checked_sink = checked.clone();
        let number_sink = number.clone();
        let text_sink = text.clone();

        let view = column()
            .child(Button::new("go").on_click(move || click_sink.set(1)))
            .child(Checkbox::new(false).on_toggle(move |value| checked_sink.set(value)))
            .child(Slider::new(0.0, 0.0, 100.0).on_change(move |value| number_sink.set(value)))
            .child(TextInput::new("").on_input(move |value| {
                *text_sink.borrow_mut() = value.to_string();
            }));
        let mut rendered = HtmlRenderer::new(100, 100).render_page(view);
        assert_eq!(rendered.handlers.len(), 4);

        invoke_handler(
            &mut rendered.handlers[0],
            &BindingPayload {
                id: 0,
                value: String::new(),
                checked: false,
            },
        )
        .unwrap();
        invoke_handler(
            &mut rendered.handlers[1],
            &BindingPayload {
                id: 1,
                value: "on".into(),
                checked: true,
            },
        )
        .unwrap();
        invoke_handler(
            &mut rendered.handlers[2],
            &BindingPayload {
                id: 2,
                value: "73".into(),
                checked: false,
            },
        )
        .unwrap();
        invoke_handler(
            &mut rendered.handlers[3],
            &BindingPayload {
                id: 3,
                value: "browser".into(),
                checked: false,
            },
        )
        .unwrap();

        assert_eq!(clicks.get(), 1);
        assert!(checked.get());
        assert_eq!(number.get(), 73.0);
        assert_eq!(&*text.borrow(), "browser");
    }
}
