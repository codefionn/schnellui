//! The [`TemplateRenderer`] that builds an [`AndroidNode`] tree.

use schnellui::scene::Size;
use schnellui::template::{
    Align, ButtonAppearance, ButtonProps, CheckboxProps, ComponentKind, ComponentProps,
    ComponentRef, ContainerKind, ContainerProps, EdgeInsets, FlexChild, Justify, ResponsiveQuery,
    ResponsiveTarget, Role, SliderProps, TemplateRenderer, TextAlign, TextContent, TextInputProps,
    TextProps, WrapMode,
};

use super::ops::{kind, prop};
use super::renderer::Viewport;
use super::tree::{AndroidNode, OptionMarker};

/// Renders templates into an [`AndroidNode`] tree.
pub struct AndroidTemplate {
    viewport: Viewport,
}

impl AndroidTemplate {
    pub fn new(viewport: Viewport) -> Self {
        Self { viewport }
    }
}

fn justify_code(justify: Justify) -> i32 {
    match justify {
        Justify::Start => 0,
        Justify::Center => 1,
        Justify::End => 2,
        Justify::SpaceBetween => 3,
        Justify::SpaceAround => 4,
        Justify::SpaceEvenly => 5,
    }
}

fn align_code(align: Align) -> i32 {
    match align {
        Align::Start => 0,
        Align::Center => 1,
        Align::End => 2,
        Align::Stretch => 3,
    }
}

fn container_kind(kind: ContainerKind) -> u8 {
    match kind {
        ContainerKind::Row => kind::ROW,
        ContainerKind::Column => kind::COLUMN,
        ContainerKind::Stack => kind::STACK,
        ContainerKind::Scroll => kind::SCROLL,
    }
}

fn labelled(node: &mut AndroidNode, label: &str) {
    if !label.is_empty() {
        node.str(prop::A11Y_LABEL, label);
    }
}

/// Moves the key of `child` to a wrapper, so the wrapper is what its siblings match.
fn wrap_in_stack(mut child: AndroidNode) -> AndroidNode {
    let mut wrapper = AndroidNode::new(kind::STACK);
    wrapper.key = child.key.take();
    wrapper.children.push(child);
    wrapper
}

fn apply_flex(node: &mut AndroidNode, props: &FlexChild) {
    node.opt_f32(prop::WIDTH, props.width);
    node.opt_f32(prop::HEIGHT, props.height);
    node.opt_f32(prop::MIN_WIDTH, props.min_width);
    node.opt_f32(prop::MIN_HEIGHT, props.min_height);
    node.opt_f32(prop::MAX_WIDTH, props.max_width);
    node.opt_f32(prop::MAX_HEIGHT, props.max_height);
    node.opt_f32(prop::GROW, props.grow);
    node.opt_f32(prop::BASIS, props.basis);
}

fn apply_insets(node: &mut AndroidNode, insets: EdgeInsets) {
    node.pad(insets.left, insets.top, insets.right, insets.bottom);
}

fn set_text(node: &mut AndroidNode, text: &str) {
    if !text.is_empty() {
        node.str(prop::TEXT, text);
    }
}

impl AndroidTemplate {
    /// Puts a visible 14 sp label above `widget` when `label` is not empty.
    fn with_label(&mut self, label: &str, widget: AndroidNode) -> AndroidNode {
        if label.is_empty() {
            return widget;
        }
        let text = self.text(TextProps {
            content: TextContent::Static(label.to_owned().into()),
            size: 14.0,
            role: Role::Label,
            wrap: WrapMode::NoWrap,
            align: TextAlign::Start,
            ellipsis: false,
        });
        let mut column = AndroidNode::new(kind::COLUMN);
        column.children.push(text);
        column.children.push(widget);
        column
    }
}

impl TemplateRenderer for AndroidTemplate {
    type Node = AndroidNode;

    fn container(&mut self, props: ContainerProps, children: Vec<Self::Node>) -> Self::Node {
        let mut node = AndroidNode::new(container_kind(props.kind));
        node.children = children;
        node.opt_f32(prop::WIDTH, props.width);
        node.opt_f32(prop::HEIGHT, props.height);
        node.opt_f32(prop::MIN_WIDTH, props.min_width);
        node.opt_f32(prop::MIN_HEIGHT, props.min_height);
        if props.fill {
            node.bool(prop::FILL, true);
        }
        match props.kind {
            ContainerKind::Row | ContainerKind::Column => {
                if props.justify != Justify::Start {
                    node.i32(prop::JUSTIFY, justify_code(props.justify));
                }
                if props.align != Align::Start {
                    node.i32(prop::ALIGN, align_code(props.align));
                }
                if props.gap != 0.0 {
                    node.f32(prop::GAP, props.gap);
                }
                if props.wrap {
                    node.bool(prop::WRAP, true);
                }
            }
            ContainerKind::Stack => {
                if props.align != Align::Start {
                    node.i32(prop::ALIGN, align_code(props.align));
                }
            }
            ContainerKind::Scroll => {
                if props.scrollbar {
                    node.bool(prop::SCROLLBAR, true);
                }
            }
        }
        node
    }

    fn pad(&mut self, insets: EdgeInsets, child: Option<Self::Node>) -> Self::Node {
        let mut node = match child {
            Some(child) => wrap_in_stack(child),
            None => AndroidNode::new(kind::SPACER),
        };
        apply_insets(&mut node, insets);
        node
    }

    fn spacer(&mut self) -> Self::Node {
        let mut node = AndroidNode::new(kind::SPACER);
        node.f32(prop::GROW, 1.0);
        node
    }

    fn flex(&mut self, props: FlexChild, child: Option<Self::Node>) -> Self::Node {
        let mut node = match (child, props.padding) {
            (Some(child), Some(insets)) => {
                let mut wrapper = wrap_in_stack(child);
                apply_insets(&mut wrapper, insets);
                wrapper
            }
            (Some(child), None) => child,
            (None, padding) => {
                let mut spacer = AndroidNode::new(kind::SPACER);
                if let Some(insets) = padding {
                    apply_insets(&mut spacer, insets);
                }
                spacer
            }
        };
        apply_flex(&mut node, &props);
        node
    }

    fn responsive(&mut self, query: ResponsiveQuery, mut child: Self::Node) -> Self::Node {
        if query.target == ResponsiveTarget::Viewport {
            let size = Size {
                width: self.viewport.width,
                height: self.viewport.height,
            };
            if !query.matches(size) {
                child.bool(prop::VISIBLE, false);
            }
        }
        child
    }

    fn component_ref(&mut self, _reference: ComponentRef, child: Self::Node) -> Self::Node {
        child
    }

    /// A node holds one key. A later call replaces the earlier one.
    fn keyed(&mut self, key: &str, mut child: Self::Node) -> Self::Node {
        child.key = Some(key.to_owned());
        child
    }

    fn text(&mut self, props: TextProps) -> Self::Node {
        let mut node = AndroidNode::new(kind::TEXT);
        let content = match props.content {
            TextContent::Static(text) => text.into_owned(),
            TextContent::Dynamic(mut producer) => producer(),
        };
        if !content.is_empty() {
            node.str(prop::TEXT, content);
        }
        node.f32(prop::TEXT_SIZE, props.size);
        if props.role != Role::Label {
            node.i32(prop::ROLE, props.role.as_u16() as i32);
        }
        match props.wrap {
            WrapMode::NoWrap => {}
            WrapMode::Word => node.i32(prop::TEXT_WRAP, 1),
            WrapMode::Anywhere => node.i32(prop::TEXT_WRAP, 2),
        }
        match props.align {
            TextAlign::Start => {}
            TextAlign::Center => node.i32(prop::TEXT_ALIGN, 1),
            TextAlign::End => node.i32(prop::TEXT_ALIGN, 2),
            TextAlign::Justify => node.i32(prop::TEXT_ALIGN, 3),
        }
        if props.ellipsis {
            node.bool(prop::ELLIPSIS, true);
        }
        node
    }

    fn button(&mut self, props: ButtonProps) -> Self::Node {
        let mut node = AndroidNode::new(kind::BUTTON);
        if !props.label.is_empty() {
            node.str(prop::TEXT, props.label.into_owned());
        }
        if props.disabled {
            node.bool(prop::ENABLED, false);
        }
        node.opt_f32(prop::WIDTH, props.width);
        node.opt_f32(prop::HEIGHT, props.height);
        if props.appearance == ButtonAppearance::Ghost {
            node.i32(prop::APPEARANCE, 1);
        }
        node.handlers.click = props.on_click;
        node
    }

    fn checkbox(&mut self, props: CheckboxProps) -> Self::Node {
        let mut node = AndroidNode::new(kind::CHECKBOX);
        node.bool(prop::CHECKED, props.checked);
        if let Some(name) = props.name {
            labelled(&mut node, &name);
        }
        node.handlers.toggle = props.on_toggle;
        node
    }

    fn slider(&mut self, props: SliderProps) -> Self::Node {
        let mut node = AndroidNode::new(kind::SLIDER);
        node.f32(prop::VALUE, props.value);
        node.f32(prop::MIN, props.min);
        node.f32(prop::MAX, props.max);
        node.opt_f32(prop::STEP, props.step);
        if let Some(name) = props.name {
            labelled(&mut node, &name);
        }
        if props.disabled {
            node.bool(prop::ENABLED, false);
        }
        node.handlers.change = props.on_change;
        node
    }

    fn text_input(&mut self, props: TextInputProps) -> Self::Node {
        let mut node = AndroidNode::new(kind::TEXT_INPUT);
        node.str(prop::TEXT, props.value);
        labelled(&mut node, &props.label);
        if props.password {
            node.bool(prop::PASSWORD, true);
        }
        node.handlers.input = props.on_input;
        node
    }

    fn component(&mut self, props: ComponentProps, children: Vec<Self::Node>) -> Self::Node {
        let mut node = match props.kind {
            ComponentKind::Row
            | ComponentKind::Column
            | ComponentKind::Stack
            | ComponentKind::Scroll => {
                let kind = match props.kind {
                    ComponentKind::Row => ContainerKind::Row,
                    ComponentKind::Column => ContainerKind::Column,
                    ComponentKind::Stack => ContainerKind::Stack,
                    _ => ContainerKind::Scroll,
                };
                let mut container = ContainerProps::new(kind);
                container.width = props.width;
                container.height = props.height;
                self.container(container, children)
            }
            ComponentKind::Text => {
                let text = if props.label.is_empty() {
                    props.value.clone()
                } else {
                    props.label.clone().into_owned()
                };
                let mut node = self.text(TextProps {
                    content: TextContent::Static(text.into()),
                    size: 16.0,
                    role: Role::Label,
                    wrap: WrapMode::NoWrap,
                    align: TextAlign::Start,
                    ellipsis: false,
                });
                node.opt_f32(prop::WIDTH, props.width);
                node.opt_f32(prop::HEIGHT, props.height);
                node
            }
            ComponentKind::Button => self.button(ButtonProps {
                label: props.label.clone(),
                disabled: props.disabled,
                width: props.width,
                height: props.height,
                appearance: ButtonAppearance::Solid,
                on_click: props.on_click,
            }),
            ComponentKind::Checkbox => {
                let name = (!props.label.is_empty()).then(|| props.label.clone());
                self.checkbox(CheckboxProps {
                    checked: props.checked,
                    name,
                    on_toggle: props.on_toggle,
                })
            }
            ComponentKind::Slider => {
                let name = (!props.label.is_empty()).then(|| props.label.clone());
                self.slider(SliderProps {
                    value: props.number,
                    min: props.min,
                    max: props.max,
                    step: None,
                    name,
                    disabled: props.disabled,
                    on_change: props.on_change,
                })
            }
            ComponentKind::TextInput => self.text_input(TextInputProps {
                value: props.value.clone(),
                label: props.label.clone(),
                on_input: props.on_input,
                password: false,
            }),
            ComponentKind::Spacer => self.spacer(),
            ComponentKind::Switch => {
                let mut node = AndroidNode::new(kind::SWITCH);
                set_text(&mut node, &props.label);
                node.bool(prop::CHECKED, props.checked);
                node.handlers.toggle = props.on_toggle;
                node
            }
            ComponentKind::Radio => {
                let mut node = AndroidNode::new(kind::RADIO);
                set_text(&mut node, &props.label);
                node.bool(prop::CHECKED, props.selected || props.checked);
                node.handlers.click = props.on_click;
                node
            }
            ComponentKind::Dropdown => {
                let mut node = AndroidNode::new(kind::DROPDOWN);
                let mut labels: Vec<String> = Vec::new();
                let mut values: Vec<String> = Vec::new();
                let mut selected = None;
                for item in &props.items {
                    labels.push(item.clone().into_owned());
                    values.push(item.clone().into_owned());
                }
                for child in children {
                    if let Some(option) = child.option {
                        if option.selected && selected.is_none() {
                            selected = Some(labels.len());
                        }
                        labels.push(option.label);
                        values.push(option.value);
                    }
                }
                if selected.is_none() && !props.value.is_empty() {
                    selected = values.iter().position(|v| *v == props.value);
                }
                if !labels.is_empty() {
                    node.str(prop::ITEMS, labels.join("\u{1f}"));
                }
                if let Some(index) = selected {
                    node.i32(prop::SELECTED_INDEX, index as i32);
                }
                labelled(&mut node, &props.label);
                if props.disabled {
                    node.bool(prop::ENABLED, false);
                }
                node.option_values = values;
                node.handlers.select = props.on_input;
                node.opt_f32(prop::WIDTH, props.width);
                node.opt_f32(prop::HEIGHT, props.height);
                self.with_label(&props.label, node)
            }
            ComponentKind::DropdownOption => {
                let mut node = AndroidNode::new(kind::SPACER);
                let label = props.label.clone().into_owned();
                let value = if props.value.is_empty() {
                    label.clone()
                } else {
                    props.value.clone()
                };
                node.option = Some(OptionMarker {
                    label,
                    value,
                    selected: props.selected,
                });
                node
            }
            ComponentKind::ProgressBar => {
                let mut node = AndroidNode::new(kind::PROGRESS);
                node.f32(prop::VALUE, props.number);
                node.f32(prop::MIN, props.min);
                node.f32(prop::MAX, props.max);
                labelled(&mut node, &props.label);
                if props.disabled {
                    node.bool(prop::ENABLED, false);
                }
                node.opt_f32(prop::WIDTH, props.width);
                node.opt_f32(prop::HEIGHT, props.height);
                self.with_label(&props.label, node)
            }
            ComponentKind::LoadingSpinner => {
                let mut node = AndroidNode::new(kind::LOADING);
                labelled(&mut node, &props.label);
                if props.disabled {
                    node.bool(prop::ENABLED, false);
                }
                node
            }
            ComponentKind::TextArea => {
                let mut node = AndroidNode::new(kind::TEXT_AREA);
                node.str(prop::TEXT, props.value.clone());
                labelled(&mut node, &props.label);
                node.handlers.input = props.on_input;
                node
            }
            ComponentKind::Image => {
                let mut node = AndroidNode::new(kind::IMAGE);
                if !props.value.is_empty() {
                    node.str(prop::SRC, props.value.clone());
                }
                labelled(&mut node, &props.label);
                node
            }
            ComponentKind::Divider => AndroidNode::new(kind::DIVIDER),
            ComponentKind::Badge => {
                let mut node = AndroidNode::new(kind::BADGE);
                set_text(&mut node, &props.label);
                node
            }
            ComponentKind::Link => {
                let mut node = AndroidNode::new(kind::LINK);
                set_text(&mut node, &props.label);
                if !props.value.is_empty() {
                    node.str(prop::HREF, props.value.clone());
                }
                node.handlers.click = props.on_click;
                node
            }
            // The retained renderer has no handler for closing a dialog, so the
            // click slot of the props is the dismiss handler here.
            ComponentKind::Dialog => {
                let mut node = AndroidNode::new(kind::DIALOG);
                set_text(&mut node, &props.label);
                node.children = children;
                node.handlers.dismiss = props.on_click;
                node
            }
            ComponentKind::TabBar => {
                let mut node = AndroidNode::new(kind::TAB_BAR);
                node.children = children;
                labelled(&mut node, &props.label);
                node
            }
            ComponentKind::Tab => {
                let mut node = AndroidNode::new(kind::TAB);
                set_text(&mut node, &props.label);
                node.bool(prop::SELECTED, props.selected);
                node.handlers.click = props.on_click;
                node
            }
            ComponentKind::List => {
                let mut node = AndroidNode::new(kind::COLUMN);
                node.i32(prop::ROLE, Role::List.as_u16() as i32);
                labelled(&mut node, &props.label);
                node.children = children;
                node
            }
            ComponentKind::ListItem => {
                let mut node = AndroidNode::new(kind::LIST_ITEM);
                set_text(&mut node, &props.label);
                node.bool(prop::SELECTED, props.selected);
                node.children = children;
                node.handlers.click = props.on_click;
                node
            }
            _ => {
                let mut column = AndroidNode::new(kind::COLUMN);
                if !props.label.is_empty() {
                    let label = self.text(TextProps {
                        content: TextContent::Static(props.label.clone()),
                        size: 16.0,
                        role: Role::Label,
                        wrap: WrapMode::NoWrap,
                        align: TextAlign::Start,
                        ellipsis: false,
                    });
                    column.children.push(label);
                }
                column.children.extend(children);
                column
            }
        };
        if props.disabled
            && matches!(
                props.kind,
                ComponentKind::Button
                    | ComponentKind::Checkbox
                    | ComponentKind::TextInput
                    | ComponentKind::Switch
                    | ComponentKind::Radio
                    | ComponentKind::TextArea
                    | ComponentKind::Link
                    | ComponentKind::Tab
                    | ComponentKind::ListItem
                    | ComponentKind::Image
                    | ComponentKind::Divider
                    | ComponentKind::Badge
                    | ComponentKind::Dialog
                    | ComponentKind::TabBar
                    | ComponentKind::List
            )
        {
            node.bool(prop::ENABLED, false);
        }
        // Dropdown and ProgressBar set their size on the inner widget above.
        if matches!(
            props.kind,
            ComponentKind::Switch
                | ComponentKind::Radio
                | ComponentKind::LoadingSpinner
                | ComponentKind::TextArea
                | ComponentKind::Image
                | ComponentKind::Divider
                | ComponentKind::Badge
                | ComponentKind::Link
                | ComponentKind::Dialog
                | ComponentKind::TabBar
                | ComponentKind::Tab
                | ComponentKind::List
                | ComponentKind::ListItem
        ) {
            node.opt_f32(prop::WIDTH, props.width);
            node.opt_f32(prop::HEIGHT, props.height);
        }
        node
    }
}
