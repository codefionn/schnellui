//! # schnellui-view-parser
//!
//! The `view!` parser + AST, kept in a **separate crate** from the proc-macro
//! (SOUL §3.3) — exactly as Dioxus (`dioxus-rsx`) and Sycamore
//! (`sycamore-view-parser`) do — so autoformat, hot-reload reparse, and tooling
//! don't re-invoke the compiler.
//!
//! The AST carries the **static/dynamic split** (SOUL §3.3, §3.4): every node
//! classifies as an invariant skeleton (hoistable to a `const`) or a dynamic slot
//! (wrapped in a `RenderEffect`). `schnellui-macro` consumes this via a dedicated
//! `Codegen` (never a blanket `ToTokens`) so it can thread render-mode.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{ParseStream, Parser};
use syn::token::{Brace, Paren};
use syn::{braced, parenthesized, Expr, ExprLit, Ident, LitStr, Token};

/// A parsed `view! { … }` body: an ordered list of top-level nodes.
#[derive(Clone)]
pub struct ViewTree {
    pub roots: Vec<Node>,
}

/// One node in the view AST (SOUL §3.3).
#[derive(Clone)]
pub enum Node {
    /// An element: a widget/container tag with attributes and children,
    /// e.g. `column { … }` or `button(on:click = …) { "x" }`.
    Element(Element),
    /// A fully static text literal child — hoistable to a `const` (§3.3).
    StaticText(LitStr),
    /// A dynamic slot: a bare signal read `(count)` or a `move || …` closure,
    /// compiled to a `RenderEffect` that mutates the retained node in place (§3.3).
    Dynamic(Expr),
}

/// An element node: `tag(attr = value, on:event = handler) { children }`.
#[derive(Clone)]
pub struct Element {
    /// the tag ident (`column`, `text`, `button`, …) — resolved against the
    /// `schnellui-widgets` builder set at codegen time (§3.3, §8.1).
    pub tag: Ident,
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
}

/// One attribute on an element.
#[derive(Clone)]
pub struct Attr {
    pub name: AttrName,
    pub value: AttrValue,
}

/// Attribute name kinds — `on:click` events are distinguished from plain props so
/// codegen wires them to `.on_*` handlers vs setters (§3.3, §6.3).
#[derive(Clone)]
pub enum AttrName {
    /// a plain property, e.g. `class`, `min`, `checked`.
    Prop(Ident),
    /// an event binding `on:<event>`, e.g. `on:click` (routes to the same handler
    /// as an inbound AccessKit `ActionRequest`, §6.3).
    Event(Ident),
}

/// An attribute value, classified static vs dynamic (SOUL §3.3).
#[derive(Clone)]
pub enum AttrValue {
    /// a compile-time-constant literal → part of the hoisted skeleton.
    Static(syn::Lit),
    /// a reactive expression / closure → a dynamic attr slot.
    Dynamic(Expr),
    /// a valueless flag attribute — a bare prop ident with no `= value`, e.g.
    /// `ellipsis`. Lowers to a **nullary** builder call (`.ellipsis()`). It is a
    /// compile-time-constant presence toggle, so it stays part of the hoisted
    /// skeleton and counts as static (SOUL §3.3, §8.1).
    Flag,
}

impl Node {
    /// `true` if this node (and, for elements, its whole subtree) is invariant —
    /// no signals, no dynamic attrs — and can be hoisted to a `const` (SOUL §3.3,
    /// Directive #4). This drives "work ∝ dynamic sites".
    pub fn is_static(&self) -> bool {
        match self {
            Node::StaticText(_) => true,
            Node::Dynamic(_) => false,
            Node::Element(e) => {
                e.attrs.iter().all(|a| a.value.is_static())
                    && e.children.iter().all(Node::is_static)
            }
        }
    }
}

impl AttrValue {
    /// `true` for a compile-time-constant value — a literal or a valueless flag —
    /// both of which are part of the hoisted skeleton (SOUL §3.3).
    pub fn is_static(&self) -> bool {
        matches!(self, AttrValue::Static(_) | AttrValue::Flag)
    }
}

impl ViewTree {
    /// Counts dynamic sites (bare-signal / closure slots + dynamic attrs) —
    /// the quantity all update work is proportional to (SOUL Directive #4).
    pub fn dynamic_site_count(&self) -> usize {
        fn count(n: &Node) -> usize {
            match n {
                Node::StaticText(_) => 0,
                Node::Dynamic(_) => 1,
                Node::Element(e) => {
                    e.attrs.iter().filter(|a| !a.value.is_static()).count()
                        + e.children.iter().map(count).sum::<usize>()
                }
            }
        }
        self.roots.iter().map(count).sum()
    }
}

/// Parses a `view! { … }` token body into a [`ViewTree`] (SOUL §3.3).
///
/// Hand-rolled on `syn` (the schnellui grammar is brace/paren-based, unlike
/// rstml's angle-bracket HTML): an ordered list of top-level nodes, where
///
/// ```text
/// node    = string-literal              // static text child
///         | "(" expr ")"               // dynamic slot (RenderEffect)
///         | element
/// element = ident ("(" attrs ")")? ("{" node* "}")?
/// attrs   = attr ("," attr)* ","?
/// attr    = (ident | "on" ":" ident) ("=" expr)?   // a bare prop ident with no
///                                                  // value is a flag (e.g. `ellipsis`)
/// ```
///
/// Kept compiler-independent so tooling and hot-reload can reparse without a
/// rebuild.
pub fn parse_view(input: TokenStream) -> syn::Result<ViewTree> {
    fn parse_roots(input: ParseStream) -> syn::Result<ViewTree> {
        let mut roots = Vec::new();
        while !input.is_empty() {
            roots.push(parse_node(input)?);
        }
        Ok(ViewTree { roots })
    }
    parse_roots.parse2(input)
}

/// Parses one node: a string literal, a `(expr)` dynamic slot, or an element.
fn parse_node(input: ParseStream) -> syn::Result<Node> {
    if input.peek(LitStr) {
        Ok(Node::StaticText(input.parse()?))
    } else if input.peek(Paren) {
        let content;
        parenthesized!(content in input);
        let expr: Expr = content.parse()?;
        if !content.is_empty() {
            return Err(content.error("unexpected tokens after `(expr)` dynamic slot"));
        }
        Ok(Node::Dynamic(expr))
    } else if input.peek(Ident) {
        Ok(Node::Element(parse_element(input)?))
    } else {
        Err(input.error("expected a string literal, a `(expr)` dynamic slot, or an element"))
    }
}

/// Parses `ident ("(" attrs ")")? ("{" children "}")?`.
fn parse_element(input: ParseStream) -> syn::Result<Element> {
    let tag: Ident = input.parse()?;

    let attrs = if input.peek(Paren) {
        let content;
        parenthesized!(content in input);
        parse_attrs(&content)?
    } else {
        Vec::new()
    };

    let children = if input.peek(Brace) {
        let content;
        braced!(content in input);
        let mut kids = Vec::new();
        while !content.is_empty() {
            kids.push(parse_node(&content)?);
        }
        kids
    } else {
        Vec::new()
    };

    Ok(Element {
        tag,
        attrs,
        children,
    })
}

/// Parses a comma-separated attribute list (trailing comma allowed).
fn parse_attrs(input: ParseStream) -> syn::Result<Vec<Attr>> {
    let mut attrs = Vec::new();
    while !input.is_empty() {
        attrs.push(parse_attr(input)?);
        if input.is_empty() {
            break;
        }
        input.parse::<Token![,]>()?;
    }
    Ok(attrs)
}

/// Parses one `(ident | "on" ":" ident) ("=" expr)?`, classifying the value as a
/// compile-time literal (static, hoistable), a reactive expression (dynamic), or —
/// when a plain prop carries no `= value` — a valueless [`AttrValue::Flag`]
/// (e.g. `ellipsis`, lowering to a nullary builder call, SOUL §8.1).
fn parse_attr(input: ParseStream) -> syn::Result<Attr> {
    let first: Ident = input.parse()?;
    let name = if input.peek(Token![:]) {
        input.parse::<Token![:]>()?;
        let event: Ident = input.parse()?;
        if first != "on" {
            return Err(syn::Error::new(
                first.span(),
                "only `on:<event>` event bindings use `:` — a plain prop is `name = value`",
            ));
        }
        AttrName::Event(event)
    } else {
        AttrName::Prop(first)
    };

    // No `= value` → a valueless flag. Only plain props may be flags; an
    // `on:<event>` binding always needs a handler.
    if !input.peek(Token![=]) {
        return match name {
            AttrName::Prop(_) => Ok(Attr {
                name,
                value: AttrValue::Flag,
            }),
            AttrName::Event(ev) => Err(syn::Error::new(
                ev.span(),
                "an `on:<event>` binding needs a handler: `on:click = move || …`",
            )),
        };
    }

    input.parse::<Token![=]>()?;
    let expr: Expr = input.parse()?;
    let value = match expr {
        // a bare literal (and nothing more) is part of the hoisted skeleton.
        Expr::Lit(ExprLit { lit, attrs }) if attrs.is_empty() => AttrValue::Static(lit),
        other => AttrValue::Dynamic(other),
    };
    Ok(Attr { name, value })
}

/// The render mode threaded through codegen (SOUL §3.3, §5). Lives here rather than
/// in the proc-macro crate because a `proc-macro` crate can only export macros — so
/// the codegen (like Dioxus's `dioxus-rsx`) sits alongside the AST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    /// native wgpu (Vulkan/Metal/DX12) or browser WebGPU.
    Native,
    /// WebGL2 fallback (sparse strips), no compute (§5).
    WebGl2,
}

/// Turns a parsed [`ViewTree`] into a typed builder-chain `TokenStream` with the
/// static/dynamic split materialized (SOUL §3.3). A dedicated struct (not a blanket
/// `ToTokens`) so it can thread [`RenderMode`].
pub struct Codegen {
    mode: RenderMode,
}

impl Codegen {
    /// A codegen targeting `mode`.
    pub fn new(mode: RenderMode) -> Codegen {
        Codegen { mode }
    }

    /// The render mode this codegen emits for.
    pub fn mode(&self) -> RenderMode {
        self.mode
    }

    /// Emits the builder chain for a view tree (SOUL §3.3): static subtrees become
    /// plain `schnellui-widgets` builder calls (built once when the setup fn runs,
    /// Directive #4), while each `(expr)` dynamic slot becomes the contract's
    /// dynamic-slot builder (`Text::dynamic`) wrapping the expression in a
    /// `move ||` closure so it re-runs through a render effect, and `on:<event>`
    /// binds to the matching `.on_<event>` handler (SOUL §6.3).
    ///
    /// A `view! { … }` yields exactly one root node; the emitted tokens are an
    /// expression evaluating to a `impl schnellui_widgets::View`.
    pub fn emit(&self, tree: &ViewTree) -> TokenStream {
        match tree.roots.as_slice() {
            [root] => {
                let root = self.emit_view(root);
                // Minimum-size props are extension methods on `View`, which keeps
                // them available to every component without duplicating fields
                // across all leaf builders. Import the trait inside the expansion
                // so `button(min_width = …)` works regardless of caller imports.
                quote! {{
                    use ::schnellui_widgets::View as _;
                    #root
                }}
            }
            [] => syn::Error::new(
                Span::call_site(),
                "view! { } is empty — expected exactly one root node",
            )
            .to_compile_error(),
            _ => syn::Error::new(
                Span::call_site(),
                "view! expects a single root node — wrap children in a `column { … }` or `row { … }`",
            )
            .to_compile_error(),
        }
    }

    /// Emits a node in *View position* (a container child or the root): a bare
    /// string/`(expr)` becomes an implicit `Text`, an element dispatches by tag.
    fn emit_view(&self, node: &Node) -> TokenStream {
        match node {
            Node::StaticText(s) => quote! { ::schnellui_widgets::Text::new(#s) },
            Node::Dynamic(e) => dynamic_text(e),
            Node::Element(el) => self.emit_element(el),
        }
    }

    /// Lowers one element to its typed `schnellui-widgets` constructor + chained
    /// builder methods (SOUL §3.3, §8.1). The tag selects the widget type and how
    /// its children are consumed (container `.child(…)` vs text/label content).
    fn emit_element(&self, el: &Element) -> TokenStream {
        let tag = el.tag.to_string();
        match tag.as_str() {
            // A node-transparent stable component reference. The explicit wrapper
            // form works around container builders needing their `.child(...)`
            // calls before an extension wrapper is applied:
            // `component_ref(value = card_ref) { column { ... } }`.
            "component_ref" | "referenced" => {
                if el.children.len() != 1 {
                    return err(
                        el.tag.span(),
                        "`component_ref { … }` requires exactly one child",
                    );
                }
                let reference = consumed(el, "value")
                    .or_else(|| consumed(el, "reference"))
                    .unwrap_or_else(|| {
                        syn::Error::new(
                            el.tag.span(),
                            "`component_ref` requires `value = <ComponentRef>`",
                        )
                        .to_compile_error()
                    });
                let methods = self.chained_methods(el, &["value", "reference"]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::Referenced::new(#reference) #methods #children }
            }
            // --- layout containers: children become `.child(…)` (SOUL §8.1).
            // `justify` / `align` take bare keywords mapped to the re-exported
            // flex enums (`row(justify = space_between)`); `wrap` is a valueless
            // flag lowered generically to the nullary `.wrap()` (SOUL §8.1). ---
            "column" | "row" | "stack" => {
                let ty = type_ident(&tag);
                let style = match container_style_methods(el) {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &["justify", "align"]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::#ty::new() #style #methods #children }
            }
            "scroll" => {
                let methods = self.chained_methods(el, &[]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::Scroll::new() #methods #children }
            }
            // --- flex: per-child responsive factors on its single child, or a
            // weighted spacer when childless (SOUL §8.1). All factor attrs
            // (`grow`/`shrink`/`basis`/`min_width`/…) lower generically. ---
            "flex" => {
                if el.children.len() > 1 {
                    return err(el.tag.span(), "`flex { … }` accepts at most one child");
                }
                let methods = self.chained_methods(el, &[]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::Flex::new() #methods #children }
            }
            "pad" => {
                let all = consumed(el, "all").unwrap_or_else(|| quote! { 0.0f32 });
                let methods = self.chained_methods(el, &["all"]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::Pad::all(#all) #methods #children }
            }
            "spacer" => {
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::Spacer::new() #methods }
            }
            // --- text: the single child *is* the content (static vs dynamic) ---
            "text" => {
                let base = match single_content(el) {
                    Ok(None) => quote! { ::schnellui_widgets::Text::new("") },
                    Ok(Some(Node::StaticText(s))) => quote! { ::schnellui_widgets::Text::new(#s) },
                    Ok(Some(Node::Dynamic(e))) => dynamic_text(e),
                    Ok(Some(Node::Element(_))) => {
                        return err(el.tag.span(), "`text { … }` child must be a string literal or a `(expr)` dynamic slot");
                    }
                    Err(e) => return e,
                };
                // `wrap = <keyword>` / `align = <keyword>` map to the re-exported
                // enum paths (SOUL §8.1); `ellipsis` is a flag handled generically
                // by `chained_methods` as a nullary `.ellipsis()` setter.
                let style = match text_style_methods(el) {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &["wrap", "align"]);
                quote! { #base #style #methods }
            }
            // --- button: the single static child is its label (SOUL §6.3) ---
            "button" => {
                let label = match static_label(el, "button") {
                    Ok(l) => l,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::Button::new(#label) #methods }
            }
            // --- link: an inline navigation action; label like a button's ---
            "link" => {
                let label = match static_label(el, "link") {
                    Ok(l) => l,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::Link::new(#label) #methods }
            }
            // --- badge: a short status pill; the single static child is its text ---
            "badge" => {
                let label = match static_label(el, "badge") {
                    Ok(l) => l,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::Badge::new(#label) #methods }
            }
            // --- tabs / tab: a semantic tab bar of exclusive tabs (SOUL §6.3).
            // `tab(selected = true, on:select = …) { "general" }` lowers the label
            // like a button's; `selected`/`on:select` chain generically. ---
            "tabs" | "tab_bar" | "tabbar" => {
                let methods = self.chained_methods(el, &[]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::TabBar::new() #methods #children }
            }
            "tab" => {
                let label = match static_label(el, "tab") {
                    Ok(l) => l,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::Tab::new(#label) #methods }
            }
            // --- grouped tabs: labelled sections whose recursive tab nodes can
            // render flat or as an indented tree. Labels are constructor props
            // because these elements use their children for nested structure. ---
            "grouped_tabs" | "grouped_tab_list" | "grouped_tablist" => {
                let methods = self.chained_methods(el, &[]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::GroupedTabList::new() #methods #children }
            }
            "tab_group" | "tabgroup" => {
                let label = consumed(el, "label").unwrap_or_else(|| quote! { "" });
                let methods = self.chained_methods(el, &["label"]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::TabGroup::new(#label) #methods #children }
            }
            "tab_node" | "tabnode" | "tree_tab" => {
                let label = consumed(el, "label").unwrap_or_else(|| quote! { "" });
                let methods = self.chained_methods(el, &["label"]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::TabNode::new(#label) #methods #children }
            }
            // --- list / list_item: a semantic single-selection list (SOUL §8.1) ---
            "list" => {
                let methods = self.chained_methods(el, &[]);
                let children = self.child_calls(el);
                quote! { ::schnellui_widgets::List::new() #methods #children }
            }
            "list_item" | "listitem" => {
                let label = match static_label(el, "list_item") {
                    Ok(l) => l,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::ListItem::new(#label) #methods }
            }
            // --- dialog: a semantic floating surface. `title` feeds the
            // constructor/accessibility name; `position = <keyword>` maps to the
            // placement enum; modal/modeless/fixed/non_fixed/persistent remain
            // ordinary valueless builder flags. ---
            "dialog" => {
                let title = consumed(el, "title").unwrap_or_else(|| quote! { "" });
                let position = match dialog_position_methods(el) {
                    Ok(methods) => methods,
                    Err(error) => return error,
                };
                let methods = self.chained_methods(el, &["title", "position"]);
                let children = self.child_calls(el);
                quote! {
                    ::schnellui_widgets::Dialog::new(#title)
                        #position #methods #children
                }
            }
            // --- table: rows of static cells with first-class semantics (SOUL
            // §8.1, §6.1). Children must be `table_row` elements, lowered to
            // `.push_row(TableRow::new()…)`; `selected_row = …` / `on:select_row
            // = …` chain generically. `table_row(header)`'s valueless flag lowers
            // to the nullary `.header()`. ---
            "table" => {
                let methods = self.chained_methods(el, &[]);
                let mut rows = TokenStream::new();
                for child in &el.children {
                    let row = match self.emit_table_row(el, child) {
                        Ok(r) => r,
                        Err(e) => return e,
                    };
                    rows.extend(quote! { .push_row(#row) });
                }
                quote! { ::schnellui_widgets::Table::new() #methods #rows }
            }
            "checkbox" => {
                let checked = consumed(el, "checked").unwrap_or_else(|| quote! { false });
                let methods = self.chained_methods(el, &["checked"]);
                quote! { ::schnellui_widgets::Checkbox::new(#checked) #methods }
            }
            "slider" => {
                let value = consumed(el, "value").unwrap_or_else(|| quote! { 0.0f32 });
                let min = consumed(el, "min").unwrap_or_else(|| quote! { 0.0f32 });
                let max = consumed(el, "max").unwrap_or_else(|| quote! { 1.0f32 });
                let methods = self.chained_methods(el, &["value", "min", "max"]);
                quote! { ::schnellui_widgets::Slider::new(#value, #min, #max) #methods }
            }
            // --- progress: a read-only range status (SOUL §8.1) ---
            "progress" => {
                let value = consumed(el, "value").unwrap_or_else(|| quote! { 0.0f32 });
                let min = consumed(el, "min").unwrap_or_else(|| quote! { 0.0f32 });
                let max = consumed(el, "max").unwrap_or_else(|| quote! { 1.0f32 });
                let methods = self.chained_methods(el, &["value", "min", "max"]);
                quote! { ::schnellui_widgets::ProgressBar::new(#value, #min, #max) #methods }
            }
            // --- spinner/loading_spinner: indeterminate progress ---
            "spinner" | "loading_spinner" => {
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::LoadingSpinner::new() #methods }
            }
            // --- switch: an on/off toggle; `on:toggle` binds like a checkbox's ---
            "switch" => {
                let on = consumed(el, "on").unwrap_or_else(|| quote! { false });
                let methods = self.chained_methods(el, &["on"]);
                quote! { ::schnellui_widgets::Switch::new(#on) #methods }
            }
            // --- radio: one exclusive option of a group (SOUL §6.3) ---
            "radio" => {
                let selected = consumed(el, "selected").unwrap_or_else(|| quote! { false });
                let methods = self.chained_methods(el, &["selected"]);
                quote! { ::schnellui_widgets::Radio::new(#selected) #methods }
            }
            // --- divider: a decorative separator, no args (SOUL §8.1) ---
            "divider" => {
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::Divider::new() #methods }
            }
            "text_input" | "textinput" => {
                let value = consumed(el, "value").unwrap_or_else(|| quote! { "" });
                let methods = self.chained_methods(el, &["value"]);
                quote! { ::schnellui_widgets::TextInput::new(#value) #methods }
            }
            "password_input" | "passwordinput" => {
                let value = consumed(el, "value").unwrap_or_else(|| quote! { "" });
                let methods = self.chained_methods(el, &["value"]);
                quote! { ::schnellui_widgets::PasswordInput::new(#value) #methods }
            }
            "image" => {
                let src = consumed(el, "src")
                    .or_else(|| consumed(el, "source"))
                    .or_else(|| static_child(el))
                    .unwrap_or_else(|| quote! { "" });
                let methods = self.chained_methods(el, &["src", "source"]);
                quote! { ::schnellui_widgets::Image::new(#src) #methods }
            }
            "icon" => {
                let name = consumed(el, "name")
                    .or_else(|| static_child(el))
                    .unwrap_or_else(|| quote! { "" });
                let methods = self.chained_methods(el, &["name"]);
                quote! { ::schnellui_widgets::Icon::new(#name) #methods }
            }
            // --- svg: a vector image; the single static child is its markup
            // (SOUL §8.1). `alt`/`width`/`height` chain generically. ---
            "svg" => {
                let markup = match static_label(el, "svg") {
                    Ok(l) => l,
                    Err(e) => return e,
                };
                let methods = self.chained_methods(el, &[]);
                quote! { ::schnellui_widgets::Svg::new(#markup) #methods }
            }
            other => err(el.tag.span(), &format!("unknown view! element `{other}`")),
        }
    }

    /// Lowers one `table` child to a `TableRow` builder expression (SOUL §8.1).
    /// The child must be a `table_row { … }` (alias `tr`). Plain cells are string
    /// literals; a header row may also contain `table_column`/`th` titles with an
    /// optional `sort` direction and `on:sort` action.
    fn emit_table_row(&self, table: &Element, child: &Node) -> Result<TokenStream, TokenStream> {
        let Node::Element(el) = child else {
            return Err(err(
                table.tag.span(),
                "`table { … }` children must be `table_row { … }` elements",
            ));
        };
        let tag = el.tag.to_string();
        if tag != "table_row" && tag != "tr" {
            return Err(err(
                el.tag.span(),
                "`table { … }` children must be `table_row { … }` elements",
            ));
        }
        let methods = self.chained_methods(el, &[]);
        let is_header = el
            .attrs
            .iter()
            .any(|attr| matches!(&attr.name, AttrName::Prop(prop) if prop == "header"));
        let mut cells = TokenStream::new();
        for cell in &el.children {
            match cell {
                Node::StaticText(s) => cells.extend(quote! { .cell(#s) }),
                Node::Element(column)
                    if matches!(column.tag.to_string().as_str(), "table_column" | "th") =>
                {
                    if !is_header {
                        return Err(err(
                            column.tag.span(),
                            "`table_column`/`th` is only valid inside `table_row(header)`",
                        ));
                    }
                    let label = static_label(column, "table_column")?;
                    let sort = table_column_sort_methods(column)?;
                    let column_methods = self.chained_methods(column, &["sort"]);
                    cells.extend(quote! {
                        .column(
                            ::schnellui_widgets::TableColumn::new(#label)
                                #sort #column_methods
                        )
                    });
                }
                _ => {
                    return Err(err(
                        el.tag.span(),
                        "`table_row { … }` cells must be string literals; sortable header titles use `table_column` or `th`",
                    ));
                }
            }
        }
        Ok(quote! { ::schnellui_widgets::TableRow::new() #methods #cells })
    }

    /// Emits `.child(<view>)` for every child (container tags only).
    fn child_calls(&self, el: &Element) -> TokenStream {
        let mut ts = TokenStream::new();
        for child in &el.children {
            let cv = self.emit_view(child);
            ts.extend(quote! { .child(#cv) });
        }
        ts
    }

    /// Emits chained builder methods for every attribute: `on:<event>` → an
    /// `.on_<event>(…)` handler (SOUL §6.3), any other prop → a `.<name>(…)`
    /// setter, skipping props already consumed by the constructor.
    fn chained_methods(&self, el: &Element, consumed_props: &[&str]) -> TokenStream {
        let mut ts = TokenStream::new();
        // Layout bounds must be applied after component-specific properties and
        // events. On a leaf, the first bound returns the node-transparent `Flex`
        // wrapper; applying an ordinary component setter after that would target
        // the wrapper instead of the component. Attribute order therefore remains
        // semantically irrelevant.
        let attrs = el
            .attrs
            .iter()
            .filter(|attr| !is_min_size_attr(attr))
            .chain(el.attrs.iter().filter(|attr| is_min_size_attr(attr)));
        for attr in attrs {
            match &attr.name {
                AttrName::Event(ev) => {
                    let method = format_ident!("on_{}", ev);
                    let v = value_tokens(&attr.value);
                    ts.extend(quote! { .#method(#v) });
                }
                AttrName::Prop(p) => {
                    if consumed_props.contains(&p.to_string().as_str()) {
                        continue;
                    }
                    // a valueless flag lowers to a nullary setter (`.ellipsis()`);
                    // a valued prop to `.name(value)` (SOUL §8.1).
                    if matches!(attr.value, AttrValue::Flag) {
                        ts.extend(quote! { .#p() });
                    } else {
                        let v = value_tokens(&attr.value);
                        ts.extend(quote! { .#p(#v) });
                    }
                }
            }
        }
        ts
    }
}

fn is_min_size_attr(attr: &Attr) -> bool {
    matches!(
        &attr.name,
        AttrName::Prop(prop) if prop == "min_width" || prop == "min_height"
    )
}

/// The dynamic-slot builder for a `(expr)` text site (SOUL §3.3): wrap the
/// expression in a `move ||` closure that stringifies its (Display) value on
/// every run; `Text::dynamic` drives it through a render effect so the tracked
/// signal reads re-fire it and mutate the retained node in place.
fn dynamic_text(e: &Expr) -> TokenStream {
    quote! {
        ::schnellui_widgets::Text::dynamic(move || ::std::string::ToString::to_string(&(#e)))
    }
}

/// Emits the text-styling builder calls for the enum-valued `wrap` / `align`
/// attributes on a `text` element (SOUL §8.1). Each `= <keyword>` bare ident maps
/// to the matching re-exported enum variant path
/// (`::schnellui_widgets::WrapMode::…` / `TextAlign::…`); any richer expression
/// (a qualified path like `WrapMode::Word`, a call, a variable) is passed through
/// untouched so a computed mode still works. An unknown bare keyword — or a `wrap`
/// / `align` with no value — is a spanned `compile_error!`.
///
/// (`ellipsis` is *not* handled here: it is a valueless flag lowered generically
/// by [`Codegen::chained_methods`] to a nullary `.ellipsis()`.)
fn text_style_methods(el: &Element) -> Result<TokenStream, TokenStream> {
    let mut ts = TokenStream::new();
    for attr in &el.attrs {
        let AttrName::Prop(p) = &attr.name else {
            continue;
        };
        let call = match p.to_string().as_str() {
            "wrap" => enum_attr(
                attr,
                "wrap",
                "WrapMode",
                "nowrap, word, anywhere",
                wrap_variant,
            )?,
            "align" => enum_attr(
                attr,
                "align",
                "TextAlign",
                "start, center, end, justify",
                align_variant,
            )?,
            _ => continue,
        };
        ts.extend(call);
    }
    Ok(ts)
}

/// Lowers `table_column(sort = asc|desc)` to the public sort-direction enum.
fn table_column_sort_methods(el: &Element) -> Result<TokenStream, TokenStream> {
    let mut methods = TokenStream::new();
    for attr in &el.attrs {
        let AttrName::Prop(prop) = &attr.name else {
            continue;
        };
        if prop != "sort" {
            continue;
        }
        methods.extend(enum_attr(
            attr,
            "sort",
            "SortDirection",
            "asc, ascending, desc, descending",
            table_sort_variant,
        )?);
    }
    Ok(methods)
}

/// Emits the container-styling builder calls for the enum-valued `justify` /
/// `align` attributes on a flex container element (SOUL §8.1) — the container
/// twin of [`text_style_methods`]. Each `= <keyword>` bare ident maps to the
/// matching re-exported flex enum path (`::schnellui_widgets::Justify::…` /
/// `Align::…`); any richer expression passes through untouched so a computed
/// value still works. An unknown bare keyword — or a `justify` / `align` with no
/// value — is a spanned `compile_error!`.
///
/// (`wrap` is *not* handled here: on a container it is a valueless flag lowered
/// generically by [`Codegen::chained_methods`] to the nullary `.wrap()`.)
fn container_style_methods(el: &Element) -> Result<TokenStream, TokenStream> {
    let mut ts = TokenStream::new();
    for attr in &el.attrs {
        let AttrName::Prop(p) = &attr.name else {
            continue;
        };
        let call = match p.to_string().as_str() {
            "justify" => enum_attr(
                attr,
                "justify",
                "Justify",
                "start, center, end, space_between, space_around, space_evenly",
                justify_variant,
            )?,
            "align" => enum_attr(
                attr,
                "align",
                "Align",
                "start, center, end, stretch",
                container_align_variant,
            )?,
            _ => continue,
        };
        ts.extend(call);
    }
    Ok(ts)
}

/// Lowers a dialog's standard `position = <keyword>` placements. Explicit
/// coordinates use the builder escape hatch `at(left, top)`.
fn dialog_position_methods(el: &Element) -> Result<TokenStream, TokenStream> {
    let mut ts = TokenStream::new();
    for attr in &el.attrs {
        let AttrName::Prop(p) = &attr.name else {
            continue;
        };
        if p != "position" {
            continue;
        }
        ts.extend(enum_attr(
            attr,
            "position",
            "DialogPosition",
            "top_left, top, top_right, left, center, right, bottom_left, bottom, bottom_right",
            dialog_position_variant,
        )?);
    }
    Ok(ts)
}

/// Lowers one enum-valued attribute (`method = <keyword>`) to `.method(<enum
/// path>)`, mapping a bare keyword ident via `variant_of` and passing any other
/// expression through verbatim (SOUL §8.1). `enum_ty` is the re-exported enum's
/// ident under `::schnellui_widgets`; `valid` is the human-readable keyword list
/// used in the error for an unknown keyword.
fn enum_attr(
    attr: &Attr,
    method: &str,
    enum_ty: &str,
    valid: &str,
    variant_of: fn(&str) -> Option<&'static str>,
) -> Result<TokenStream, TokenStream> {
    let method_id = Ident::new(method, Span::call_site());
    let ty_id = Ident::new(enum_ty, Span::call_site());

    if matches!(attr.value, AttrValue::Flag) {
        return Err(err(
            method_span(attr),
            &format!(
                "`{method}` needs a value, e.g. `{method} = {}`",
                first_word(valid)
            ),
        ));
    }

    // a bare keyword ident → the mapped enum variant path.
    if let Some(id) = as_bare_ident(&attr.value) {
        return match variant_of(&id.to_string()) {
            Some(variant) => {
                let var_id = Ident::new(variant, id.span());
                Ok(quote! { .#method_id(::schnellui_widgets::#ty_id::#var_id) })
            }
            None => Err(err(
                id.span(),
                &format!("unknown `{method}` keyword `{id}` — expected one of: {valid}"),
            )),
        };
    }

    // otherwise pass the expression through (escape hatch for a computed mode).
    let v = value_tokens(&attr.value);
    Ok(quote! { .#method_id(#v) })
}

/// The span of an attribute's name (for diagnostics).
fn method_span(attr: &Attr) -> Span {
    match &attr.name {
        AttrName::Prop(p) => p.span(),
        AttrName::Event(e) => e.span(),
    }
}

/// The first comma-separated token of a keyword list, for a suggestion in errors.
fn first_word(valid: &str) -> &str {
    valid.split(',').next().unwrap_or(valid).trim()
}

/// The single-segment, unqualified [`Ident`] of a bare-path attribute value
/// (`word`, `center`), if the value is exactly that — so enum-keyword attrs map
/// the keyword to a variant while any qualified path or richer expr passes through
/// (SOUL §8.1).
fn as_bare_ident(v: &AttrValue) -> Option<&Ident> {
    let AttrValue::Dynamic(Expr::Path(p)) = v else {
        return None;
    };
    if p.qself.is_none()
        && p.path.leading_colon.is_none()
        && p.path.segments.len() == 1
        && matches!(p.path.segments[0].arguments, syn::PathArguments::None)
    {
        Some(&p.path.segments[0].ident)
    } else {
        None
    }
}

/// Maps a `wrap = <keyword>` ident to a `schnellui_text::WrapMode` variant name.
/// Chosen spelling: `nowrap`/`none` → `NoWrap`, `word` → `Word`,
/// `anywhere` → `Anywhere` (SOUL §8.1).
fn wrap_variant(id: &str) -> Option<&'static str> {
    match id {
        "nowrap" | "none" => Some("NoWrap"),
        "word" => Some("Word"),
        "anywhere" => Some("Anywhere"),
        _ => None,
    }
}

/// Maps an `align = <keyword>` ident to a `schnellui_text::TextAlign` variant
/// name. Chosen spelling: lowercase keyword → PascalCase variant — `start` →
/// `Start`, `center` → `Center`, `end` → `End`, `justify` → `Justify` (SOUL §8.1).
fn align_variant(id: &str) -> Option<&'static str> {
    match id {
        "start" => Some("Start"),
        "center" => Some("Center"),
        "end" => Some("End"),
        "justify" => Some("Justify"),
        _ => None,
    }
}

/// Maps a container `justify = <keyword>` ident to a `schnellui_layout::Justify`
/// variant name. Chosen spelling: snake_case keyword → PascalCase variant —
/// `space_between` → `SpaceBetween` etc. (SOUL §8.1).
fn justify_variant(id: &str) -> Option<&'static str> {
    match id {
        "start" => Some("Start"),
        "center" => Some("Center"),
        "end" => Some("End"),
        "space_between" => Some("SpaceBetween"),
        "space_around" => Some("SpaceAround"),
        "space_evenly" => Some("SpaceEvenly"),
        _ => None,
    }
}

/// Maps a container `align = <keyword>` ident to a `schnellui_layout::Align`
/// variant name (cross-axis; `stretch` replaces text-align's `justify`).
fn container_align_variant(id: &str) -> Option<&'static str> {
    match id {
        "start" => Some("Start"),
        "center" => Some("Center"),
        "end" => Some("End"),
        "stretch" => Some("Stretch"),
        _ => None,
    }
}

fn table_sort_variant(id: &str) -> Option<&'static str> {
    match id {
        "asc" | "ascending" => Some("Ascending"),
        "desc" | "descending" => Some("Descending"),
        _ => None,
    }
}

fn dialog_position_variant(id: &str) -> Option<&'static str> {
    match id {
        "top_left" => Some("TopLeft"),
        "top" => Some("Top"),
        "top_right" => Some("TopRight"),
        "left" => Some("Left"),
        "center" => Some("Center"),
        "right" => Some("Right"),
        "bottom_left" => Some("BottomLeft"),
        "bottom" => Some("Bottom"),
        "bottom_right" => Some("BottomRight"),
        _ => None,
    }
}

/// Tokens for an attribute value — a hoisted literal or a passed-through expr. A
/// [`AttrValue::Flag`] carries no value token (it lowers to a *nullary* call in
/// [`Codegen::chained_methods`]); this yields empty tokens as a defensive fallback
/// for callers that never expect a flag in value position.
fn value_tokens(v: &AttrValue) -> TokenStream {
    match v {
        AttrValue::Static(lit) => quote! { #lit },
        AttrValue::Dynamic(e) => quote! { #e },
        AttrValue::Flag => TokenStream::new(),
    }
}

/// Value tokens of a named prop attribute, if present (used for constructor args).
fn consumed(el: &Element, name: &str) -> Option<TokenStream> {
    el.attrs.iter().find_map(|a| match &a.name {
        AttrName::Prop(p) if *p == name => Some(value_tokens(&a.value)),
        _ => None,
    })
}

/// The literal of an element's single static-text child, if that is its only
/// child (used as a fallback constructor arg for `image`/`icon`).
fn static_child(el: &Element) -> Option<TokenStream> {
    match el.children.as_slice() {
        [Node::StaticText(s)] => Some(quote! { #s }),
        _ => None,
    }
}

/// A content element's single child (`None` if childless), erroring on >1 child.
fn single_content(el: &Element) -> Result<Option<&Node>, TokenStream> {
    match el.children.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        _ => Err(err(el.tag.span(), "this element accepts at most one child")),
    }
}

/// A labeled leaf's single static string-literal child — the `button` convention
/// shared by `link` / `badge` / `tab` / `list_item` (SOUL §6.3): the label is both
/// the visible text and the accessible name. Childless yields `""`.
fn static_label(el: &Element, tag: &str) -> Result<TokenStream, TokenStream> {
    match single_content(el)? {
        None => Ok(quote! { "" }),
        Some(Node::StaticText(s)) => Ok(quote! { #s }),
        Some(_) => Err(err(
            el.tag.span(),
            &format!("`{tag} {{ … }}` label must be a string literal"),
        )),
    }
}

/// PascalCase widget type ident for a tag (`column` → `Column`).
fn type_ident(tag: &str) -> Ident {
    let mut chars = tag.chars();
    let name = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    Ident::new(&name, Span::call_site())
}

/// A spanned `compile_error!` as an expression-position [`TokenStream`].
fn err(span: Span, msg: &str) -> TokenStream {
    syn::Error::new(span, msg).to_compile_error()
}

#[cfg(test)]
mod tests;
