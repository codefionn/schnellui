use super::*;
use crate::render_html::renderer::RUST_BINDING;
use std::collections::HashSet;
use std::fmt;
use std::ops::Range;

/// The spinner animation declared once for every backend. The CSS compiled here
/// and the frame sampling in schnellui::widgets describe the same 900ms linear
/// infinite rotation, so the HTML and GPU renderers cannot drift apart.
use schnellui::motion::{Easing, Motion, Property, Repeat};
pub(crate) const SPINNER_MOTION: Motion = Motion {
    property: Property::Rotate { turns: 1.0 },
    duration_ms: 900.0,
    easing: Easing::Linear,
    repeat: Repeat::Infinite,
    delay_ms: 0.0,
};

/// A node of the emitted tree: an index into the renderer's node table. The
/// node's own markup lives in the renderer's single output buffer, so building
/// a parent never copies its children.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HtmlNode(u32);

/// One emitted element. Its own markup is `buf[start..end]` and splits at the
/// `>` that closes the opening tag, so attributes added later (`key`, `ref`)
/// are written between the two halves when the tree is serialised instead of
/// being inserted into finished text. The children follow `buf[split..end]`,
/// then `tail` closes the element.
struct NodeRec {
    start: usize,
    split: usize,
    end: usize,
    tail: &'static str,
    kids: Range<usize>,
    /// Escaped `data-sui-key` value, as a range of the buffer.
    key: Option<Range<usize>>,
    reference: Option<u64>,
    /// Whether the key attribute precedes the ref attribute (application order).
    key_first: bool,
}

/// HTML implementation of the generic base-component rendering seam.
///
/// Every node's own markup is appended to one shared buffer as the template is
/// rendered bottom-up; `Display` for [`Rendered`] then walks the node table once
/// and writes the document in order. Bytes are copied once into the buffer and
/// once into the output, whatever the nesting depth.
#[derive(Default)]
pub(crate) struct DomTemplate {
    pub(crate) handlers: Vec<RustHandler>,
    pub(crate) responsive_css: String,
    /// Class ids of the responsive rules already emitted, so a rule used twice
    /// is written once.
    responsive_seen: HashSet<u64>,
    pub(crate) queried_refs: Vec<ComponentRef>,
    buf: String,
    nodes: Vec<NodeRec>,
    /// Child ids of every node, addressed by `NodeRec::kids`.
    kids: Vec<HtmlNode>,
    /// Reused escape and rule buffers, so per-component work does not allocate.
    label_text: String,
    label_attr: String,
    rule_at: String,
    rule_cond: String,
}

/// A rendered tree that writes itself, in document order, into any formatter.
pub(crate) struct Rendered<'a> {
    dom: &'a DomTemplate,
    root: HtmlNode,
}

impl fmt::Display for Rendered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.dom.write_node(f, self.root)
    }
}

macro_rules! emit {
    ($dom:ident, $($arg:tt)*) => {
        let _ = write!($dom.buf, $($arg)*);
    };
}

impl DomTemplate {
    pub(crate) fn display(&self, root: HtmlNode) -> Rendered<'_> {
        Rendered { dom: self, root }
    }

    fn write_node(&self, f: &mut fmt::Formatter<'_>, node: HtmlNode) -> fmt::Result {
        let rec = &self.nodes[node.0 as usize];
        f.write_str(&self.buf[rec.start..rec.split])?;
        let key = |f: &mut fmt::Formatter<'_>| match &rec.key {
            Some(range) => write!(f, r#" data-sui-key="{}""#, &self.buf[range.clone()]),
            None => Ok(()),
        };
        let reference = |f: &mut fmt::Formatter<'_>| match rec.reference {
            Some(id) => write!(f, r#" data-sui-ref="{id}""#),
            None => Ok(()),
        };
        if rec.key_first {
            key(f)?;
            reference(f)?;
        } else {
            reference(f)?;
            key(f)?;
        }
        f.write_str(&self.buf[rec.split..rec.end])?;
        for &kid in &self.kids[rec.kids.clone()] {
            self.write_node(f, kid)?;
        }
        f.write_str(rec.tail)
    }

    /// Registers the element whose markup was appended to the buffer since
    /// `start`. The markup must contain the `>` that closes its opening tag, and
    /// that has to be the first `>` in it.
    fn finish(&mut self, start: usize, tail: &'static str, children: &[HtmlNode]) -> HtmlNode {
        let end = self.buf.len();
        let split = start
            + self.buf[start..]
                .find('>')
                .expect("renderer nodes always start with an HTML element");
        let kid_start = self.kids.len();
        self.kids.extend_from_slice(children);
        let id = HtmlNode(self.nodes.len() as u32);
        self.nodes.push(NodeRec {
            start,
            split,
            end,
            tail,
            kids: kid_start..self.kids.len(),
            key: None,
            reference: None,
            key_first: false,
        });
        id
    }

    /// Takes ownership of a handler and returns its binding id.
    fn bind(&mut self, handler: Option<RustHandler>) -> Option<usize> {
        handler.map(|handler| {
            let id = self.handlers.len();
            self.handlers.push(handler);
            id
        })
    }
}

/// Writes the inline event attribute that calls back into Rust, if any.
fn write_binding(out: &mut String, id: Option<usize>, event: &str, expression: &str) {
    if let Some(id) = id {
        let _ = write!(
            out,
            r#" {event}="{RUST_BINDING}(JSON.stringify({{id:{id},{expression}}}))""#
        );
    }
}

const KEYBOARD_ACTIVATION: &str = r#" tabindex="0" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();this.click()}""#;

impl TemplateRenderer for DomTemplate {
    type Node = HtmlNode;

    fn container(&mut self, props: ContainerProps, children: Vec<Self::Node>) -> Self::Node {
        let class = match props.kind {
            ContainerKind::Row => "sui-container sui-row",
            ContainerKind::Column => "sui-container sui-column",
            ContainerKind::Stack => "sui-container sui-stack",
            ContainerKind::Scroll => "sui-container sui-scroll",
        };
        let kind = match props.kind {
            ContainerKind::Row => ComponentKind::Row,
            ContainerKind::Column => ComponentKind::Column,
            ContainerKind::Stack => ComponentKind::Stack,
            ContainerKind::Scroll => ComponentKind::Scroll,
        };
        let start = self.buf.len();
        emit!(self, r#"<div class="{class}" style=""#);
        let style = &mut self.buf;
        css_px(style, "gap", props.gap);
        css_decl(style, "justify-content", justify_css(props.justify));
        css_decl(style, "align-items", align_css(props.align));
        if props.wrap {
            css_decl(style, "flex-wrap", "wrap");
        }
        if props.fill {
            css_decl(style, "width", "100%");
            css_decl(style, "height", "100%");
        }
        optional_px(style, "width", props.width);
        optional_px(style, "height", props.height);
        optional_px(style, "min-width", props.min_width);
        optional_px(style, "min-height", props.min_height);
        emit!(self, r#"" data-sui-component="{}""#, kind.as_str());
        if props.kind == ContainerKind::Scroll {
            emit!(
                self,
                r#" data-sui-scrollbar="{}" data-sui-edge-auto-scroll="{}""#,
                props.scrollbar,
                props.edge_auto_scroll
            );
        }
        self.buf.push('>');
        self.finish(start, "</div>", &children)
    }

    fn pad(&mut self, insets: EdgeInsets, child: Option<Self::Node>) -> Self::Node {
        let start = self.buf.len();
        emit!(
            self,
            r#"<div class="sui-pad" style="padding:{}px {}px {}px {}px;" data-sui-component="Pad">"#,
            insets.top,
            insets.right,
            insets.bottom,
            insets.left
        );
        let children: Vec<HtmlNode> = child.into_iter().collect();
        self.finish(start, "</div>", &children)
    }

    fn spacer(&mut self) -> Self::Node {
        let start = self.buf.len();
        self.buf
            .push_str(r#"<div class="sui-spacer" data-sui-component="Spacer" aria-hidden="true">"#);
        self.finish(start, "</div>", &[])
    }

    fn flex(&mut self, props: FlexChild, child: Option<Self::Node>) -> Self::Node {
        let start = self.buf.len();
        self.buf.push_str(r#"<div class="sui-flex" style=""#);
        let style = &mut self.buf;
        optional_number(style, "flex-grow", props.grow);
        optional_number(style, "flex-shrink", props.shrink);
        optional_px(style, "flex-basis", props.basis);
        optional_px(style, "min-width", props.min_width);
        optional_px(style, "min-height", props.min_height);
        optional_px(style, "max-width", props.max_width);
        optional_px(style, "max-height", props.max_height);
        emit!(self, r#"" data-sui-component="Flex">"#);
        let children: Vec<HtmlNode> = child.into_iter().collect();
        self.finish(start, "</div>", &children)
    }

    fn responsive(&mut self, query: ResponsiveQuery, child: Self::Node) -> Self::Node {
        let mut at_rule = std::mem::take(&mut self.rule_at);
        let mut condition = std::mem::take(&mut self.rule_cond);
        at_rule.clear();
        condition.clear();
        match query.target {
            ResponsiveTarget::Viewport => at_rule.push_str("@media"),
            ResponsiveTarget::Parent => at_rule.push_str("@container"),
            ResponsiveTarget::Component(reference) => {
                if !self.queried_refs.contains(&reference) {
                    self.queried_refs.push(reference);
                }
                let _ = write!(at_rule, "@container sui-ref-{}", reference.id());
            }
        }
        write_responsive_condition(&mut condition, query);
        // The class id is a hash of the rule, not its render order, so the same
        // rule keeps the same class across renders and a live restyle never
        // leaves an element pointing at a rule that was renumbered or dropped.
        let id = rule_hash(&at_rule, &condition);
        if self.responsive_seen.insert(id) {
            // Default hidden makes the rule a direct expression of `show_when`.
            // `display: contents` preserves the wrapped child's flex/grid position.
            let class = format_args!("sui-responsive-{id:016x}");
            let _ = writeln!(
                self.responsive_css,
                ".{class} {{ display: none; }}\n{at_rule} {condition} {{ .{class} {{ display: contents; }} }}"
            );
        }
        self.rule_at = at_rule;
        self.rule_cond = condition;
        let start = self.buf.len();
        emit!(
            self,
            r#"<div class="sui-responsive sui-responsive-{id:016x}" data-sui-responsive="{}">"#,
            match query.target {
                ResponsiveTarget::Viewport => "viewport",
                ResponsiveTarget::Parent => "parent",
                ResponsiveTarget::Component(_) => "component",
            }
        );
        self.finish(start, "</div>", &[child])
    }

    /// A node holds one `data-sui-key`: a later call replaces the earlier one,
    /// so with `.key("a").key("b")` the outermost (last applied) key, `b`, wins.
    fn keyed(&mut self, key: &str, child: Self::Node) -> Self::Node {
        let start = self.buf.len();
        escape_into(&mut self.buf, key, true);
        let range = start..self.buf.len();
        let rec = &mut self.nodes[child.0 as usize];
        if rec.key.is_none() {
            rec.key_first = rec.reference.is_none();
        }
        rec.key = Some(range);
        child
    }

    /// A node holds one `data-sui-ref`; the first applied (innermost) one
    /// wins. The old output emitted every ref and the HTML parser kept the
    /// first attribute, so this keeps the same ref in effect.
    fn component_ref(&mut self, reference: ComponentRef, child: Self::Node) -> Self::Node {
        self.nodes[child.0 as usize]
            .reference
            .get_or_insert(reference.id());
        child
    }

    fn text(&mut self, mut props: TextProps) -> Self::Node {
        let dynamic: String;
        let text: &str = match props.content {
            TextContent::Static(ref value) => value,
            TextContent::Dynamic(ref mut producer) => {
                dynamic = producer();
                &dynamic
            }
        };
        let start = self.buf.len();
        emit!(
            self,
            r#"<span class="sui-text" data-sui-component="Text" style="font-size:{}px;"#,
            props.size
        );
        let style = &mut self.buf;
        css_decl(
            style,
            "white-space",
            match props.wrap {
                WrapMode::NoWrap => "nowrap",
                WrapMode::Word | WrapMode::Anywhere => "normal",
            },
        );
        if props.wrap == WrapMode::Word {
            css_decl(style, "overflow-wrap", "break-word");
        } else if props.wrap == WrapMode::Anywhere {
            css_decl(style, "overflow-wrap", "anywhere");
        }
        css_decl(
            style,
            "text-align",
            match props.align {
                TextAlign::Start => "start",
                TextAlign::Center => "center",
                TextAlign::End => "end",
                TextAlign::Justify => "justify",
            },
        );
        if props.ellipsis {
            css_decl(style, "overflow", "hidden");
            css_decl(style, "text-overflow", "ellipsis");
        }
        emit!(self, r#""{}>{}"#, role_attribute(props.role), EscText(text));
        self.finish(start, "</span>", &[])
    }

    fn button(&mut self, mut props: ButtonProps) -> Self::Node {
        let class = match props.appearance {
            ButtonAppearance::Solid => "sui-button",
            ButtonAppearance::Ghost => "sui-button sui-button-ghost",
        };
        let id = self.bind(props.on_click.take().map(RustHandler::Click));
        let start = self.buf.len();
        emit!(
            self,
            r#"<button class="{class}" data-sui-component="Button" data-sui-role="button" data-sui-name="{}" style=""#,
            EscAttr(&props.label)
        );
        optional_px(&mut self.buf, "min-width", props.width);
        optional_px(&mut self.buf, "min-height", props.height);
        self.buf.push('"');
        if props.disabled {
            self.buf.push_str(" disabled");
        }
        write_binding(&mut self.buf, id, "onclick", "value:''");
        emit!(self, ">{}", EscText(&props.label));
        self.finish(start, "</button>", &[])
    }

    fn checkbox(&mut self, mut props: CheckboxProps) -> Self::Node {
        let id = self.bind(props.on_toggle.take().map(RustHandler::Toggle));
        let start = self.buf.len();
        emit!(
            self,
            r#"<input class="sui-checkbox" data-sui-component="Checkbox" data-sui-role="checkbox" data-sui-name="{}" type="checkbox""#,
            EscAttr(props.name.as_deref().unwrap_or(""))
        );
        if let Some(name) = props.name.as_deref() {
            emit!(self, r#" aria-label="{}""#, EscAttr(name));
        }
        if props.checked {
            self.buf.push_str(" checked");
        }
        write_binding(
            &mut self.buf,
            id,
            "onchange",
            "checked:this.checked,value:this.value",
        );
        self.buf.push('>');
        self.finish(start, "", &[])
    }

    fn slider(&mut self, mut props: SliderProps) -> Self::Node {
        let id = self.bind(props.on_change.take().map(RustHandler::Change));
        let start = self.buf.len();
        emit!(
            self,
            r#"<input class="sui-slider" data-sui-component="Slider" data-sui-role="slider" data-sui-name="{}" type="range" min="{}" max="{}" value="{}""#,
            EscAttr(props.name.as_deref().unwrap_or("")),
            props.min,
            props.max,
            props.value
        );
        if let Some(step) = props.step {
            emit!(self, r#" step="{step}""#);
        }
        if let Some(name) = &props.name {
            emit!(self, r#" aria-label="{}""#, EscAttr(name));
        }
        if props.disabled {
            self.buf.push_str(" disabled");
        }
        write_binding(&mut self.buf, id, "oninput", "value:this.value");
        self.buf.push('>');
        self.finish(start, "", &[])
    }

    fn text_input(&mut self, mut props: TextInputProps) -> Self::Node {
        let label = props.label;
        let (component, role, input_type) = if props.password {
            ("PasswordInput", Role::PasswordInput, "password")
        } else {
            ("TextInput", Role::TextInput, "text")
        };
        let role = role.dom_name();
        let id = self.bind(props.on_input.take().map(RustHandler::Input));
        let mut label_attr = std::mem::take(&mut self.label_attr);
        label_attr.clear();
        escape_into(&mut label_attr, &label, true);
        let start = self.buf.len();
        emit!(
            self,
            r#"<label class="sui-input-field" data-sui-component="{component}"><span>{}</span><input class="sui-input" data-sui-role="{role}" data-sui-name="{label_attr}" type="{input_type}" value="{}" aria-label="{label_attr}""#,
            EscText(&label),
            EscAttr(&props.value)
        );
        write_binding(&mut self.buf, id, "oninput", "value:this.value");
        self.buf.push('>');
        self.label_attr = label_attr;
        self.finish(start, "</label>", &[])
    }

    fn component(&mut self, mut props: ComponentProps, children: Vec<Self::Node>) -> Self::Node {
        component_html(self, &mut props, &children)
    }
}

fn component_html(
    r: &mut DomTemplate,
    props: &mut ComponentProps,
    children: &[HtmlNode],
) -> HtmlNode {
    let kind = props.kind;
    let marker = kind.as_str();
    // The label is escaped once per form and copied into every slot that uses it.
    let mut label = std::mem::take(&mut r.label_text);
    let mut label_attr = std::mem::take(&mut r.label_attr);
    label.clear();
    label_attr.clear();
    escape_into(&mut label, &props.label, false);
    escape_into(&mut label_attr, &props.label, true);
    let disabled = if props.disabled { " disabled" } else { "" };
    let selected = if props.selected {
        r#" aria-selected="true""#
    } else {
        ""
    };
    let checked = if props.checked { " checked" } else { "" };
    let start = r.buf.len();

    let node = match kind {
        ComponentKind::DockArea => {
            emit!(
                r,
                r#"<section class="sui-dock-area" data-sui-component="{marker}" data-sui-role="group" data-sui-name="{label_attr}" aria-label="{label_attr}">"#
            );
            r.finish(start, "</section>", children)
        }
        ComponentKind::DragHandle => {
            let id = r.bind(props.on_click.take().map(RustHandler::Click));
            emit!(
                r,
                r#"<button class="sui-drag-handle" data-sui-component="{marker}" data-sui-role="button" data-sui-name="{label_attr}" aria-label="{label_attr}" draggable="true"{disabled}"#
            );
            write_binding(&mut r.buf, id, "onclick", "value:''");
            r.buf.push_str(">⋮⋮");
            r.finish(start, "</button>", &[])
        }
        ComponentKind::Image => {
            emit!(
                r,
                r#"<img class="sui-image" data-sui-component="{marker}" data-sui-role="image" data-sui-name="{label_attr}" src="{}" alt="{label_attr}" style=""#,
                EscAttr(&props.value)
            );
            optional_px(&mut r.buf, "width", props.width);
            optional_px(&mut r.buf, "height", props.height);
            r.buf.push_str("\">");
            r.finish(start, "", &[])
        }
        ComponentKind::Icon => {
            emit!(
                r,
                r#"<span class="sui-icon" data-sui-component="{marker}" role="img" data-sui-role="image" data-sui-name="{label_attr}" aria-label="{label_attr}">"#
            );
            if props.detail.is_empty() {
                r.buf.push('◆');
            } else {
                escape_into(&mut r.buf, &props.detail, false);
            }
            r.finish(start, "</span>", &[])
        }
        ComponentKind::ProgressBar => {
            emit!(
                r,
                r#"<label class="sui-progress-field" data-sui-component="{marker}"><span>{label}</span><progress data-sui-role="progress-indicator" data-sui-name="{label_attr}" value="{}" min="{}" max="{}"></progress>"#,
                props.number,
                props.min,
                props.max
            );
            r.finish(start, "</label>", &[])
        }
        ComponentKind::LoadingSpinner => {
            emit!(
                r,
                r#"<span class="sui-spinner" data-sui-component="{marker}" data-sui-role="progress-indicator" data-sui-name="{label_attr}" role="progressbar" aria-label="{label_attr}" aria-busy="true">"#
            );
            r.finish(start, "</span>", &[])
        }
        ComponentKind::Switch => {
            let id = r.bind(props.on_toggle.take().map(RustHandler::Toggle));
            emit!(
                r,
                r#"<label class="sui-switch-field" data-sui-component="{marker}"><input class="sui-switch" data-sui-role="switch" data-sui-name="{label_attr}" type="checkbox" role="switch" aria-label="{label_attr}"{checked}{disabled}"#
            );
            write_binding(
                &mut r.buf,
                id,
                "onchange",
                "checked:this.checked,value:this.value",
            );
            emit!(r, "><span>{label}</span>");
            r.finish(start, "</label>", &[])
        }
        ComponentKind::Radio => {
            let radio_checked = if props.selected || props.checked {
                " checked"
            } else {
                ""
            };
            let id = r.bind(props.on_click.take().map(RustHandler::Click));
            emit!(
                r,
                r#"<label class="sui-radio-field" data-sui-component="{marker}"><input data-sui-role="radio" data-sui-name="{label_attr}" type="radio" aria-label="{label_attr}"{radio_checked}{disabled}"#
            );
            write_binding(&mut r.buf, id, "onchange", "value:this.value");
            emit!(r, "><span>{label}</span>");
            r.finish(start, "</label>", &[])
        }
        ComponentKind::Divider => {
            emit!(
                r,
                r#"<hr class="sui-divider" data-sui-component="{marker}">"#
            );
            r.finish(start, "", &[])
        }
        ComponentKind::Link => {
            let id = if props.disabled {
                None
            } else {
                r.bind(props.on_click.take().map(RustHandler::Click))
            };
            emit!(
                r,
                r#"<a class="sui-link" data-sui-component="{marker}" data-sui-role="link" data-sui-name="{label_attr}""#
            );
            if props.disabled {
                r.buf.push_str(r#" aria-disabled="true" tabindex="-1""#);
            } else if props.value.is_empty() {
                r.buf.push_str(r##" href="#" aria-disabled="false""##);
            } else {
                emit!(
                    r,
                    r#" href="{}" aria-disabled="false""#,
                    EscAttr(&props.value)
                );
            }
            write_binding(&mut r.buf, id, "onclick", "value:''");
            emit!(r, ">{label}");
            r.finish(start, "</a>", &[])
        }
        ComponentKind::Badge => {
            emit!(
                r,
                r#"<output class="sui-badge" data-sui-component="{marker}" data-sui-role="status" data-sui-name="{label_attr}">{label}"#
            );
            r.finish(start, "</output>", &[])
        }
        ComponentKind::Dialog => {
            emit!(
                r,
                r#"<dialog class="sui-dialog" data-sui-component="{marker}" data-sui-role="dialog" data-sui-name="{label_attr}" aria-label="{label_attr}" open><header>{label}</header>"#
            );
            r.finish(start, "</dialog>", children)
        }
        ComponentKind::GroupedTabList => {
            emit!(
                r,
                r#"<nav class="sui-grouped-tabs" data-sui-component="{marker}" data-sui-role="tab-list" data-sui-name="{label_attr}" aria-label="{label_attr}">"#
            );
            r.finish(start, "</nav>", children)
        }
        ComponentKind::TabGroup => {
            emit!(
                r,
                r#"<section class="sui-tab-group" data-sui-component="{marker}" data-sui-role="group" data-sui-name="{label_attr}" aria-label="{label_attr}"><strong>{label}</strong>"#
            );
            r.finish(start, "</section>", children)
        }
        ComponentKind::TabNode => {
            emit!(
                r,
                r#"<div class="sui-tab-node" data-sui-component="{marker}" data-sui-role="group" data-sui-name="{label_attr}">"#
            );
            r.finish(start, "</div>", children)
        }
        ComponentKind::TabBar => {
            emit!(
                r,
                r#"<div class="sui-tab-bar" data-sui-component="{marker}" role="tablist" data-sui-role="tab-list" data-sui-name="{label_attr}" aria-label="{label_attr}">"#
            );
            r.finish(start, "</div>", children)
        }
        ComponentKind::Tab => {
            let id = r.bind(props.on_click.take().map(RustHandler::Click));
            emit!(
                r,
                r#"<button class="sui-tab" data-sui-component="{marker}" role="tab" data-sui-role="tab" data-sui-name="{label_attr}"{selected}{disabled}"#
            );
            write_binding(&mut r.buf, id, "onclick", "value:''");
            emit!(r, ">{label}");
            r.finish(start, "</button>", &[])
        }
        ComponentKind::List => {
            emit!(
                r,
                r#"<ul class="sui-list" data-sui-component="{marker}" data-sui-role="list" data-sui-name="{label_attr}" aria-label="{label_attr}">"#
            );
            r.finish(start, "</ul>", children)
        }
        ComponentKind::ListItem => {
            let id = if props.disabled {
                None
            } else {
                r.bind(props.on_click.take().map(RustHandler::Click))
            };
            emit!(
                r,
                r#"<li class="sui-list-item" data-sui-component="{marker}" data-sui-role="list-item" data-sui-name="{label_attr}"{selected}"#
            );
            if props.disabled {
                r.buf.push_str(r#" aria-disabled="true""#);
            }
            if id.is_some() {
                r.buf.push_str(KEYBOARD_ACTIVATION);
            }
            write_binding(&mut r.buf, id, "onclick", "value:''");
            emit!(r, ">{label}");
            r.finish(start, "</li>", children)
        }
        ComponentKind::Dropdown => {
            let id = r.bind(props.on_input.take().map(RustHandler::Input));
            emit!(
                r,
                r#"<label class="sui-dropdown-field" data-sui-component="{marker}"><span>{label}</span><select class="sui-dropdown" data-sui-role="combo-box" data-sui-name="{label_attr}" aria-label="{label_attr}"{disabled}"#
            );
            write_binding(&mut r.buf, id, "onchange", "value:this.value");
            r.buf.push('>');
            r.finish(start, "</select></label>", children)
        }
        ComponentKind::DropdownOption => {
            emit!(
                r,
                r#"<option class="sui-dropdown-option" data-sui-component="{marker}" data-sui-role="list-box-option" data-sui-name="{label_attr}" value="{}"{}>{label}"#,
                EscAttr(if props.value.is_empty() {
                    &props.label
                } else {
                    &props.value
                }),
                if props.selected { " selected" } else { "" }
            );
            r.finish(start, "</option>", &[])
        }
        ComponentKind::Table => {
            emit!(
                r,
                r#"<table class="sui-table" data-sui-component="{marker}" data-sui-role="table" data-sui-name="{label_attr}" aria-label="{label_attr}"><tbody>"#
            );
            r.finish(start, "</tbody></table>", children)
        }
        ComponentKind::TableRow => {
            let id = if props.disabled {
                None
            } else {
                r.bind(props.on_click.take().map(RustHandler::Click))
            };
            emit!(
                r,
                r#"<tr class="sui-table-row" data-sui-component="{marker}" data-sui-role="table-row" data-sui-name="{label_attr}"{selected}"#
            );
            if props.disabled {
                r.buf.push_str(r#" aria-disabled="true""#);
            }
            if id.is_some() {
                r.buf.push_str(KEYBOARD_ACTIVATION);
            }
            write_binding(&mut r.buf, id, "onclick", "value:''");
            r.buf.push('>');
            for cell in &props.items {
                emit!(r, "<td>{}</td>", EscText(cell));
            }
            r.finish(start, "</tr>", children)
        }
        ComponentKind::RichText => {
            emit!(
                r,
                r#"<article class="sui-rich-text" data-sui-component="{marker}" data-sui-role="document" data-sui-name="{label_attr}" aria-label="{label_attr}"><h3>{label}</h3><p>{}</p>"#,
                EscText(&props.value)
            );
            r.finish(start, "</article>", children)
        }
        ComponentKind::TextArea => {
            let id = r.bind(props.on_input.take().map(RustHandler::Input));
            emit!(
                r,
                r#"<label class="sui-textarea-field" data-sui-component="{marker}"><span>{label}</span><textarea class="sui-textarea" data-sui-role="multiline-text-input" data-sui-name="{label_attr}" aria-label="{label_attr}" placeholder="{}"{disabled}"#,
                EscAttr(&props.detail)
            );
            write_binding(&mut r.buf, id, "oninput", "value:this.value");
            emit!(r, ">{}</textarea>", EscText(&props.value));
            r.finish(start, "</label>", &[])
        }
        ComponentKind::Svg => {
            emit!(
                r,
                r#"<figure class="sui-svg" data-sui-component="{marker}" data-sui-role="image" data-sui-name="{label_attr}" aria-label="{label_attr}" style=""#
            );
            optional_px(&mut r.buf, "width", props.width);
            optional_px(&mut r.buf, "height", props.height);
            r.buf.push_str("\">");
            match &props.markup {
                // Only explicitly trusted markup is inserted verbatim.
                Some(markup) => r.buf.push_str(markup.as_str()),
                // SVG source given as a plain value is shown as an image, like
                // the GPU backend draws it. Scripts do not run in image SVG, so
                // nothing in the source can reach the page.
                None if !props.value.is_empty() => {
                    r.buf.push_str(r#"<img src="data:image/svg+xml;base64,"#);
                    base64_into(&mut r.buf, props.value.as_bytes());
                    emit!(r, r#"" alt="{label_attr}">"#);
                }
                None => {}
            }
            r.finish(start, "</figure>", &[])
        }
        ComponentKind::ThemeProvider => {
            emit!(
                r,
                r#"<section class="sui-theme-provider" data-sui-component="{marker}" data-theme="{}">"#,
                EscAttr(&props.value)
            );
            r.finish(start, "</section>", children)
        }
        // Base components have dedicated methods and cannot reach this branch.
        ComponentKind::Row
        | ComponentKind::Column
        | ComponentKind::Stack
        | ComponentKind::Pad
        | ComponentKind::Spacer
        | ComponentKind::Flex
        | ComponentKind::Scroll
        | ComponentKind::Text
        | ComponentKind::Button
        | ComponentKind::Checkbox
        | ComponentKind::Slider
        | ComponentKind::TextInput => {
            unreachable!("base component has a dedicated renderer method")
        }
    };
    r.label_text = label;
    r.label_attr = label_attr;
    node
}

/// FNV-1a over the rule text: stable across runs and renders, unlike a counter.
fn rule_hash(at_rule: &str, condition: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in at_rule.bytes().chain([0]).chain(condition.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn write_responsive_condition(out: &mut String, query: ResponsiveQuery) {
    struct Css(Length);
    impl fmt::Display for Css {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self.0 {
                Length::Px(value) => write!(f, "{value}px"),
                Length::Em(value) => write!(f, "{value}em"),
            }
        }
    }

    let mut first = true;
    for (name, value) in [
        ("min-width", query.min_width),
        ("max-width", query.max_width),
        ("min-height", query.min_height),
        ("max-height", query.max_height),
    ] {
        if let Some(value) = value {
            if !first {
                out.push_str(" and ");
            }
            first = false;
            let _ = write!(out, "({name}: {})", Css(value));
        }
    }
    if first {
        out.push_str("(min-width: 0px)");
    }
}

/// The stylesheet text (the content of the document's `<style>` element).
/// `responsive_css` is spliced in after the static layout rules.
pub(crate) fn stylesheet(width: u32, height: u32, theme: Theme, responsive_css: &str) -> String {
    fn interaction_css(style: schnellui::widgets::InteractionStyle, focus: bool) -> String {
        let mut css = String::new();
        if let Some(color) = style.background {
            let _ = write!(css, "background-color:{};", color_css(color));
        }
        if let Some(color) = style.foreground {
            let _ = write!(css, "color:{};", color_css(color));
        }
        if let Some(color) = style.border {
            if focus {
                let _ = write!(
                    css,
                    "outline:3px solid {};outline-offset:-3px;",
                    color_css(color)
                );
            } else {
                let _ = write!(css, "border-color:{};", color_css(color));
            }
        }
        css
    }

    let shape = theme.shape;
    use schnellui::widgets::{InteractionComponent as Component, InteractionState as State};
    let button_hover = interaction_css(
        theme.interaction_style(Component::Button, State::Hover),
        false,
    );
    let navigation_hover = interaction_css(
        theme.interaction_style(Component::Navigation, State::Hover),
        false,
    );
    let editable_hover = interaction_css(
        theme.interaction_style(Component::Editable, State::Hover),
        false,
    );
    let toggle_hover = interaction_css(
        theme.interaction_style(Component::Toggle, State::Hover),
        false,
    );
    let raw_hover = interaction_css(
        theme.interaction_style(Component::RawSurface, State::Hover),
        false,
    );
    let button_focus = interaction_css(
        theme.interaction_style(Component::Button, State::Focus),
        true,
    );
    let navigation_focus = interaction_css(
        theme.interaction_style(Component::Navigation, State::Focus),
        true,
    );
    let editable_focus = interaction_css(
        theme.interaction_style(Component::Editable, State::Focus),
        true,
    );
    let raw_focus = interaction_css(
        theme.interaction_style(Component::RawSurface, State::Focus),
        true,
    );
    let toggle_focus = interaction_css(
        theme.interaction_style(Component::Toggle, State::Focus),
        true,
    );
    let button_active = interaction_css(
        theme.interaction_style(Component::Button, State::Active),
        false,
    );
    let navigation_active = interaction_css(
        theme.interaction_style(Component::Navigation, State::Active),
        false,
    );
    let toggle_active = interaction_css(
        theme.interaction_style(Component::Toggle, State::Active),
        false,
    );
    let editable_active = interaction_css(
        theme.interaction_style(Component::Editable, State::Active),
        false,
    );
    let raw_active = interaction_css(
        theme.interaction_style(Component::RawSurface, State::Active),
        false,
    );
    // The spinner animation is declared once (schnellui::motion) and compiled to
    // CSS here; the GPU renderer samples the exact same declaration per frame.
    let spin = schnellui::motion::css_animation(&SPINNER_MOTION, "sui-spin");
    let spin_shorthand = spin.shorthand;
    let spin_keyframes = spin.keyframes;
    format!(
        r#"
:root {{
  --sui-text: {text};
  --sui-muted: {muted};
  --sui-surface: {surface};
  --sui-outline: {outline};
  --sui-accent: {accent};
  --sui-on-accent: {on_accent};
  --sui-disabled: {disabled};
  --sui-page: {page};
  --sui-radius: {radius}px;
  --sui-density: {density};
  --sui-frame: {frame}px;
  --sui-shadow: {shadow}px;
  --sui-focus: {focus};
}}
* {{ box-sizing: border-box; }}
html, body {{ margin: 0; width: {width}px; height: {height}px; overflow: hidden; }}
body {{ background: var(--sui-page); color: var(--sui-text); font: 16px/1.2 Arial, Helvetica, sans-serif; }}
button, input, select, textarea {{ font: inherit; }}
#schnellui-root {{
  width: {width}px;
  height: {height}px;
  overflow: hidden;
}}
.sui-container {{ display: flex; min-width: 0; min-height: 0; }}
.sui-row {{ flex-direction: row; }}
.sui-column {{ flex-direction: column; }}
.sui-stack {{ position: relative; }}
.sui-stack > * {{ position: absolute; inset: 0; }}
.sui-scroll {{ flex-direction: column; overflow-y: auto; overflow-x: visible; scrollbar-width: none; }}
.sui-scroll::-webkit-scrollbar {{ width: 0; height: 0; }}
.sui-scroll[data-sui-scrollbar="true"] {{ scrollbar-width: thin; scrollbar-color: var(--sui-outline) var(--sui-surface); }}
.sui-scroll[data-sui-scrollbar="true"]::-webkit-scrollbar {{ width: 10px; }}
.sui-scroll[data-sui-scrollbar="true"]::-webkit-scrollbar-track {{ background: var(--sui-surface); border-radius: 999px; }}
.sui-scroll[data-sui-scrollbar="true"]::-webkit-scrollbar-thumb {{ background: var(--sui-outline); border: 2px solid var(--sui-surface); border-radius: 999px; }}
.sui-pad {{ display: flex; flex-direction: column; }}
.sui-spacer {{ flex: 1 0 0; }}
.sui-flex {{ display: flex; }}
.sui-flex > * {{ flex: 1 1 auto; }}
.sui-container:has(> .sui-responsive[data-sui-responsive="parent"]),
.sui-pad:has(> .sui-responsive[data-sui-responsive="parent"]),
.sui-flex:has(> .sui-responsive[data-sui-responsive="parent"]) {{
  container-type: size;
}}
{responsive_css}
.sui-text {{ display: inline-block; color: var(--sui-text); min-width: 0; }}
.sui-button {{
  appearance: none;
  border: var(--sui-frame) solid var(--sui-outline);
  border-radius: var(--sui-radius);
  padding: calc(4px * var(--sui-density)) calc(8px * var(--sui-density));
  color: var(--sui-on-accent);
  background: var(--sui-accent);
  box-shadow: var(--sui-shadow) var(--sui-shadow) 0 var(--sui-text);
  cursor: pointer;
}}
.sui-button-ghost {{ color: var(--sui-text); background: transparent; border-color: transparent; box-shadow: none; }}
.sui-button:disabled {{ background: var(--sui-disabled); cursor: default; }}
.sui-checkbox {{ width: calc(18px * var(--sui-density)); height: calc(18px * var(--sui-density)); margin: 0; accent-color: var(--sui-accent); }}
.sui-slider {{ width: 160px; margin: 0; accent-color: var(--sui-accent); }}
.sui-input-field {{ display: inline-flex; flex-direction: column; gap: 2px; color: var(--sui-muted); font-size: 12px; }}
.sui-input {{ min-width: 180px; padding: calc(4px * var(--sui-density)) calc(8px * var(--sui-density)); color: var(--sui-text); background: var(--sui-surface); border: max(1px, var(--sui-frame)) solid var(--sui-outline); border-radius: var(--sui-radius); }}
.sui-dock-area, .sui-tab-group, .sui-rich-text {{
  padding: 10px; border: 1px dashed var(--sui-outline); border-radius: var(--sui-radius);
}}
.sui-drag-handle {{ cursor: grab; border: 0; color: var(--sui-muted); background: transparent; border-radius: var(--sui-radius); letter-spacing: -3px; }}
.sui-image {{ display: block; object-fit: cover; border-radius: var(--sui-radius); background: var(--sui-surface); }}
.sui-icon {{ display: inline-grid; place-items: center; width: 28px; height: 28px; color: var(--sui-accent); }}
.sui-progress-field, .sui-textarea-field, .sui-dropdown-field {{
  display: inline-flex; flex-direction: column; gap: 4px; color: var(--sui-muted); font-size: 12px;
}}
progress {{ width: 180px; accent-color: var(--sui-accent); }}
.sui-spinner {{ display: inline-block; width: 24px; height: 24px; border: 3px solid var(--sui-outline); border-top-color: var(--sui-accent); border-radius: 50%; animation: {spin_shorthand}; }}
{spin_keyframes}
.sui-switch-field, .sui-radio-field {{ display: inline-flex; align-items: center; gap: 7px; }}
.sui-switch {{ accent-color: var(--sui-accent); }}
.sui-divider {{ width: 100%; margin: 2px 0; border: 0; border-top: 1px solid var(--sui-outline); }}
.sui-link {{ color: var(--sui-accent); border-radius: 2px; text-underline-offset: 2px; }}
.sui-link[aria-disabled="true"] {{ color: var(--sui-muted); cursor: not-allowed; opacity: 0.72; }}
.sui-badge {{ display: inline-block; width: fit-content; padding: 3px 8px; border-radius: 999px; color: var(--sui-on-accent); background: var(--sui-accent); font-size: 12px; }}
.sui-dialog {{ position: static; margin: 0; width: min(420px, 100%); padding: 14px; color: var(--sui-text); background: var(--sui-surface); border: 1px solid var(--sui-outline); border-radius: var(--sui-radius); }}
.sui-dialog::backdrop {{ display: none; }}
.sui-dialog header {{ margin-bottom: 8px; font-weight: 700; }}
.sui-grouped-tabs, .sui-tab-bar {{ display: flex; gap: 6px; align-items: flex-start; }}
.sui-tab-group {{ display: flex; flex-direction: column; gap: 5px; }}
.sui-tab-node {{ padding-left: 8px; border-left: 2px solid var(--sui-outline); }}
.sui-tab {{ padding: 5px 9px; color: var(--sui-text); background: var(--sui-surface); border: 1px solid var(--sui-outline); border-radius: var(--sui-radius); cursor: pointer; }}
.sui-tab[aria-selected="true"] {{ color: var(--sui-on-accent); background: var(--sui-accent); }}
.sui-list {{ margin: 0; padding: 0; list-style: none; border: 1px solid var(--sui-outline); border-radius: var(--sui-radius); overflow: hidden; }}
.sui-list-item {{ padding: 5px 9px; }}
.sui-list-item[tabindex="0"], .sui-table-row[tabindex="0"] {{ cursor: pointer; }}
.sui-list-item[aria-disabled="true"], .sui-table-row[aria-disabled="true"] {{ cursor: not-allowed; opacity: 0.68; }}
.sui-list-item + .sui-list-item {{ border-top: 1px solid var(--sui-outline); }}
.sui-list-item[aria-selected="true"] {{ background: color-mix(in srgb, var(--sui-accent) 18%, transparent); }}
.sui-dropdown, .sui-textarea {{ min-width: 180px; padding: 6px 8px; color: var(--sui-text); background: var(--sui-surface); border: 1px solid var(--sui-outline); border-radius: var(--sui-radius); }}
.sui-textarea {{ min-height: 74px; resize: none; }}
.sui-table {{ border-collapse: collapse; font-size: 13px; }}
.sui-table td {{ padding: 6px 10px; border: 1px solid var(--sui-outline); }}
.sui-table-row[aria-selected="true"] {{ background: color-mix(in srgb, var(--sui-accent) 18%, transparent); }}
.sui-rich-text h3, .sui-rich-text p {{ margin: 0 0 6px; }}
.sui-svg {{ display: grid; place-items: center; margin: 0; }}
.sui-svg svg {{ width: 100%; height: 100%; }}
.sui-svg img {{ width: 100%; height: 100%; object-fit: contain; }}
.sui-theme-provider {{ padding: 10px; color: #f8fafc; background: #172033; border-radius: var(--sui-radius); }}
.sui-theme-provider .sui-text {{ color: inherit; }}

:where(button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])) {{
  transition:
    color 120ms ease,
    background-color 120ms ease,
    border-color 120ms ease,
    outline-color 120ms ease,
    transform 90ms ease;
}}
:where(button, input, select, textarea):disabled {{
  cursor: not-allowed;
  opacity: 0.68;
}}

@media (hover: hover) and (pointer: fine) {{
  .sui-button:not(:disabled):hover {{
    {button_hover}
    transform: translate(-1px, -1px);
  }}
  .sui-button-ghost:not(:disabled):hover,
  .sui-tab:not(:disabled):hover,
  .sui-drag-handle:not(:disabled):hover,
  .sui-list-item[tabindex="0"]:hover,
  .sui-table-row[tabindex="0"]:hover {{
    {navigation_hover}
  }}
  .sui-link:not([aria-disabled="true"]):hover {{
    text-decoration-thickness: 2px;
    text-underline-offset: 4px;
  }}
  :where(.sui-input, .sui-dropdown, .sui-textarea):not(:disabled):hover {{
    {editable_hover}
  }}
  :where(.sui-checkbox, .sui-slider, .sui-switch, .sui-radio-field input):not(:disabled):hover {{
    {toggle_hover}
  }}
  :where(.sui-scroll, .sui-image, .sui-rich-text):hover {{ {raw_hover} }}
}}

:where(
  .sui-tab,
  .sui-drag-handle,
  .sui-list-item[tabindex="0"],
  .sui-table-row[tabindex="0"]
):not(:disabled):active {{
  {navigation_active}
  transform: translate(1px, 1px);
}}
.sui-button:not(:disabled):active {{ {button_active} transform: translate(1px, 1px); }}
:where(.sui-checkbox, .sui-slider, .sui-switch, .sui-radio-field input):not(:disabled):active {{ {toggle_active} }}
:where(.sui-input, .sui-dropdown, .sui-textarea):not(:disabled):active {{ {editable_active} }}
:where(.sui-scroll, .sui-image, .sui-rich-text):active {{ {raw_active} }}

:where(button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])):focus-visible {{
  outline: none;
}}
:where(.sui-button):focus-visible {{ {button_focus} }}
:where(.sui-tab, .sui-list-item, .sui-table-row, .sui-link):focus-visible {{ {navigation_focus} }}
:where(.sui-input, .sui-dropdown, .sui-textarea):focus-visible {{
  {editable_focus}
}}
:where(.sui-checkbox, .sui-slider, .sui-switch, .sui-radio-field input):focus-visible {{ {toggle_focus} }}
:where(.sui-scroll, .sui-image, .sui-rich-text):focus-visible {{ {raw_focus} }}
:where(.sui-input-field, .sui-dropdown-field, .sui-textarea-field):has(:focus-visible),
:where(.sui-switch-field, .sui-radio-field):has(input:focus-visible) {{
  color: var(--sui-accent);
}}

@media (prefers-reduced-motion: reduce) {{
  :where(button, a[href], input, select, textarea, [tabindex]) {{
    transition: none;
  }}
  .sui-spinner {{ animation: none; }}
}}

@media (forced-colors: active) {{
  :where(button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])):focus-visible {{
    outline-color: Highlight;
  }}
}}
"#,
        text = color_css(theme.text),
        muted = color_css(theme.text_muted),
        surface = color_css(theme.surface),
        outline = color_css(theme.outline),
        accent = color_css(theme.accent),
        focus = color_css(theme.focus_color()),
        on_accent = color_css(theme.on_accent),
        disabled = color_css(theme.disabled),
        page = color_css(theme.page),
        radius = 4.0 * shape.roundness,
        density = shape.density,
        frame = shape.frame,
        shadow = shape.shadow,
        button_hover = button_hover,
        navigation_hover = navigation_hover,
        editable_hover = editable_hover,
        toggle_hover = toggle_hover,
        raw_hover = raw_hover,
        button_focus = button_focus,
        navigation_focus = navigation_focus,
        editable_focus = editable_focus,
        raw_focus = raw_focus,
        toggle_focus = toggle_focus,
        button_active = button_active,
        navigation_active = navigation_active,
        toggle_active = toggle_active,
        editable_active = editable_active,
        raw_active = raw_active,
        spin_shorthand = spin_shorthand,
        spin_keyframes = spin_keyframes,
    )
}

/// Wraps a stylesheet and rendered body in a complete document.
pub(crate) fn document(style: &str, body: &str) -> String {
    let mut out = String::with_capacity(
        style.len() + body.len() + DOCUMENT_HEAD.len() + DOCUMENT_TAIL.len() + 64,
    );
    out.push_str(DOCUMENT_HEAD);
    out.push_str(style);
    out.push_str("</style>\n</head>\n<body><main id=\"schnellui-root\">");
    out.push_str(body);
    out.push_str("</main>\n");
    out.push_str(DOCUMENT_TAIL);
    out
}

const DOCUMENT_HEAD: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style id="schnellui-native-styles">"#;
const DOCUMENT_TAIL: &str = r#"<script>
(() => {
  let active = null;
  let pointerY = 0;
  let frame = 0;
  const tick = () => {
    if (!active) return;
    const rect = active.getBoundingClientRect();
    const zone = Math.min(24, rect.height / 2);
    const delta = pointerY <= rect.top + zone ? -12 :
      pointerY >= rect.bottom - zone ? 12 : 0;
    if (delta) active.scrollTop += delta;
    frame = requestAnimationFrame(tick);
  };
  document.addEventListener('pointerdown', event => {
    if (event.button !== 0) return;
    active = event.target.closest('.sui-scroll[data-sui-edge-auto-scroll="true"]');
    if (!active) return;
    pointerY = event.clientY;
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(tick);
  });
  document.addEventListener('pointermove', event => { pointerY = event.clientY; });
  const stop = () => { active = null; cancelAnimationFrame(frame); };
  document.addEventListener('pointerup', stop);
  document.addEventListener('pointercancel', stop);
})();
</script>
</body>
</html>"#;
pub(crate) fn normalize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn css_decl(output: &mut String, property: &str, value: &str) {
    let _ = write!(output, "{property}:{value};");
}

fn css_px(output: &mut String, property: &str, value: f32) {
    let _ = write!(output, "{property}:{value}px;");
}

fn optional_px(output: &mut String, property: &str, value: Option<f32>) {
    if let Some(value) = value {
        css_px(output, property, value);
    }
}

fn optional_number(output: &mut String, property: &str, value: Option<f32>) {
    if let Some(value) = value {
        let _ = write!(output, "{property}:{value};");
    }
}

fn justify_css(value: Justify) -> &'static str {
    match value {
        Justify::Start => "flex-start",
        Justify::Center => "center",
        Justify::End => "flex-end",
        Justify::SpaceBetween => "space-between",
        Justify::SpaceAround => "space-around",
        Justify::SpaceEvenly => "space-evenly",
    }
}

fn align_css(value: Align) -> &'static str {
    match value {
        Align::Start => "flex-start",
        Align::Center => "center",
        Align::End => "flex-end",
        Align::Stretch => "stretch",
    }
}

fn role_attribute(role: Role) -> &'static str {
    match role {
        Role::Status => r#" role="status""#,
        _ => "",
    }
}

pub(crate) fn color_css(color: schnellui::scene::Color) -> String {
    if color.a == 255 {
        format!("#{:02x}{:02x}{:02x}", color.r, color.g, color.b)
    } else {
        format!(
            "rgba({}, {}, {}, {:.4})",
            color.r,
            color.g,
            color.b,
            color.a as f32 / 255.0
        )
    }
}

/// Feeds `value` to `emit` in chunks with `& < >` (and, for attribute values,
/// `"` and `'`) replaced by entities. One pass, no intermediate string. All
/// escaped bytes are ASCII, so slicing around them keeps the UTF-8 intact.
fn escape_chunks<E>(
    value: &str,
    attr: bool,
    emit: &mut impl FnMut(&str) -> Result<(), E>,
) -> Result<(), E> {
    let mut last = 0;
    for (at, byte) in value.bytes().enumerate() {
        let entity = match byte {
            b'&' => "&amp;",
            b'<' => "&lt;",
            b'>' => "&gt;",
            b'"' if attr => "&quot;",
            b'\'' if attr => "&#39;",
            _ => continue,
        };
        emit(&value[last..at])?;
        emit(entity)?;
        last = at + 1;
    }
    emit(&value[last..])
}

/// Appends `value` escaped for text content (`attr` false) or for a quoted
/// attribute value (`attr` true).
fn escape_into(out: &mut String, value: &str, attr: bool) {
    let _ = escape_chunks::<()>(value, attr, &mut |chunk| {
        out.push_str(chunk);
        Ok(())
    });
}

/// Text-content escaping as a `Display` value, written straight into the buffer.
struct EscText<'a>(&'a str);

impl fmt::Display for EscText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        escape_chunks(self.0, false, &mut |chunk| f.write_str(chunk))
    }
}

/// Quoted-attribute escaping as a `Display` value.
struct EscAttr<'a>(&'a str);

impl fmt::Display for EscAttr<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        escape_chunks(self.0, true, &mut |chunk| f.write_str(chunk))
    }
}

/// Appends the standard, padded base64 encoding of `data`.
fn base64_into(out: &mut String, data: &[u8]) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    out.reserve(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(chunk.get(1).copied().unwrap_or(0)) << 8)
            | u32::from(chunk.get(2).copied().unwrap_or(0));
        let sextet = |shift: u32| ALPHABET[(n >> shift & 63) as usize] as char;
        out.push(sextet(18));
        out.push(sextet(12));
        out.push(if chunk.len() > 1 { sextet(6) } else { '=' });
        out.push(if chunk.len() > 2 { sextet(0) } else { '=' });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inverse of the escaper, for round-trip checks.
    fn unescape(value: &str) -> String {
        value
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }

    const HOSTILE: &[&str] = &[
        "",
        "plain",
        r#""><script>alert(1)</script>"#,
        "' onmouseover='x",
        "a & b &amp; c &lt; d",
        "<<>>&&\"\"''",
        "é ✓ 日本語 🦀 <b>é</b>",
        "&#39;&quot;",
    ];

    #[test]
    fn escapers_round_trip_hostile_strings() {
        for &input in HOSTILE {
            let mut text = String::new();
            escape_into(&mut text, input, false);
            assert!(!text.contains(['<', '>']), "{text}");
            assert_eq!(unescape(&text), input);
            assert_eq!(EscText(input).to_string(), text);

            let mut attr = String::new();
            escape_into(&mut attr, input, true);
            assert!(!attr.contains(['<', '>', '"', '\'']), "{attr}");
            assert_eq!(unescape(&attr), input);
            assert_eq!(EscAttr(input).to_string(), attr);
        }
        // Text keeps quotes, which are only special inside attribute values.
        let mut text = String::new();
        escape_into(&mut text, r#"say "hi" it's"#, false);
        assert_eq!(text, r#"say "hi" it's"#);
    }

    #[test]
    fn base64_matches_the_standard_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            let mut out = String::new();
            base64_into(&mut out, input.as_bytes());
            assert_eq!(out, expected);
        }
        let mut out = String::new();
        base64_into(&mut out, &[0xfb, 0xff, 0xfe]);
        assert_eq!(out, "+//+");
    }
}
