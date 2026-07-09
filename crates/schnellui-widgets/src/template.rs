//! Adapter from renderer-generic templates to the retained schnellui widget tree.

use schnellui_layout::{Container as LayoutContainer, ContainerStyle};
use schnellui_template::{
    ButtonAppearance as TemplateButtonAppearance, ButtonProps, CheckboxProps, ComponentKind,
    ComponentProps, ContainerKind, ContainerProps, SliderProps, TemplateRenderer,
    TextAlign as TemplateTextAlign, TextContent, TextInputProps, TextProps,
    WrapMode as TemplateWrapMode,
};

use crate::{
    AnyView, Button, ButtonAppearance, Checkbox, Column, Flex, Pad, Row, Scroll, Slider, Spacer,
    Stack, Text, TextAlign, TextInput, View, WrapMode,
};

/// Retained-tree implementation of [`TemplateRenderer`].
///
/// It is intentionally an adapter rather than another component implementation:
/// defaults and composition live in `schnellui-template`; this type only maps those
/// properties to the existing retained widgets.
#[derive(Default)]
pub struct SceneTemplate;

impl SceneTemplate {
    fn style(props: ContainerProps, container: LayoutContainer) -> ContainerStyle {
        let mut style = ContainerStyle::new(container);
        style.justify = props.justify;
        style.align = props.align;
        style.gap = props.gap;
        style.wrap = props.wrap;
        style.fill = props.fill;
        style.width = props.width;
        style.height = props.height;
        style.min_width = props.min_width;
        style.min_height = props.min_height;
        style
    }

    fn one_child(mut children: Vec<AnyView>) -> Option<AnyView> {
        match children.len() {
            0 => None,
            1 => children.pop(),
            _ => Some(Box::new(Column {
                children,
                style: ContainerStyle::new(LayoutContainer::Column),
            })),
        }
    }
}

impl TemplateRenderer for SceneTemplate {
    type Node = AnyView;

    fn container(&mut self, props: ContainerProps, children: Vec<Self::Node>) -> Self::Node {
        match props.kind {
            ContainerKind::Row => Box::new(Row {
                children,
                style: Self::style(props, LayoutContainer::Row),
            }),
            ContainerKind::Column => Box::new(Column {
                children,
                style: Self::style(props, LayoutContainer::Column),
            }),
            ContainerKind::Stack => Box::new(Stack {
                children,
                style: Self::style(props, LayoutContainer::Stack),
            }),
            ContainerKind::Scroll => {
                let scrollbar = props.scrollbar;
                let edge_auto_scroll = props.edge_auto_scroll;
                Box::new(Scroll {
                    child: Self::one_child(children),
                    style: Self::style(props, LayoutContainer::Scroll),
                    on_scroll: None,
                    on_scroll_debounced: None,
                    name: None,
                    scrollbar,
                    edge_auto_scroll,
                    follow_end: false,
                    initial_offset: None,
                    restoration_key: None,
                })
            }
        }
    }

    fn pad(
        &mut self,
        insets: schnellui_layout::EdgeInsets,
        child: Option<Self::Node>,
    ) -> Self::Node {
        Box::new(Pad { insets, child })
    }

    fn spacer(&mut self) -> Self::Node {
        Box::new(Spacer::new())
    }

    fn flex(
        &mut self,
        props: schnellui_layout::FlexChild,
        child: Option<Self::Node>,
    ) -> Self::Node {
        Box::new(Flex { child, flex: props })
    }

    fn responsive(
        &mut self,
        query: schnellui_layout::ResponsiveQuery,
        child: Self::Node,
    ) -> Self::Node {
        Box::new(crate::Responsive::new(query).child(child))
    }

    fn component_ref(
        &mut self,
        reference: schnellui_scene::ComponentRef,
        child: Self::Node,
    ) -> Self::Node {
        Box::new(crate::Referenced::new(reference).child(child))
    }

    fn text(&mut self, props: TextProps) -> Self::Node {
        let text = match props.content {
            TextContent::Static(value) => Text::new(value),
            TextContent::Dynamic(producer) => Text::dynamic(producer),
        };
        let wrap = match props.wrap {
            TemplateWrapMode::NoWrap => WrapMode::NoWrap,
            TemplateWrapMode::Word => WrapMode::Word,
            TemplateWrapMode::Anywhere => WrapMode::Anywhere,
        };
        let align = match props.align {
            TemplateTextAlign::Start => TextAlign::Start,
            TemplateTextAlign::Center => TextAlign::Center,
            TemplateTextAlign::End => TextAlign::End,
            TemplateTextAlign::Justify => TextAlign::Justify,
        };
        let text = text
            .size(props.size)
            .role(props.role)
            .wrap(wrap)
            .align(align);
        Box::new(if props.ellipsis {
            text.ellipsis()
        } else {
            text
        })
    }

    fn button(&mut self, mut props: ButtonProps) -> Self::Node {
        let appearance = match props.appearance {
            TemplateButtonAppearance::Solid => ButtonAppearance::Solid,
            TemplateButtonAppearance::Ghost => ButtonAppearance::Ghost,
        };
        let mut button = Button::new(props.label)
            .disabled(props.disabled)
            .appearance(appearance);
        if let Some(width) = props.width {
            button = button.width(width);
        }
        if let Some(height) = props.height {
            button = button.height(height);
        }
        if let Some(handler) = props.on_click.take() {
            button = button.on_click(handler);
        }
        Box::new(button)
    }

    fn checkbox(&mut self, mut props: CheckboxProps) -> Self::Node {
        let mut checkbox = Checkbox::new(props.checked);
        if let Some(name) = props.name {
            checkbox = checkbox.name(name);
        }
        if let Some(handler) = props.on_toggle.take() {
            checkbox = checkbox.on_toggle(handler);
        }
        Box::new(checkbox)
    }

    fn slider(&mut self, mut props: SliderProps) -> Self::Node {
        let mut slider = Slider::new(props.value, props.min, props.max).disabled(props.disabled);
        if let Some(step) = props.step {
            slider = slider.step(step);
        }
        if let Some(name) = props.name {
            slider = slider.name(name);
        }
        if let Some(handler) = props.on_change.take() {
            slider = slider.on_change(handler);
        }
        Box::new(slider)
    }

    fn text_input(&mut self, mut props: TextInputProps) -> Self::Node {
        if props.password {
            let mut input = crate::PasswordInput::new(props.value).label(props.label);
            if let Some(handler) = props.on_input.take() {
                input = input.on_input(handler);
            }
            Box::new(input)
        } else {
            let mut input = TextInput::new(props.value).label(props.label);
            if let Some(handler) = props.on_input.take() {
                input = input.on_input(handler);
            }
            Box::new(input)
        }
    }

    fn component(&mut self, mut props: ComponentProps, children: Vec<Self::Node>) -> Self::Node {
        match props.kind {
            ComponentKind::Image => {
                let mut image = crate::Image::new(props.value).alt(props.label);
                if let (Some(width), Some(height)) = (props.width, props.height) {
                    image = image.size(width, height);
                }
                Box::new(image)
            }
            ComponentKind::Icon => Box::new(crate::Icon::new(props.label)),
            ComponentKind::ProgressBar => Box::new(
                crate::ProgressBar::new(props.number, props.min, props.max).name(props.label),
            ),
            ComponentKind::LoadingSpinner => {
                Box::new(crate::LoadingSpinner::new().name(props.label))
            }
            ComponentKind::Switch => {
                let mut widget = crate::Switch::new(props.checked);
                if let Some(handler) = props.on_toggle.take() {
                    widget = widget.on_toggle(handler);
                }
                Box::new(widget)
            }
            ComponentKind::Radio => {
                let mut widget = crate::Radio::new(props.selected);
                if let Some(handler) = props.on_click.take() {
                    widget = widget.on_select(handler);
                }
                Box::new(widget)
            }
            ComponentKind::Divider => Box::new(crate::Divider::new()),
            ComponentKind::Link => {
                let mut widget = crate::Link::new(props.label).disabled(props.disabled);
                if let Some(handler) = props.on_click.take() {
                    widget = widget.on_click(handler);
                }
                Box::new(widget)
            }
            ComponentKind::Badge => Box::new(crate::Badge::new(props.label)),
            ComponentKind::TextArea => {
                let mut widget = crate::TextArea::new(props.value).placeholder(props.label);
                if let Some(handler) = props.on_input.take() {
                    widget = widget.on_input(handler);
                }
                Box::new(widget)
            }
            ComponentKind::Svg => {
                let mut widget = crate::Svg::new(props.value).alt(props.label);
                if let (Some(width), Some(height)) = (props.width, props.height) {
                    widget = widget.size(width, height);
                }
                Box::new(widget)
            }
            // These structural/configuration widgets need data that has already
            // been rendered to type-erased children. Preserve the exact generic
            // tree as retained layout instead of reimplementing component logic.
            _ => {
                let mut all =
                    Vec::with_capacity(children.len() + usize::from(!props.label.is_empty()));
                if !props.label.is_empty() {
                    all.push(Box::new(Text::new(props.label)) as AnyView);
                }
                all.extend(children);
                Box::new(Column {
                    children: all,
                    style: ContainerStyle::new(LayoutContainer::Column),
                })
            }
        }
    }
}

/// Lets an adapter-produced type-erased child be mounted as an ordinary retained
/// view. The extra box is consumed immediately and does not survive in the scene.
impl View for AnyView {
    fn build(
        self: Box<Self>,
        ctx: &mut crate::BuildCtx,
        parent: Option<schnellui_scene::WidgetId>,
    ) -> schnellui_scene::WidgetId {
        (*self).build(ctx, parent)
    }
}

#[cfg(test)]
mod tests {
    use schnellui_scene::{Scene, WidgetKind};
    use schnellui_template::{column, Template, Text};

    use super::*;
    use crate::BuildCtx;

    #[test]
    fn generic_template_builds_the_retained_tree() {
        let root = column()
            .gap(4.0)
            .child(Text::new("one"))
            .child(Text::new("two"))
            .render(&mut SceneTemplate);
        let mut scene = Scene::new();
        let mut layout = schnellui_layout::LayoutEngine::new();
        let mut text = schnellui_text::TextShaper::new();
        let mut atlas = schnellui_text::GlyphAtlas::new(128, 128);
        let mut ctx = BuildCtx {
            context: crate::Context::new(),
            runtime: crate::Runtime::new(),
            scene: &mut scene,
            layout: &mut layout,
            text: &mut text,
            atlas: &mut atlas,
            scale: 1.0,
        };
        let id = Box::new(root).build(&mut ctx, None);
        assert_eq!(scene.node(id).unwrap().kind, WidgetKind::Column);
        assert_eq!(scene.node(id).unwrap().children.len(), 2);
    }
}
