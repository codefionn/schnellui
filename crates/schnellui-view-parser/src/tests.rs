use super::*;
use quote::quote;

fn lit_str(s: &str) -> LitStr {
    syn::parse2::<LitStr>(quote!(#s)).unwrap()
}

#[test]
fn static_text_is_static() {
    let n = Node::StaticText(lit_str("Counter"));
    assert!(n.is_static());
}

#[test]
fn dynamic_slot_is_not_static() {
    let expr: Expr = syn::parse2(quote!(count)).unwrap();
    let n = Node::Dynamic(expr);
    assert!(!n.is_static());
}

#[test]
fn element_static_iff_subtree_static() {
    let ident: Ident = syn::parse2(quote!(column)).unwrap();
    let dynamic: Expr = syn::parse2(quote!(count)).unwrap();
    let e = Element {
        tag: ident.clone(),
        attrs: vec![],
        children: vec![Node::StaticText(lit_str("title")), Node::Dynamic(dynamic)],
    };
    assert!(!Node::Element(e).is_static());

    let e2 = Element {
        tag: ident,
        attrs: vec![],
        children: vec![Node::StaticText(lit_str("a"))],
    };
    assert!(Node::Element(e2).is_static());
}

#[test]
fn counts_dynamic_sites() {
    let ident: Ident = syn::parse2(quote!(column)).unwrap();
    let dynamic: Expr = syn::parse2(quote!(count)).unwrap();
    let tree = ViewTree {
        roots: vec![Node::Element(Element {
            tag: ident,
            attrs: vec![Attr {
                name: AttrName::Prop(syn::parse2(quote!(class)).unwrap()),
                value: AttrValue::Dynamic(syn::parse2(quote!(cls)).unwrap()),
            }],
            children: vec![Node::StaticText(lit_str("t")), Node::Dynamic(dynamic)],
        })],
    };
    assert_eq!(tree.dynamic_site_count(), 2); // one dyn attr + one dyn child
}

// --- parser: grammar over real token streams (SOUL §3.3) ---

#[test]
fn parses_counter_view() {
    let tree = parse_view(quote! {
        column {
            text(size = 24.0) { "Counter" }
            text { (count.get()) }
            button(on:click = move || {}) { "increment" }
        }
    })
    .unwrap();

    assert_eq!(tree.roots.len(), 1);
    let Node::Element(col) = &tree.roots[0] else {
        panic!("root is not an element");
    };
    assert_eq!(col.tag.to_string(), "column");
    assert_eq!(col.children.len(), 3);

    // child 0: `text(size = 24.0) { "Counter" }` — static prop + static child.
    let Node::Element(t0) = &col.children[0] else {
        panic!("child 0 not element");
    };
    assert_eq!(t0.tag.to_string(), "text");
    assert_eq!(t0.attrs.len(), 1);
    assert!(matches!(t0.attrs[0].name, AttrName::Prop(_)));
    assert!(t0.attrs[0].value.is_static());
    assert!(matches!(t0.children[0], Node::StaticText(_)));
    assert!(col.children[0].is_static());

    // child 1: `text { (count.get()) }` — a dynamic slot.
    let Node::Element(t1) = &col.children[1] else {
        panic!("child 1 not element");
    };
    assert!(matches!(t1.children[0], Node::Dynamic(_)));
    assert!(!col.children[1].is_static());

    // child 2: `button(on:click = …) { "increment" }` — an event binding.
    let Node::Element(b) = &col.children[2] else {
        panic!("child 2 not element");
    };
    assert_eq!(b.tag.to_string(), "button");
    assert!(matches!(b.attrs[0].name, AttrName::Event(_)));
    assert!(!b.attrs[0].value.is_static());
}

#[test]
fn parses_event_name() {
    let tree = parse_view(quote! { button(on:click = move || {}) { "x" } }).unwrap();
    let Node::Element(b) = &tree.roots[0] else {
        panic!()
    };
    match &b.attrs[0].name {
        AttrName::Event(ev) => assert_eq!(ev.to_string(), "click"),
        _ => panic!("expected an event binding"),
    }
}

#[test]
fn plain_colon_without_on_is_error() {
    // `foo:bar = …` is rejected — only `on:<event>` uses `:`.
    assert!(parse_view(quote! { button(foo:bar = 1) { "x" } }).is_err());
}

#[test]
fn childless_element_parses() {
    let tree = parse_view(quote! { column { spacer } }).unwrap();
    let Node::Element(col) = &tree.roots[0] else {
        panic!()
    };
    let Node::Element(sp) = &col.children[0] else {
        panic!()
    };
    assert_eq!(sp.tag.to_string(), "spacer");
    assert!(sp.attrs.is_empty());
    assert!(sp.children.is_empty());
}

#[test]
fn parsed_dynamic_site_count_matches() {
    let tree = parse_view(quote! {
        column {
            text { "Counter" }
            text { (count.get()) }
            button(on:click = move || {}) { "increment" }
        }
    })
    .unwrap();
    // one dynamic child + one dynamic (event) attr.
    assert_eq!(tree.dynamic_site_count(), 2);
}

#[test]
fn trailing_comma_in_attrs() {
    let tree = parse_view(quote! { slider(value = 1.0, min = 0.0, max = 2.0,) {} }).unwrap();
    let Node::Element(s) = &tree.roots[0] else {
        panic!()
    };
    assert_eq!(s.attrs.len(), 3);
}

// --- valueless flag attrs + enum-keyword attrs (SOUL §8.1) ---

#[test]
fn parses_valueless_flag_attr() {
    // `ellipsis` carries no `= value` → a Flag, and a Flag is static.
    let tree = parse_view(quote! { text(ellipsis) { "x" } }).unwrap();
    let Node::Element(t) = &tree.roots[0] else {
        panic!()
    };
    assert_eq!(t.attrs.len(), 1);
    assert!(matches!(t.attrs[0].name, AttrName::Prop(ref p) if p == "ellipsis"));
    assert!(matches!(t.attrs[0].value, AttrValue::Flag));
    assert!(t.attrs[0].value.is_static());
    // a flag does not add a dynamic site, and the subtree stays static.
    assert_eq!(tree.dynamic_site_count(), 0);
    assert!(tree.roots[0].is_static());
}

#[test]
fn flag_attr_before_comma_and_valued_attr() {
    // a flag composes with valued attrs in any position, trailing comma ok.
    let tree = parse_view(quote! { text(ellipsis, size = 12.0,) { "x" } }).unwrap();
    let Node::Element(t) = &tree.roots[0] else {
        panic!()
    };
    assert_eq!(t.attrs.len(), 2);
    assert!(matches!(t.attrs[0].value, AttrValue::Flag));
    assert!(t.attrs[1].value.is_static());
}

#[test]
fn wrap_and_align_keywords_parse_as_bare_idents() {
    let tree = parse_view(quote! { text(wrap = word, align = center) { "x" } }).unwrap();
    let Node::Element(t) = &tree.roots[0] else {
        panic!()
    };
    assert_eq!(t.attrs.len(), 2);
    // keyword values are bare path idents (Dynamic), recognized at codegen.
    assert_eq!(
        as_bare_ident(&t.attrs[0].value).unwrap().to_string(),
        "word"
    );
    assert_eq!(
        as_bare_ident(&t.attrs[1].value).unwrap().to_string(),
        "center"
    );
}

#[test]
fn event_flag_without_handler_is_error() {
    // `on:click` with no `= handler` is rejected (events need a handler).
    assert!(parse_view(quote! { button(on:click) { "x" } }).is_err());
}

#[test]
fn as_bare_ident_rejects_qualified_path() {
    // a qualified path is an escape-hatch expr, not a keyword.
    let tree = parse_view(quote! { text(wrap = WrapMode::Word) { "x" } }).unwrap();
    let Node::Element(t) = &tree.roots[0] else {
        panic!()
    };
    assert!(as_bare_ident(&t.attrs[0].value).is_none());
}

// --- codegen: the typed builder chain (SOUL §3.3) ---

fn emit(tokens: TokenStream) -> String {
    let tree = parse_view(tokens).unwrap();
    Codegen::new(RenderMode::Native).emit(&tree).to_string()
}

#[test]
fn codegen_counter_builder_chain() {
    let tree = parse_view(quote! {
        column {
            text(size = 24.0) { "Counter" }
            text { (count.get()) }
            button(on:click = move || { count.set(count.get() + 1); }) { "increment" }
        }
    })
    .unwrap();
    let ts = Codegen::new(RenderMode::Native).emit(&tree);

    // The emitted tokens must be a syntactically valid Rust expression.
    syn::parse2::<Expr>(ts.clone())
        .unwrap_or_else(|e| panic!("emit is not a valid expr: {e}\n{ts}"));

    let s = ts.to_string();
    assert!(s.contains("Column"), "{s}");
    assert!(s.contains("Text :: new"), "{s}");
    assert!(s.contains("Text :: dynamic"), "{s}");
    assert!(s.contains("Button :: new"), "{s}");
    assert!(s.contains(". size"), "{s}");
    assert!(s.contains(". child"), "{s}");
    assert!(s.contains("on_click"), "{s}");
    // static label stays a plain string literal; no effect wiring on it.
    assert!(s.contains("\"increment\""), "{s}");
}

#[test]
fn codegen_bare_children_become_text() {
    let s = emit(quote! { row { "hi" (x) } });
    assert!(s.contains("Row"), "{s}");
    assert!(s.contains("Text :: new"), "{s}"); // bare "hi"
    assert!(s.contains("Text :: dynamic"), "{s}"); // bare (x)
}

#[test]
fn codegen_event_maps_to_on_method() {
    let s = emit(quote! { slider(on:change = move |_v| {}) {} });
    assert!(s.contains("Slider :: new"), "{s}");
    assert!(s.contains("on_change"), "{s}");
}

#[test]
fn codegen_password_input_builds_the_protected_widget() {
    let s = emit(quote! {
        password_input(value = "secret", label = "API key", on:input = move |_| {}) {}
    });
    assert!(s.contains("PasswordInput :: new"), "{s}");
    assert!(s.contains(". label"), "{s}");
    assert!(s.contains(". on_input"), "{s}");
}

#[test]
fn codegen_dialog_builder_chain() {
    let s = emit(quote! {
        dialog(title = "Delete project", position = bottom_right, fixed, persistent, non_decorated) {
            text { "This cannot be undone." }
            button { "Cancel" }
        }
    });
    assert!(s.contains("Dialog :: new"), "{s}");
    assert!(s.contains("\"Delete project\""), "{s}");
    assert!(s.contains("DialogPosition :: BottomRight"), "{s}");
    assert!(s.contains(". fixed ()"), "{s}");
    assert!(s.contains(". persistent ()"), "{s}");
    assert!(s.contains(". non_decorated ()"), "{s}");
    assert!(s.matches(". child").count() >= 2, "{s}");
}

// --- text wrapping / alignment / ellipsis lowering (SOUL §8.1) ---

#[test]
fn codegen_text_wrap_word_lowers_to_enum() {
    let s = emit(quote! { text(wrap = word) { "hello" } });
    assert!(s.contains("Text :: new"), "{s}");
    assert!(s.contains(". wrap"), "{s}");
    assert!(s.contains("WrapMode :: Word"), "{s}");
}

#[test]
fn codegen_text_wrap_anywhere_lowers_to_enum() {
    let s = emit(quote! { text(wrap = anywhere) { "hello" } });
    assert!(s.contains("WrapMode :: Anywhere"), "{s}");
}

#[test]
fn codegen_text_align_center_lowers_to_enum() {
    let s = emit(quote! { text(align = center) { "hi" } });
    assert!(s.contains(". align"), "{s}");
    assert!(s.contains("TextAlign :: Center"), "{s}");
}

#[test]
fn codegen_text_ellipsis_is_nullary_flag() {
    let s = emit(quote! { text(ellipsis) { "hi" } });
    // a flag lowers to a *nullary* setter, no argument.
    assert!(s.contains(". ellipsis ()"), "{s}");
}

#[test]
fn codegen_text_wrap_align_ellipsis_combined_is_valid_expr() {
    let tree =
        parse_view(quote! { text(wrap = word, align = center, ellipsis) { "paragraph" } }).unwrap();
    let ts = Codegen::new(RenderMode::Native).emit(&tree);
    syn::parse2::<Expr>(ts.clone())
        .unwrap_or_else(|e| panic!("emit is not a valid expr: {e}\n{ts}"));
    let s = ts.to_string();
    assert!(s.contains("WrapMode :: Word"), "{s}");
    assert!(s.contains("TextAlign :: Center"), "{s}");
    assert!(s.contains(". ellipsis ()"), "{s}");
}

#[test]
fn codegen_wrap_keyword_maps_nowrap_and_none() {
    // both `nowrap` and `none` spell the NoWrap variant.
    assert!(emit(quote! { text(wrap = nowrap) { "x" } }).contains("WrapMode :: NoWrap"));
    assert!(emit(quote! { text(wrap = none) { "x" } }).contains("WrapMode :: NoWrap"));
}

#[test]
fn codegen_wrap_qualified_path_passes_through() {
    // a qualified path is an escape hatch — passed through, not remapped.
    let s = emit(quote! { text(wrap = ::schnellui_widgets::WrapMode::Word) { "x" } });
    assert!(s.contains(". wrap"), "{s}");
    // no double-qualification: it is used verbatim.
    assert!(
        s.contains(":: schnellui_widgets :: WrapMode :: Word"),
        "{s}"
    );
}

#[test]
fn codegen_unknown_wrap_keyword_is_compile_error() {
    let s = emit(quote! { text(wrap = sideways) { "x" } });
    assert!(s.contains("compile_error"), "{s}");
    assert!(s.contains("unknown `wrap` keyword"), "{s}");
}

#[test]
fn codegen_unknown_align_keyword_is_compile_error() {
    let s = emit(quote! { text(align = middle) { "x" } });
    assert!(s.contains("compile_error"), "{s}");
    assert!(s.contains("unknown `align` keyword"), "{s}");
}

// --- responsive flex containers (SOUL §8.1: justify/align/wrap/flex) ---

#[test]
fn codegen_container_justify_lowers_to_enum() {
    let s = emit(quote! { row(justify = space_between) { "a" "b" } });
    assert!(s.contains("Row :: new"), "{s}");
    assert!(s.contains(". justify"), "{s}");
    assert!(s.contains("Justify :: SpaceBetween"), "{s}");
}

#[test]
fn codegen_container_align_lowers_to_flex_enum() {
    // container `align` maps to the layout Align (stretch exists), not TextAlign.
    let s = emit(quote! { column(align = stretch) { "a" } });
    assert!(s.contains("Align :: Stretch"), "{s}");
    assert!(!s.contains("TextAlign"), "{s}");
}

#[test]
fn codegen_container_wrap_is_nullary_flag() {
    let s = emit(quote! { row(wrap, gap = 8.0) { "a" "b" } });
    assert!(s.contains(". wrap ()"), "{s}");
    assert!(s.contains(". gap"), "{s}");
}

#[test]
fn codegen_container_width_passes_through() {
    // per-axis definite width for a responsive wrap line (SOUL §8.1).
    let s = emit(quote! { row(width = 320.0, wrap) { "a" } });
    assert!(s.contains(". width (320.0)"), "{s}");
}

#[test]
fn codegen_container_fill_is_nullary_flag() {
    // `fill` sizes the container to its parent / the viewport (SOUL §8.1).
    let s = emit(quote! { column(fill, align = stretch) { "a" } });
    assert!(s.contains(". fill ()"), "{s}");
    assert!(s.contains("Align :: Stretch"), "{s}");
}

#[test]
fn codegen_flex_wraps_single_child() {
    let tree = parse_view(quote! { flex(grow = 1.0, basis = 120.0) { button { "b" } } });
    let ts = Codegen::new(RenderMode::Native).emit(&tree.unwrap());
    syn::parse2::<Expr>(ts.clone())
        .unwrap_or_else(|e| panic!("emit is not a valid expr: {e}\n{ts}"));
    let s = ts.to_string();
    assert!(s.contains("Flex :: new"), "{s}");
    assert!(s.contains(". grow (1.0)"), "{s}");
    assert!(s.contains(". basis (120.0)"), "{s}");
    assert!(s.contains(". child"), "{s}");
    assert!(s.contains("Button :: new"), "{s}");
}

#[test]
fn codegen_childless_flex_is_weighted_spacer() {
    let s = emit(quote! { flex(grow = 2.0) });
    assert!(s.contains("Flex :: new"), "{s}");
    assert!(s.contains(". grow (2.0)"), "{s}");
    assert!(!s.contains(". child"), "{s}");
}

#[test]
fn codegen_flex_with_two_children_is_compile_error() {
    let s = emit(quote! { flex(grow = 1.0) { "a" "b" } });
    assert!(s.contains("compile_error"), "{s}");
    assert!(s.contains("at most one child"), "{s}");
}

#[test]
fn codegen_unknown_justify_keyword_is_compile_error() {
    let s = emit(quote! { row(justify = between) { "a" } });
    assert!(s.contains("compile_error"), "{s}");
    assert!(s.contains("unknown `justify` keyword"), "{s}");
}

#[test]
fn codegen_wrap_without_value_is_compile_error() {
    // a flag-form `wrap` has no mode to apply → helpful error.
    let s = emit(quote! { text(wrap) { "x" } });
    assert!(s.contains("compile_error"), "{s}");
    assert!(s.contains("needs a value"), "{s}");
}

#[test]
fn codegen_dynamic_wrapped_text_lowers_both() {
    // wrapping composes with a dynamic content slot.
    let s = emit(quote! { text(wrap = anywhere, align = end) { (msg.get()) } });
    assert!(s.contains("Text :: dynamic"), "{s}");
    assert!(s.contains("WrapMode :: Anywhere"), "{s}");
    assert!(s.contains("TextAlign :: End"), "{s}");
}

#[test]
fn codegen_constructor_args_are_consumed_not_chained() {
    // value/min/max feed the ctor and must NOT reappear as chained setters.
    let s = emit(quote! { slider(value = 3.0, min = 0.0, max = 10.0) {} });
    assert!(s.contains("Slider :: new (3.0 , 0.0 , 10.0)"), "{s}");
    assert!(!s.contains(". value"), "{s}");
}

#[test]
fn codegen_progress_builds_progressbar_with_ctor_args() {
    let s = emit(quote! { progress(value = 50.0, min = 0.0, max = 100.0) {} });
    assert!(s.contains("ProgressBar :: new (50.0 , 0.0 , 100.0)"), "{s}");
    // value/min/max feed the ctor and must NOT reappear as chained setters.
    assert!(!s.contains(". value"), "{s}");
}

#[test]
fn codegen_spinner_builds_loading_spinner_and_chains_options() {
    let s = emit(quote! { spinner(size = 32.0, name = "Loading files") {} });
    assert!(s.contains("LoadingSpinner :: new"), "{s}");
    assert!(s.contains(". size (32.0)"), "{s}");
    assert!(s.contains(". name (\"Loading files\")"), "{s}");
}

#[test]
fn codegen_switch_builds_and_binds_on_toggle() {
    let s = emit(quote! { switch(on = true, on:toggle = move |_v| {}) {} });
    assert!(s.contains("Switch :: new"), "{s}");
    assert!(s.contains("Switch :: new (true)"), "{s}");
    // `on:toggle` binds the handler exactly like a checkbox's does.
    assert!(s.contains("on_toggle"), "{s}");
    // the `on` ctor arg is consumed, not re-chained as `.on(...)`.
    assert!(!s.contains(". on ("), "{s}");
}

#[test]
fn codegen_radio_builds_and_binds_on_select() {
    let s = emit(quote! { radio(selected = true, on:select = move || {}) {} });
    assert!(s.contains("Radio :: new (true)"), "{s}");
    assert!(s.contains("on_select"), "{s}");
}

#[test]
fn codegen_divider_builds_with_no_args() {
    let s = emit(quote! { column { divider } });
    assert!(s.contains("Divider :: new"), "{s}");
}

#[test]
fn codegen_divider_thickness_chains() {
    let s = emit(quote! { divider(thickness = 2.0) {} });
    assert!(s.contains("Divider :: new"), "{s}");
    assert!(s.contains(". thickness"), "{s}");
}

#[test]
fn codegen_link_builds_and_binds_on_click() {
    let s = emit(quote! { link(on:click = move || {}) { "docs" } });
    assert!(s.contains("Link :: new"), "{s}");
    assert!(s.contains("\"docs\""), "{s}");
    assert!(s.contains("on_click"), "{s}");
}

#[test]
fn codegen_badge_builds_from_static_child() {
    let s = emit(quote! { badge { "3" } });
    assert!(s.contains("Badge :: new"), "{s}");
    assert!(s.contains("\"3\""), "{s}");
}

#[test]
fn codegen_tabs_builds_tabbar_with_selected_tab_children() {
    let s = emit(quote! {
        tabs(gap = 4.0, on:reorder = move |_from, _to| {}) {
            tab(selected = true, on:select = move || {}) { "general" }
            tab { "privacy" }
        }
    });
    assert!(s.contains("TabBar :: new"), "{s}");
    assert!(s.contains(". gap"), "{s}");
    assert!(s.contains("on_reorder"), "{s}");
    assert!(s.contains("Tab :: new"), "{s}");
    assert!(s.contains("\"general\""), "{s}");
    assert!(s.contains(". selected (true)"), "{s}");
    assert!(s.contains("on_select"), "{s}");
    assert!(s.contains(". child"), "{s}");
}

#[test]
fn codegen_grouped_tabs_builds_recursive_tab_nodes() {
    let s = emit(quote! {
        grouped_tabs(tree, indent = 18.0, group_gap = 10.0) {
            tab_group(label = "Workspace") {
                tab_node(
                    label = "Editor",
                    selected = true,
                    action = ::schnellui_widgets::Button::new("Refresh")
                ) {
                    tab_node(label = "Outline", expanded = false) {}
                }
                tab { "Terminal" }
            }
        }
    });
    assert!(s.contains("GroupedTabList :: new"), "{s}");
    assert!(s.contains(". tree ()"), "{s}");
    assert!(s.contains(". indent (18.0)"), "{s}");
    assert!(s.contains("TabGroup :: new (\"Workspace\")"), "{s}");
    assert!(s.contains("TabNode :: new (\"Editor\")"), "{s}");
    assert!(s.contains(". selected (true)"), "{s}");
    assert!(s.contains(". action"), "{s}");
    assert!(s.contains("Button :: new (\"Refresh\")"), "{s}");
    assert!(s.contains("TabNode :: new (\"Outline\")"), "{s}");
    assert!(s.contains(". expanded (false)"), "{s}");
    assert!(s.contains("Tab :: new (\"Terminal\")"), "{s}");
}

#[test]
fn codegen_list_builds_with_items_and_on_select() {
    let s = emit(quote! {
        list {
            list_item(selected = true) { "inbox" }
            list_item(on:select = move || {}) { "archive" }
        }
    });
    assert!(s.contains("List :: new"), "{s}");
    assert!(s.contains("ListItem :: new"), "{s}");
    assert!(s.contains("\"inbox\""), "{s}");
    assert!(s.contains(". selected (true)"), "{s}");
    assert!(s.contains("on_select"), "{s}");
}

#[test]
fn codegen_table_builds_rows_headers_and_cells() {
    let s = emit(quote! {
        table(selected_row = 1, on:select_row = move |_i| {}) {
            table_row(header) { "Name" "Age" }
            table_row { "Ada" "36" }
            tr { "Grace" "85" }
        }
    });
    assert!(s.contains("Table :: new"), "{s}");
    assert!(s.contains(". selected_row (1)"), "{s}");
    assert!(s.contains("on_select_row"), "{s}");
    assert!(s.contains(". push_row"), "{s}");
    assert!(s.contains("TableRow :: new"), "{s}");
    // the valueless `header` flag lowers to the nullary `.header()`
    assert!(s.contains(". header ()"), "{s}");
    assert!(s.contains(". cell (\"Name\")"), "{s}");
    assert!(s.contains(". cell (\"85\")"), "{s}");
}

#[test]
fn codegen_table_column_lowers_sort_direction_and_action() {
    let s = emit(quote! {
        table {
            table_row(header) {
                table_column(sort = asc, on:sort = move |_direction| {}) { "Name" }
                th(sort = desc) { "Age" }
                "City"
            }
            table_row { "Ada" "36" "London" }
        }
    });
    assert!(s.contains("TableColumn :: new (\"Name\")"), "{s}");
    assert!(s.contains("SortDirection :: Ascending"), "{s}");
    assert!(s.contains("on_sort"), "{s}");
    assert!(s.contains("TableColumn :: new (\"Age\")"), "{s}");
    assert!(s.contains("SortDirection :: Descending"), "{s}");
    assert!(s.contains(". cell (\"City\")"), "{s}");
}

#[test]
fn codegen_table_column_rejects_data_rows_and_unknown_directions() {
    for tokens in [
        quote! { table { table_row { table_column { "Name" } } } },
        quote! { table { table_row(header) { th(sort = sideways) { "Name" } } } },
    ] {
        let tree = parse_view(tokens).unwrap();
        let s = Codegen::new(RenderMode::Native).emit(&tree).to_string();
        assert!(s.contains("compile_error"), "{s}");
    }
}

#[test]
fn codegen_svg_builds_from_markup_child_and_chains_alt() {
    let s = emit(quote! {
        svg(alt = "logo", width = 24.0) { "<svg viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"10\"/></svg>" }
    });
    assert!(s.contains("Svg :: new"), "{s}");
    assert!(s.contains("viewBox"), "{s}");
    assert!(s.contains(". alt (\"logo\")"), "{s}");
    assert!(s.contains(". width (24.0)"), "{s}");
}

#[test]
fn codegen_table_rejects_non_row_children() {
    for tokens in [
        quote! { table { text { "loose" } } },
        quote! { table { "loose" } },
    ] {
        let tree = parse_view(tokens).unwrap();
        let s = Codegen::new(RenderMode::Native).emit(&tree).to_string();
        assert!(s.contains("compile_error"), "{s}");
    }
}

#[test]
fn codegen_table_row_rejects_dynamic_cells() {
    let tree = parse_view(quote! { table { table_row { (x) } } }).unwrap();
    let s = Codegen::new(RenderMode::Native).emit(&tree).to_string();
    assert!(s.contains("compile_error"), "{s}");
}

#[test]
fn codegen_labeled_leaf_rejects_non_literal_label() {
    // the shared `static_label` contract: a dynamic child is a compile error
    for tokens in [
        quote! { link { (x) } },
        quote! { badge { (x) } },
        quote! { tab { (x) } },
        quote! { list_item { (x) } },
    ] {
        let tree = parse_view(tokens).unwrap();
        let s = Codegen::new(RenderMode::Native).emit(&tree).to_string();
        assert!(s.contains("compile_error"), "{s}");
    }
}

#[test]
fn codegen_multiple_roots_is_compile_error() {
    let tree = parse_view(quote! { text { "a" } text { "b" } }).unwrap();
    let s = Codegen::new(RenderMode::Native).emit(&tree).to_string();
    assert!(s.contains("compile_error"), "{s}");
}

#[test]
fn codegen_unknown_tag_is_compile_error() {
    let s = emit(quote! { frobnicate { "x" } });
    assert!(s.contains("compile_error"), "{s}");
    assert!(s.contains("unknown view! element"), "{s}");
}

#[test]
fn render_mode_is_threaded() {
    assert_eq!(Codegen::new(RenderMode::Native).mode(), RenderMode::Native);
    assert_eq!(Codegen::new(RenderMode::WebGl2).mode(), RenderMode::WebGl2);
}
