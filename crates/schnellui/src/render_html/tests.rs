use crate::render_html::renderer::*;
use crate::render_html::template::*;
use crate::render_html::*;

/// Counts allocations made by the current thread, for the emission benchmark.
///
/// The counting `#[global_allocator]` is the one `allocation_counter` installs on
/// link; every test module in the umbrella shares it (one test binary, one global
/// allocator).
mod counting {
    /// Allocation count and bytes requested while `f` runs on this thread.
    pub fn measure<T>(f: impl FnOnce() -> T) -> (T, u64, u64) {
        let mut out = None;
        let info = allocation_counter::measure(|| out = Some(f()));
        (out.unwrap(), info.count_total, info.bytes_total)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;

    use schnellui::scene::Color;
    use schnellui::template::{
        column, link, list, list_item, row, table, table_row, Button, Checkbox, PasswordInput,
        Slider, Text, TextInput,
    };

    use super::*;

    /// One of every component kind with hostile strings in every text slot, so
    /// the golden and role-table tests cover all emission paths.
    fn kitchen_sink() -> impl schnellui::template::Template {
        use schnellui::template::*;
        let evil = r#"a"b'c<d>&e é ✓"#;
        column()
            .gap(3.5)
            .child(Text::new(evil))
            .child(Button::new(evil).on_click(|| {}))
            .child(Checkbox::new(true).name(evil))
            .child(Slider::new(4.0, 0.0, 10.0).step(0.5).name(evil))
            .child(TextInput::new(evil).label(evil))
            .child(PasswordInput::new("pw").label(evil))
            .child(Pad::all(4.0).child(Text::new("padded")))
            .child(Flex::new().grow(1.0).child(Text::new("flex")))
            .child(Text::new("keyed").key("k1"))
            .child(Text::new("keyed twice").key("inner").key("outer"))
            .child(
                Text::new("ref")
                    .show_when(ResponsiveQuery::viewport().max_width(px(400.0)))
                    .key("rk"),
            )
            .child(
                row()
                    .child(dock_area().label(evil).child(Text::new("in dock")))
                    .child(drag_handle().label(evil).on_click(|| {}))
                    .child(image().label(evil).value(evil).size(10.0, 20.0))
                    .child(icon().label(evil).detail(evil))
                    .child(icon().label("default icon"))
                    .child(progress_bar().label(evil).number(3.0).range(0.0, 9.0))
                    .child(loading_spinner().label(evil))
                    .child(switch().label(evil).checked(true).on_toggle(|_| {}))
                    .child(radio().label(evil).selected(true).on_click(|| {}))
                    .child(divider())
                    .child(link().label(evil).value(evil).on_click(|| {}))
                    .child(link().label("off").disabled(true))
                    .child(badge().label(evil)),
            )
            .child(
                dialog().label(evil).child(
                    grouped_tab_list().label(evil).child(
                        tab_group().label(evil).child(
                            tab_node().label(evil).child(
                                tab_bar()
                                    .label(evil)
                                    .child(tab().label(evil).selected(true).on_click(|| {})),
                            ),
                        ),
                    ),
                ),
            )
            .child(
                list().label(evil).child(
                    list_item()
                        .label(evil)
                        .selected(true)
                        .on_click(|| {})
                        .child(Text::new("nested")),
                ),
            )
            .child(
                dropdown()
                    .label(evil)
                    .on_input(|_| {})
                    .child(dropdown_option().label(evil).value(evil).selected(true))
                    .child(dropdown_option().label("plain")),
            )
            .child(
                table().label(evil).child(
                    table_row()
                        .label(evil)
                        .item(evil)
                        .item("cell")
                        .on_click(|| {}),
                ),
            )
            .child(rich_text().label(evil).value(evil).child(Text::new("rt")))
            .child(
                text_area()
                    .label(evil)
                    .detail(evil)
                    .value(evil)
                    .on_input(|_| {}),
            )
            .child(svg().label(evil).size(5.0, 6.0).value("<svg/>"))
            .child(svg().markup(TrustedHtml::from_static("<svg id=\"t\"></svg>")))
            .child(theme_provider().value(evil).child(Text::new("themed")))
    }

    /// A long list of nested rows under a deep container chain, built through the
    /// renderer seam because templates have no dynamic child list.
    struct BigList(usize);

    impl schnellui::template::Template for BigList {
        fn render<R: TemplateRenderer>(self, r: &mut R) -> R::Node {
            use schnellui::template::*;
            let items: Vec<R::Node> = (0..self.0)
                .map(|i| {
                    list_item()
                        .label(format!("Item {i} <x>"))
                        .child(
                            row().gap(4.0).child(
                                Pad::all(2.0).child(
                                    column()
                                        .child(Text::new(format!("title {i} & more")))
                                        .child(
                                            row()
                                                .child(Button::new("Open").on_click(|| {}))
                                                .child(Text::new("meta")),
                                        ),
                                ),
                            ),
                        )
                        .key(format!("k{i}"))
                        .render(r)
                })
                .collect();
            let mut props = ComponentProps::new(ComponentKind::List);
            props.label = "Items".into();
            let mut node = r.component(props, items);
            for _ in 0..6 {
                node = r.container(ContainerProps::new(ContainerKind::Column), vec![node]);
            }
            node
        }
    }

    /// `cargo test -p schnellui --release --lib emission_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn emission_bench() {
        let renderer = HtmlRenderer::new(800, 600);
        let (html, allocs, bytes) =
            counting::measure(|| renderer.render(BigList(1000)).into_string());
        let mut times = Vec::new();
        for _ in 0..15 {
            let start = std::time::Instant::now();
            std::hint::black_box(renderer.render(BigList(1000)));
            times.push(start.elapsed());
        }
        times.sort();
        println!(
            "emission_bench: html={}B allocs={allocs} requested={bytes}B median={:?} min={:?}",
            html.len(),
            times[times.len() / 2],
            times[0]
        );
    }

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
        use schnellui::template::scroll;

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
    fn keyed_components_emit_data_sui_key() {
        use schnellui::template::Template as _;

        let html = HtmlRenderer::new(320, 180)
            .render(
                column()
                    .child(Button::new("Delete").key("a"))
                    .child(Button::new("Delete").key(r#"b"c"#)),
            )
            .into_string();
        assert!(html.contains(r#"data-sui-key="a""#));
        assert!(html.contains(r#"data-sui-key="b&quot;c""#));
    }

    #[test]
    fn svg_source_value_renders_as_an_image_and_trusted_markup_stays_verbatim() {
        use schnellui::template::{svg, TrustedHtml};

        let hostile = r#"<svg onload="alert(1)"><script>alert(1)</script></svg>"#;
        let renderer = HtmlRenderer::new(320, 180);
        let base = renderer.render(svg()).into_string();
        let html = renderer
            .render(svg().label(r#"a"b"#).value(hostile))
            .into_string();
        // No source markup reaches the page; it travels inside the data URL.
        assert_eq!(
            html.matches("<script").count(),
            base.matches("<script").count()
        );
        assert!(!html.contains("onload"));
        assert!(!html.contains("alert(1)"));
        let start = html
            .find(r#"<img src="data:image/svg+xml;base64,"#)
            .unwrap();
        let url_start = start + r#"<img src=""#.len();
        let url_end = url_start + html[url_start..].find('"').unwrap();
        let payload = &html[url_start + "data:image/svg+xml;base64,".len()..url_end];
        assert_eq!(decode_base64(payload), hostile.as_bytes());
        assert!(html[url_end..].starts_with(r#"" alt="a&quot;b">"#));

        // An empty source draws nothing.
        assert!(!base.contains("<img src=\"data:image/svg+xml"));

        let trusted = renderer
            .render(svg().markup(TrustedHtml::from_static(r#"<svg id="trusted-mark"></svg>"#)))
            .into_string();
        assert!(trusted.contains(r#"<svg id="trusted-mark"></svg>"#));
        assert!(!trusted.contains("data:image/svg+xml"));
    }

    fn decode_base64(encoded: &str) -> Vec<u8> {
        const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut bits = 0u32;
        let mut count = 0;
        for c in encoded.chars().filter(|&c| c != '=') {
            bits = bits << 6 | ALPHABET.find(c).expect("base64 alphabet") as u32;
            count += 6;
            if count >= 8 {
                count -= 8;
                out.push((bits >> count) as u8);
                bits &= (1 << count) - 1;
            }
        }
        out
    }

    #[test]
    fn a_node_emits_one_key_last_wins_and_one_ref_first_wins() {
        use schnellui::template::{ComponentRef, Template as _};

        let (first, second) = (ComponentRef::new(), ComponentRef::new());
        let html = HtmlRenderer::new(320, 180)
            .render(
                column()
                    .child(Button::new("x").key("a").key("b"))
                    .child(Button::new("y").key("k").with_ref(first).with_ref(second)),
            )
            .into_string();
        assert_eq!(html.matches("data-sui-key=").count(), 2);
        assert!(html.contains(r#"data-sui-key="b""#));
        assert!(!html.contains(r#"data-sui-key="a""#));
        assert_eq!(html.matches("data-sui-ref=").count(), 1);
        assert!(html.contains(&format!(
            r#"data-sui-key="k" data-sui-ref="{}""#,
            first.id()
        )));
    }

    /// The full document for the kitchen-sink fixture, byte for byte. A change to
    /// the emitted markup or stylesheet shows up as a reviewable diff of
    /// `snapshots/kitchen_sink.html`; regenerate it with
    /// `SCHNELLUI_UPDATE_SNAPSHOTS=1 cargo test -p schnellui --lib golden`.
    #[test]
    fn kitchen_sink_matches_the_golden_snapshot() {
        let actual = HtmlRenderer::new(640, 480).render(kitchen_sink());
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/render_html/snapshots/kitchen_sink.html"
        );
        if std::env::var_os("SCHNELLUI_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(path, actual.as_str()).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(path).expect("golden snapshot is committed");
        assert!(
            actual.as_str() == expected,
            "kitchen_sink output changed; review the diff and regenerate with \
             SCHNELLUI_UPDATE_SNAPSHOTS=1"
        );
    }

    #[test]
    fn every_emitted_data_sui_role_comes_from_the_role_table() {
        let html = HtmlRenderer::new(640, 480)
            .render(kitchen_sink())
            .into_string();
        let names: Vec<&str> = Role::ALL.iter().map(|r| r.dom_name()).collect();
        let needle = "data-sui-role=\"";
        let mut rest = html.as_str();
        let mut seen = Vec::new();
        while let Some(at) = rest.find(needle) {
            rest = &rest[at + needle.len()..];
            let name = &rest[..rest.find('"').unwrap()];
            assert!(names.contains(&name), "role {name} is not in the table");
            seen.push(name.to_string());
        }
        // The text input paths take their roles from the table too.
        assert!(seen.contains(&Role::PasswordInput.dom_name().to_string()));
        assert!(seen.contains(&Role::TextInput.dom_name().to_string()));
        let source = include_str!("template.rs");
        assert!(!source.contains("\"password-input\""));
        assert!(!source.contains("\"text-input\""));
    }

    #[test]
    fn responsive_class_ids_depend_on_the_rule_not_render_order() {
        use schnellui::template::{px, Template as _};

        let shared =
            || Text::new("narrow").show_when(ResponsiveQuery::viewport().max_width(px(400.0)));
        let other =
            || Text::new("wide").show_when(ResponsiveQuery::viewport().min_width(px(900.0)));
        let renderer = HtmlRenderer::new(800, 600);
        let first = renderer.render(column().child(shared())).into_string();
        let second = renderer
            .render(column().child(other()).child(shared()).child(shared()))
            .into_string();
        let class_of = |html: &str, text: &str| {
            let end = html.find(&format!(">{text}<")).unwrap();
            let start = html[..end].rfind("sui-responsive-").unwrap();
            html[start..start + "sui-responsive-".len() + 16].to_string()
        };
        let narrow = class_of(&first, "narrow");
        assert_eq!(narrow, class_of(&second, "narrow"));
        assert_ne!(narrow, class_of(&second, "wide"));
        // The rule used twice is emitted once, and the new rule is in the sheet.
        assert_eq!(
            second
                .matches(&format!(".{narrow} {{ display: none; }}"))
                .count(),
            1
        );
        assert!(second.contains("(min-width: 900px)"));
    }

    #[test]
    fn per_update_call_passes_the_markup_as_an_argument() {
        use crate::render_html::scripts::{patch_call_params, PATCH_FUNCTION, RECONCILE_SCRIPT};

        let small = HtmlRenderer::new(320, 180).render_page(Text::new("x"));
        let big = HtmlRenderer::new(320, 180).render_page(Text::new("row ".repeat(5000)));
        assert!(big.body().len() > small.body().len() * 50);

        let style = small.style_update(None);
        let small_call = patch_call_params(small.body(), style.as_deref());
        let big_call = patch_call_params(big.body(), style.as_deref());
        // The function declaration is constant: the markup is an argument value.
        assert_eq!(
            small_call.function_declaration,
            big_call.function_declaration
        );
        assert!(small_call.function_declaration.contains(PATCH_FUNCTION));
        assert!(!big_call.function_declaration.contains("function "));
        assert!(!big_call.function_declaration.contains("row row"));
        let values = |call: &chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams| {
            call.arguments
                .iter()
                .flatten()
                .map(|argument| argument.value.clone().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(values(&big_call)[0], serde_json::json!(big.body()));
        assert_eq!(values(&small_call)[1], serde_json::json!(style));
        // No stylesheet change travels as null.
        let unchanged = patch_call_params(big.body(), None);
        assert_eq!(values(&unchanged)[1], serde_json::Value::Null);

        // The install script defines the function once.
        assert_eq!(
            RECONCILE_SCRIPT
                .matches(&format!("globalThis.{PATCH_FUNCTION} ="))
                .count(),
            1
        );
        assert_eq!(
            RECONCILE_SCRIPT.matches("function patchChildren").count(),
            1
        );
    }

    #[test]
    fn stylesheet_is_sent_only_when_the_responsive_rules_change() {
        use schnellui::template::{px, Template as _};

        let renderer = HtmlRenderer::new(320, 180);
        let plain = renderer.render_page(Text::new("x"));
        let same = renderer.render_page(Text::new("y"));
        let responsive = renderer.render_page(
            Text::new("x").show_when(ResponsiveQuery::viewport().max_width(px(400.0))),
        );
        assert_eq!(plain.style_hash(), same.style_hash());
        assert!(plain.style_update(None).is_some());
        assert!(same.style_update(Some(plain.style_hash())).is_none());
        // A view that adds responsive rules needs the stylesheet re-sent.
        assert_ne!(plain.style_hash(), responsive.style_hash());
        let update = responsive.style_update(Some(plain.style_hash())).unwrap();
        assert!(update.contains("(max-width: 400px)"));
    }

    #[test]
    fn live_pages_compose_the_same_document_as_render() {
        let renderer = HtmlRenderer::new(320, 180);
        let page = renderer.render_page(kitchen_sink());
        let style = page.style();
        assert_eq!(page.document(), renderer.render(kitchen_sink()));
        assert!(page.document().as_str().contains(&style));
        // The theme part is built once and shared by clones of the renderer.
        assert!(Arc::ptr_eq(
            &page.styles,
            &renderer.clone().render_page(Text::new("x")).styles
        ));
        let mut themed = renderer.clone();
        themed.set_theme(Theme::default());
        assert!(!Arc::ptr_eq(
            &page.styles,
            &themed.render_page(Text::new("x")).styles
        ));
    }

    #[test]
    fn generated_document_style_carries_the_id_the_reconciler_looks_up() {
        let doc = HtmlRenderer::new(320, 180).render(Text::new("x"));
        let id = "schnellui-native-styles";
        assert!(doc.as_str().contains(&format!("<style id=\"{id}\">")));
        assert!(crate::render_html::scripts::RECONCILE_SCRIPT
            .contains(&format!("getElementById('{id}')")));
        assert_eq!(doc.as_str().matches("<style").count(), 1);
    }

    #[test]
    fn live_style_element_is_owned_by_the_reconciler() {
        let script = crate::render_html::scripts::RECONCILE_SCRIPT;
        assert!(script.contains("document.createElement('style')"));
        assert!(!script.contains("querySelector('head style')"));
    }

    /// Runs the reconciler against a fake DOM under node or deno.
    #[test]
    fn reconciler_handles_keyed_lists_and_focus() {
        use std::process::Command;

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("render_html");
        let test = dir.join("reconcile_test.mjs");
        let script = dir.join("reconcile.js");
        let runtimes: [(&str, &[&str]); 2] = [("node", &[]), ("deno", &["run", "-A"])];
        for (program, prefix) in runtimes {
            let output = Command::new(program)
                .args(prefix)
                .arg(&test)
                .arg(&script)
                .output();
            match output {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    assert!(
                        output.status.success(),
                        "{program} reconciler tests failed:\n{stdout}{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    assert!(stdout.contains("ok keyed reorder of 1000"), "{stdout}");
                    return;
                }
                Err(_) => continue,
            }
        }
        eprintln!("skipping reconciler JS tests: neither node nor deno is installed");
    }

    #[test]
    fn literal_data_sui_role_names_come_from_the_role_table() {
        let source = include_str!("template.rs");
        let names: Vec<&str> = Role::ALL.iter().map(|r| r.dom_name()).collect();
        let mut rest = source;
        let needle = "data-sui-role=\"";
        while let Some(at) = rest.find(needle) {
            rest = &rest[at + needle.len()..];
            let name = &rest[..rest.find('"').unwrap()];
            assert!(
                name.starts_with('{') || names.contains(&name),
                "unknown role literal {name}"
            );
        }
    }

    #[test]
    fn responsive_templates_emit_media_and_container_queries() {
        use schnellui::template::{em, px, ComponentRef, Template as _};

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
        use schnellui::template::{ComponentRef, Template as _};

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
        use schnellui::widgets::{ComponentInteractions, InteractionStates, InteractionStyle};

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
