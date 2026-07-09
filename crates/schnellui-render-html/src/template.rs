use super::*;
use crate::renderer::RUST_BINDING;

/// The spinner animation declared once for every backend. The CSS compiled here
/// and the frame sampling in schnellui-widgets describe the same 900ms linear
/// infinite rotation, so the HTML and GPU renderers cannot drift apart.
use schnellui_motion::{Easing, Motion, Property, Repeat};
pub(crate) const SPINNER_MOTION: Motion = Motion {
    property: Property::Rotate { turns: 1.0 },
    duration_ms: 900.0,
    easing: Easing::Linear,
    repeat: Repeat::Infinite,
    delay_ms: 0.0,
};

#[derive(Clone, Debug)]
pub(crate) struct HtmlNode(String);

/// HTML implementation of the generic base-component rendering seam.
#[derive(Default)]
pub(crate) struct DomTemplate {
    pub(crate) handlers: Vec<RustHandler>,
    pub(crate) responsive_css: String,
    responsive_count: usize,
    pub(crate) queried_refs: Vec<ComponentRef>,
}

impl DomTemplate {
    fn handler(&mut self, handler: RustHandler, event: &str, expression: &str) -> String {
        let id = self.handlers.len();
        self.handlers.push(handler);
        format!(r#" {event}="{RUST_BINDING}(JSON.stringify({{id:{id},{expression}}}))""#)
    }
}

impl TemplateRenderer for DomTemplate {
    type Node = HtmlNode;

    fn container(&mut self, props: ContainerProps, children: Vec<Self::Node>) -> Self::Node {
        let class = match props.kind {
            ContainerKind::Row => "sui-container sui-row",
            ContainerKind::Column => "sui-container sui-column",
            ContainerKind::Stack => "sui-container sui-stack",
            ContainerKind::Scroll => "sui-container sui-scroll",
        };
        let mut style = String::new();
        css_decl(&mut style, "gap", &px(props.gap));
        css_decl(&mut style, "justify-content", justify_css(props.justify));
        css_decl(&mut style, "align-items", align_css(props.align));
        if props.wrap {
            css_decl(&mut style, "flex-wrap", "wrap");
        }
        if props.fill {
            css_decl(&mut style, "width", "100%");
            css_decl(&mut style, "height", "100%");
        }
        optional_px(&mut style, "width", props.width);
        optional_px(&mut style, "height", props.height);
        optional_px(&mut style, "min-width", props.min_width);
        optional_px(&mut style, "min-height", props.min_height);
        let kind = match props.kind {
            ContainerKind::Row => ComponentKind::Row,
            ContainerKind::Column => ComponentKind::Column,
            ContainerKind::Stack => ComponentKind::Stack,
            ContainerKind::Scroll => ComponentKind::Scroll,
        };
        let mut attributes = component_attribute(kind);
        if props.kind == ContainerKind::Scroll {
            let _ = write!(
                attributes,
                r#" data-sui-scrollbar="{}" data-sui-edge-auto-scroll="{}""#,
                props.scrollbar, props.edge_auto_scroll
            );
        }
        element("div", class, &style, &attributes, children)
    }

    fn pad(&mut self, insets: EdgeInsets, child: Option<Self::Node>) -> Self::Node {
        let style = format!(
            "padding:{} {} {} {};",
            px(insets.top),
            px(insets.right),
            px(insets.bottom),
            px(insets.left)
        );
        element(
            "div",
            "sui-pad",
            &style,
            &component_attribute(ComponentKind::Pad),
            child.into_iter().collect(),
        )
    }

    fn spacer(&mut self) -> Self::Node {
        HtmlNode(
            r#"<div class="sui-spacer" data-sui-component="Spacer" aria-hidden="true"></div>"#
                .into(),
        )
    }

    fn flex(&mut self, props: FlexChild, child: Option<Self::Node>) -> Self::Node {
        let mut style = String::new();
        optional_number(&mut style, "flex-grow", props.grow);
        optional_number(&mut style, "flex-shrink", props.shrink);
        optional_px(&mut style, "flex-basis", props.basis);
        optional_px(&mut style, "min-width", props.min_width);
        optional_px(&mut style, "min-height", props.min_height);
        optional_px(&mut style, "max-width", props.max_width);
        optional_px(&mut style, "max-height", props.max_height);
        element(
            "div",
            "sui-flex",
            &style,
            &component_attribute(ComponentKind::Flex),
            child.into_iter().collect(),
        )
    }

    fn responsive(&mut self, query: ResponsiveQuery, child: Self::Node) -> Self::Node {
        let id = self.responsive_count;
        self.responsive_count += 1;
        let class = format!("sui-responsive-{id}");
        let condition = responsive_condition(query);
        let at_rule = match query.target {
            ResponsiveTarget::Viewport => "@media".to_string(),
            ResponsiveTarget::Parent => "@container".to_string(),
            ResponsiveTarget::Component(reference) => {
                if !self.queried_refs.contains(&reference) {
                    self.queried_refs.push(reference);
                }
                format!("@container sui-ref-{}", reference.id())
            }
        };
        // Default hidden makes the rule a direct expression of `show_when`.
        // `display: contents` preserves the wrapped child's flex/grid position.
        let _ = writeln!(
            self.responsive_css,
            ".{class} {{ display: none; }}\n{at_rule} {condition} {{ .{class} {{ display: contents; }} }}"
        );
        HtmlNode(format!(
            r#"<div class="sui-responsive {class}" data-sui-responsive="{}">{}</div>"#,
            match query.target {
                ResponsiveTarget::Viewport => "viewport",
                ResponsiveTarget::Parent => "parent",
                ResponsiveTarget::Component(_) => "component",
            },
            child.0
        ))
    }

    fn component_ref(&mut self, reference: ComponentRef, mut child: Self::Node) -> Self::Node {
        let id = reference.id();
        insert_root_attribute(&mut child.0, &format!(r#" data-sui-ref="{id}""#));
        child
    }

    fn text(&mut self, mut props: TextProps) -> Self::Node {
        let text = match &mut props.content {
            TextContent::Static(value) => value.to_string(),
            TextContent::Dynamic(producer) => producer(),
        };
        let mut style = format!("font-size:{};", px(props.size));
        css_decl(
            &mut style,
            "white-space",
            match props.wrap {
                WrapMode::NoWrap => "nowrap",
                WrapMode::Word | WrapMode::Anywhere => "normal",
            },
        );
        if props.wrap == WrapMode::Word {
            css_decl(&mut style, "overflow-wrap", "break-word");
        } else if props.wrap == WrapMode::Anywhere {
            css_decl(&mut style, "overflow-wrap", "anywhere");
        }
        css_decl(
            &mut style,
            "text-align",
            match props.align {
                TextAlign::Start => "start",
                TextAlign::Center => "center",
                TextAlign::End => "end",
                TextAlign::Justify => "justify",
            },
        );
        if props.ellipsis {
            css_decl(&mut style, "overflow", "hidden");
            css_decl(&mut style, "text-overflow", "ellipsis");
        }
        let role = role_attribute(props.role);
        HtmlNode(format!(
            r#"<span class="sui-text" data-sui-component="Text" style="{style}"{role}>{}</span>"#,
            escape_text(&text)
        ))
    }

    fn button(&mut self, mut props: ButtonProps) -> Self::Node {
        let mut style = String::new();
        optional_px(&mut style, "min-width", props.width);
        optional_px(&mut style, "min-height", props.height);
        let class = match props.appearance {
            ButtonAppearance::Solid => "sui-button",
            ButtonAppearance::Ghost => "sui-button sui-button-ghost",
        };
        let events = props
            .on_click
            .take()
            .map(|handler| self.handler(RustHandler::Click(handler), "onclick", "value:''"))
            .unwrap_or_default();
        HtmlNode(format!(
            r#"<button class="{class}" data-sui-component="Button" data-sui-role="button" data-sui-name="{}" style="{style}"{}{events}>{}</button>"#,
            escape_attr(&props.label),
            if props.disabled { " disabled" } else { "" },
            escape_text(&props.label)
        ))
    }

    fn checkbox(&mut self, mut props: CheckboxProps) -> Self::Node {
        let name = props
            .name
            .as_deref()
            .map(|name| format!(r#" aria-label="{}""#, escape_attr(name)))
            .unwrap_or_default();
        let semantic_name = props.name.as_deref().unwrap_or("");
        let events = props
            .on_toggle
            .take()
            .map(|handler| {
                self.handler(
                    RustHandler::Toggle(handler),
                    "onchange",
                    "checked:this.checked,value:this.value",
                )
            })
            .unwrap_or_default();
        HtmlNode(format!(
            r#"<input class="sui-checkbox" data-sui-component="Checkbox" data-sui-role="checkbox" data-sui-name="{}" type="checkbox"{name}{}{events}>"#,
            escape_attr(semantic_name),
            if props.checked { " checked" } else { "" }
        ))
    }

    fn slider(&mut self, mut props: SliderProps) -> Self::Node {
        let mut attrs = format!(
            r#" min="{}" max="{}" value="{}""#,
            props.min, props.max, props.value
        );
        if let Some(step) = props.step {
            let _ = write!(attrs, r#" step="{step}""#);
        }
        let semantic_name = props.name.as_deref().unwrap_or("");
        if let Some(name) = &props.name {
            let _ = write!(attrs, r#" aria-label="{}""#, escape_attr(name));
        }
        if props.disabled {
            attrs.push_str(" disabled");
        }
        let events = props
            .on_change
            .take()
            .map(|handler| {
                self.handler(RustHandler::Change(handler), "oninput", "value:this.value")
            })
            .unwrap_or_default();
        HtmlNode(format!(
            r#"<input class="sui-slider" data-sui-component="Slider" data-sui-role="slider" data-sui-name="{}" type="range"{attrs}{events}>"#,
            escape_attr(semantic_name)
        ))
    }

    fn text_input(&mut self, mut props: TextInputProps) -> Self::Node {
        let label = props.label;
        let (component, role, input_type) = if props.password {
            ("PasswordInput", "password-input", "password")
        } else {
            ("TextInput", "text-input", "text")
        };
        let events = props
            .on_input
            .take()
            .map(|handler| self.handler(RustHandler::Input(handler), "oninput", "value:this.value"))
            .unwrap_or_default();
        HtmlNode(format!(
            r#"<label class="sui-input-field" data-sui-component="{component}"><span>{}</span><input class="sui-input" data-sui-role="{role}" data-sui-name="{}" type="{input_type}" value="{}" aria-label="{}"{events}></label>"#,
            escape_text(&label),
            escape_attr(&label),
            escape_attr(&props.value),
            escape_attr(&label)
        ))
    }

    fn component(&mut self, mut props: ComponentProps, children: Vec<Self::Node>) -> Self::Node {
        component_html(self, &mut props, children)
    }
}

fn component_html(
    renderer: &mut DomTemplate,
    props: &mut ComponentProps,
    children: Vec<HtmlNode>,
) -> HtmlNode {
    let kind = props.kind;
    let marker = component_attribute(kind);
    let label = escape_text(&props.label);
    let label_attr = escape_attr(&props.label);
    let disabled = if props.disabled { " disabled" } else { "" };
    let selected = if props.selected {
        r#" aria-selected="true""#
    } else {
        ""
    };
    let checked = if props.checked { " checked" } else { "" };
    let child_html = children_html(children);
    let mut style = String::new();
    optional_px(&mut style, "width", props.width);
    optional_px(&mut style, "height", props.height);

    match kind {
        ComponentKind::DockArea => HtmlNode(format!(
            r#"<section class="sui-dock-area" {marker} data-sui-role="group" data-sui-name="{label_attr}" aria-label="{label_attr}">{child_html}</section>"#
        )),
        ComponentKind::DragHandle => {
            let events = props
                .on_click
                .take()
                .map(|handler| renderer.handler(RustHandler::Click(handler), "onclick", "value:''"))
                .unwrap_or_default();
            HtmlNode(format!(
                r#"<button class="sui-drag-handle" {marker} data-sui-role="button" data-sui-name="{label_attr}" aria-label="{label_attr}" draggable="true"{disabled}{events}>⋮⋮</button>"#
            ))
        }
        ComponentKind::Image => HtmlNode(format!(
            r#"<img class="sui-image" {marker} data-sui-role="image" data-sui-name="{label_attr}" src="{}" alt="{label_attr}" style="{style}">"#,
            escape_attr(&props.value)
        )),
        ComponentKind::Icon => HtmlNode(format!(
            r#"<span class="sui-icon" {marker} role="img" data-sui-role="image" data-sui-name="{label_attr}" aria-label="{label_attr}">{}</span>"#,
            if props.detail.is_empty() {
                "◆".to_string()
            } else {
                escape_text(&props.detail)
            }
        )),
        ComponentKind::ProgressBar => HtmlNode(format!(
            r#"<label class="sui-progress-field" {marker}><span>{label}</span><progress data-sui-role="progress-indicator" data-sui-name="{label_attr}" value="{}" min="{}" max="{}"></progress></label>"#,
            props.number, props.min, props.max
        )),
        ComponentKind::LoadingSpinner => HtmlNode(format!(
            r#"<span class="sui-spinner" {marker} data-sui-role="progress-indicator" data-sui-name="{label_attr}" role="progressbar" aria-label="{label_attr}" aria-busy="true"></span>"#
        )),
        ComponentKind::Switch => {
            let events = props
                .on_toggle
                .take()
                .map(|handler| {
                    renderer.handler(
                        RustHandler::Toggle(handler),
                        "onchange",
                        "checked:this.checked,value:this.value",
                    )
                })
                .unwrap_or_default();
            HtmlNode(format!(
                r#"<label class="sui-switch-field" {marker}><input class="sui-switch" data-sui-role="switch" data-sui-name="{label_attr}" type="checkbox" role="switch" aria-label="{label_attr}"{checked}{disabled}{events}><span>{label}</span></label>"#
            ))
        }
        ComponentKind::Radio => {
            let radio_checked = if props.selected || props.checked {
                " checked"
            } else {
                ""
            };
            let events = props
                .on_click
                .take()
                .map(|handler| {
                    renderer.handler(RustHandler::Click(handler), "onchange", "value:this.value")
                })
                .unwrap_or_default();
            HtmlNode(format!(
                r#"<label class="sui-radio-field" {marker}><input data-sui-role="radio" data-sui-name="{label_attr}" type="radio" aria-label="{label_attr}"{radio_checked}{disabled}{events}><span>{label}</span></label>"#
            ))
        }
        ComponentKind::Divider => HtmlNode(format!(r#"<hr class="sui-divider" {marker}>"#)),
        ComponentKind::Link => {
            let events = if props.disabled {
                String::new()
            } else {
                props
                    .on_click
                    .take()
                    .map(|handler| {
                        renderer.handler(RustHandler::Click(handler), "onclick", "value:''")
                    })
                    .unwrap_or_default()
            };
            let link_state = if props.disabled {
                r#" aria-disabled="true" tabindex="-1""#.to_string()
            } else {
                let href = if props.value.is_empty() {
                    "#".to_string()
                } else {
                    escape_attr(&props.value)
                };
                format!(r#" href="{href}" aria-disabled="false""#)
            };
            HtmlNode(format!(
                r#"<a class="sui-link" {marker} data-sui-role="link" data-sui-name="{label_attr}"{link_state}{events}>{label}</a>"#
            ))
        }
        ComponentKind::Badge => HtmlNode(format!(
            r#"<output class="sui-badge" {marker} data-sui-role="status" data-sui-name="{label_attr}">{label}</output>"#
        )),
        ComponentKind::Dialog => HtmlNode(format!(
            r#"<dialog class="sui-dialog" {marker} data-sui-role="dialog" data-sui-name="{label_attr}" aria-label="{label_attr}" open><header>{label}</header>{child_html}</dialog>"#
        )),
        ComponentKind::GroupedTabList => HtmlNode(format!(
            r#"<nav class="sui-grouped-tabs" {marker} data-sui-role="tab-list" data-sui-name="{label_attr}" aria-label="{label_attr}">{child_html}</nav>"#
        )),
        ComponentKind::TabGroup => HtmlNode(format!(
            r#"<section class="sui-tab-group" {marker} data-sui-role="group" data-sui-name="{label_attr}" aria-label="{label_attr}"><strong>{label}</strong>{child_html}</section>"#
        )),
        ComponentKind::TabNode => HtmlNode(format!(
            r#"<div class="sui-tab-node" {marker} data-sui-role="group" data-sui-name="{label_attr}">{child_html}</div>"#
        )),
        ComponentKind::TabBar => HtmlNode(format!(
            r#"<div class="sui-tab-bar" {marker} role="tablist" data-sui-role="tab-list" data-sui-name="{label_attr}" aria-label="{label_attr}">{child_html}</div>"#
        )),
        ComponentKind::Tab => {
            let events = props
                .on_click
                .take()
                .map(|handler| renderer.handler(RustHandler::Click(handler), "onclick", "value:''"))
                .unwrap_or_default();
            HtmlNode(format!(
                r#"<button class="sui-tab" {marker} role="tab" data-sui-role="tab" data-sui-name="{label_attr}"{selected}{disabled}{events}>{label}</button>"#
            ))
        }
        ComponentKind::List => HtmlNode(format!(
            r#"<ul class="sui-list" {marker} data-sui-role="list" data-sui-name="{label_attr}" aria-label="{label_attr}">{child_html}</ul>"#
        )),
        ComponentKind::ListItem => {
            let events = if props.disabled {
                String::new()
            } else {
                props
                    .on_click
                    .take()
                    .map(|handler| {
                        renderer.handler(RustHandler::Click(handler), "onclick", "value:''")
                    })
                    .unwrap_or_default()
            };
            let keyboard = if events.is_empty() {
                ""
            } else {
                r#" tabindex="0" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();this.click()}""#
            };
            let item_state = if props.disabled {
                r#" aria-disabled="true""#
            } else {
                ""
            };
            HtmlNode(format!(
                r#"<li class="sui-list-item" {marker} data-sui-role="list-item" data-sui-name="{label_attr}"{selected}{item_state}{keyboard}{events}>{label}{child_html}</li>"#
            ))
        }
        ComponentKind::Dropdown => {
            let events = props
                .on_input
                .take()
                .map(|handler| {
                    renderer.handler(RustHandler::Input(handler), "onchange", "value:this.value")
                })
                .unwrap_or_default();
            HtmlNode(format!(
                r#"<label class="sui-dropdown-field" {marker}><span>{label}</span><select class="sui-dropdown" data-sui-role="combo-box" data-sui-name="{label_attr}" aria-label="{label_attr}"{disabled}{events}>{child_html}</select></label>"#
            ))
        }
        ComponentKind::DropdownOption => HtmlNode(format!(
            r#"<option class="sui-dropdown-option" {marker} data-sui-role="list-box-option" data-sui-name="{label_attr}" value="{}"{}>{label}</option>"#,
            escape_attr(if props.value.is_empty() {
                &props.label
            } else {
                &props.value
            }),
            if props.selected { " selected" } else { "" }
        )),
        ComponentKind::Table => HtmlNode(format!(
            r#"<table class="sui-table" {marker} data-sui-role="table" data-sui-name="{label_attr}" aria-label="{label_attr}"><tbody>{child_html}</tbody></table>"#
        )),
        ComponentKind::TableRow => {
            let cells = props
                .items
                .iter()
                .map(|cell| format!("<td>{}</td>", escape_text(cell)))
                .collect::<String>();
            let events = if props.disabled {
                String::new()
            } else {
                props
                    .on_click
                    .take()
                    .map(|handler| {
                        renderer.handler(RustHandler::Click(handler), "onclick", "value:''")
                    })
                    .unwrap_or_default()
            };
            let keyboard = if events.is_empty() {
                ""
            } else {
                r#" tabindex="0" onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();this.click()}""#
            };
            let row_state = if props.disabled {
                r#" aria-disabled="true""#
            } else {
                ""
            };
            HtmlNode(format!(
                r#"<tr class="sui-table-row" {marker} data-sui-role="table-row" data-sui-name="{label_attr}"{selected}{row_state}{keyboard}{events}>{cells}{child_html}</tr>"#
            ))
        }
        ComponentKind::RichText => HtmlNode(format!(
            r#"<article class="sui-rich-text" {marker} data-sui-role="document" data-sui-name="{label_attr}" aria-label="{label_attr}"><h3>{label}</h3><p>{}</p>{child_html}</article>"#,
            escape_text(&props.value)
        )),
        ComponentKind::TextArea => {
            let events = props
                .on_input
                .take()
                .map(|handler| {
                    renderer.handler(RustHandler::Input(handler), "oninput", "value:this.value")
                })
                .unwrap_or_default();
            HtmlNode(format!(
                r#"<label class="sui-textarea-field" {marker}><span>{label}</span><textarea class="sui-textarea" data-sui-role="multiline-text-input" data-sui-name="{label_attr}" aria-label="{label_attr}" placeholder="{}"{disabled}{events}>{}</textarea></label>"#,
                escape_attr(&props.detail),
                escape_text(&props.value)
            ))
        }
        ComponentKind::Svg => HtmlNode(format!(
            r#"<figure class="sui-svg" {marker} data-sui-role="image" data-sui-name="{label_attr}" aria-label="{label_attr}" style="{style}">{}</figure>"#,
            props.value
        )),
        ComponentKind::ThemeProvider => HtmlNode(format!(
            r#"<section class="sui-theme-provider" {marker} data-theme="{}">{child_html}</section>"#,
            escape_attr(&props.value)
        )),
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
    }
}

fn children_html(children: Vec<HtmlNode>) -> String {
    children.into_iter().map(|child| child.0).collect()
}

fn insert_root_attribute(html: &mut String, attribute: &str) {
    let end = html
        .find('>')
        .expect("renderer nodes always start with an HTML element");
    html.insert_str(end, attribute);
}

fn component_attribute(kind: ComponentKind) -> String {
    format!(r#"data-sui-component="{}""#, kind.as_str())
}

fn responsive_condition(query: ResponsiveQuery) -> String {
    fn length(value: Length) -> String {
        match value {
            Length::Px(value) => format!("{value}px"),
            Length::Em(value) => format!("{value}em"),
        }
    }

    let mut conditions = Vec::new();
    for (name, value) in [
        ("min-width", query.min_width),
        ("max-width", query.max_width),
        ("min-height", query.min_height),
        ("max-height", query.max_height),
    ] {
        if let Some(value) = value {
            conditions.push(format!("({name}: {})", length(value)));
        }
    }
    if conditions.is_empty() {
        "(min-width: 0px)".into()
    } else {
        conditions.join(" and ")
    }
}

pub(crate) fn document(
    width: u32,
    height: u32,
    theme: Theme,
    responsive_css: &str,
    body: &HtmlNode,
) -> String {
    fn interaction_css(style: schnellui_widgets::InteractionStyle, focus: bool) -> String {
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
    use schnellui_widgets::{InteractionComponent as Component, InteractionState as State};
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
    // The spinner animation is declared once (schnellui-motion) and compiled to
    // CSS here; the GPU renderer samples the exact same declaration per frame.
    let spin = schnellui_motion::css_animation(&SPINNER_MOTION, "sui-spin");
    let spin_shorthand = spin.shorthand;
    let spin_keyframes = spin.keyframes;
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
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
</style>
</head>
<body><main id="schnellui-root">{body}</main>
<script>
(() => {{
  let active = null;
  let pointerY = 0;
  let frame = 0;
  const tick = () => {{
    if (!active) return;
    const rect = active.getBoundingClientRect();
    const zone = Math.min(24, rect.height / 2);
    const delta = pointerY <= rect.top + zone ? -12 :
      pointerY >= rect.bottom - zone ? 12 : 0;
    if (delta) active.scrollTop += delta;
    frame = requestAnimationFrame(tick);
  }};
  document.addEventListener('pointerdown', event => {{
    if (event.button !== 0) return;
    active = event.target.closest('.sui-scroll[data-sui-edge-auto-scroll="true"]');
    if (!active) return;
    pointerY = event.clientY;
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(tick);
  }});
  document.addEventListener('pointermove', event => {{ pointerY = event.clientY; }});
  const stop = () => {{ active = null; cancelAnimationFrame(frame); }};
  document.addEventListener('pointerup', stop);
  document.addEventListener('pointercancel', stop);
}})();
</script>
</body>
</html>"#,
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
        body = body.0,
    )
}

fn element(
    tag: &str,
    class: &str,
    style: &str,
    attributes: &str,
    children: Vec<HtmlNode>,
) -> HtmlNode {
    let mut html = format!(r#"<{tag} class="{class}" style="{style}"{attributes}>"#);
    for child in children {
        html.push_str(&child.0);
    }
    let _ = write!(html, "</{tag}>");
    HtmlNode(html)
}

pub(crate) fn normalize_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

fn px(value: f32) -> String {
    format!("{value}px")
}

fn css_decl(output: &mut String, property: &str, value: &str) {
    let _ = write!(output, "{property}:{value};");
}

fn optional_px(output: &mut String, property: &str, value: Option<f32>) {
    if let Some(value) = value {
        css_decl(output, property, &px(value));
    }
}

fn optional_number(output: &mut String, property: &str, value: Option<f32>) {
    if let Some(value) = value {
        css_decl(output, property, &value.to_string());
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn role_name(role: Role) -> &'static str {
    match role {
        Role::Group => "group",
        Role::Label => "label",
        Role::Button => "button",
        Role::CheckBox => "checkbox",
        Role::Slider => "slider",
        Role::TextInput => "text-input",
        Role::Image => "image",
        Role::List => "list",
        Role::Status => "status",
        Role::ProgressIndicator => "progress-indicator",
        Role::Switch => "switch",
        Role::Radio => "radio",
        Role::ScrollView => "scroll-view",
        Role::Chart => "chart",
        Role::Link => "link",
        Role::Tab => "tab",
        Role::TabList => "tab-list",
        Role::ListItem => "list-item",
        Role::Table => "table",
        Role::TableRow => "table-row",
        Role::Cell => "cell",
        Role::ColumnHeader => "column-header",
        Role::Document => "document",
        Role::MultilineTextInput => "multiline-text-input",
        Role::ComboBox => "combo-box",
        Role::ListBoxOption => "list-box-option",
        Role::Dialog => "dialog",
        Role::AlertDialog => "alert-dialog",
        Role::Menu => "menu",
        Role::MenuItem => "menu-item",
        Role::PasswordInput => "password-input",
    }
}

pub(crate) fn color_css(color: schnellui_scene::Color) -> String {
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

fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(value: &str) -> String {
    escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
