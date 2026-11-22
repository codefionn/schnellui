//! Adapter from renderer-generic templates to the retained schnellui widget tree.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use schnellui::layout::{Align, Container as LayoutContainer, ContainerStyle, FlexChild};
use schnellui::template::{
    ButtonAppearance as TemplateButtonAppearance, ButtonProps, CheckboxProps, ClickHandler,
    ComponentKind, ComponentProps, ContainerKind, ContainerProps, SliderProps, TemplateRenderer,
    TextAlign as TemplateTextAlign, TextContent, TextInputProps, TextProps,
    WrapMode as TemplateWrapMode,
};

use crate::widgets::{
    AnyView, Button, ButtonAppearance, Checkbox, Column, Flex, Pad, Row, Scroll, Slider, Spacer,
    Stack, Text, TextAlign, TextInput, View, WrapMode,
};

/// Retained-tree implementation of [`TemplateRenderer`].
///
/// It is intentionally an adapter rather than another component implementation:
/// defaults and composition live in `schnellui::template`; this type only maps those
/// properties to the existing retained widgets.
///
/// A dropdown's options and a table's rows are plain data in the retained widget
/// set, but a template renders them as children before their parent. The adapter
/// keeps each option's and row's data until the parent claims it, matched by the
/// address of the placeholder view the child rendered to.
#[derive(Default)]
pub struct SceneTemplate {
    options: Vec<(SlotKey, PendingOption)>,
    rows: Vec<(SlotKey, PendingRow)>,
    /// Claimed placeholders. They stay allocated until the adapter is dropped, so
    /// no later view can reuse the address of one and be mistaken for it.
    retired: Vec<AnyView>,
}

/// The address of a boxed placeholder view. Only compared, never dereferenced.
type SlotKey = *const ();

fn slot_key(view: &AnyView) -> SlotKey {
    &**view as *const dyn View as *const ()
}

struct PendingOption {
    label: Cow<'static, str>,
    value: String,
    selected: bool,
}

struct PendingRow {
    cells: Vec<Cow<'static, str>>,
    selected: bool,
    on_click: Option<ClickHandler>,
}

/// What a `dropdown_option` renders to. Its dropdown claims it and builds a
/// real option. One that no dropdown claims is shown as its label.
struct OptionSlot {
    label: Cow<'static, str>,
}

impl View for OptionSlot {
    fn build(
        self: Box<Self>,
        ctx: &mut crate::widgets::BuildCtx,
        parent: Option<schnellui::scene::WidgetId>,
    ) -> schnellui::scene::WidgetId {
        Box::new(Text::new(self.label)).build(ctx, parent)
    }
}

/// What a `table_row` renders to. Its table claims it and builds a real row.
/// One that no table claims is shown as a row of its cell texts.
struct RowSlot {
    cells: Vec<Cow<'static, str>>,
}

impl View for RowSlot {
    fn build(
        self: Box<Self>,
        ctx: &mut crate::widgets::BuildCtx,
        parent: Option<schnellui::scene::WidgetId>,
    ) -> schnellui::scene::WidgetId {
        let children = self
            .cells
            .into_iter()
            .map(|cell| Box::new(Text::new(cell)) as AnyView)
            .collect();
        let mut style = ContainerStyle::new(LayoutContainer::Row);
        style.gap = 12.0;
        Box::new(Row { children, style }).build(ctx, parent)
    }
}

impl SceneTemplate {
    pub fn new() -> Self {
        Self::default()
    }

    /// Splits `children` into the data of the placeholders recorded in
    /// `pending` and the other children, both in order.
    fn claim<T>(
        pending: &mut Vec<(SlotKey, T)>,
        retired: &mut Vec<AnyView>,
        children: Vec<AnyView>,
    ) -> (Vec<T>, Vec<AnyView>) {
        let mut claimed = Vec::new();
        let mut rest = Vec::new();
        for child in children {
            let key = slot_key(&child);
            match pending.iter().position(|(slot, _)| *slot == key) {
                Some(index) => {
                    claimed.push(pending.remove(index).1);
                    retired.push(child);
                }
                None => rest.push(child),
            }
        }
        (claimed, rest)
    }

    fn column(children: Vec<AnyView>) -> AnyView {
        Box::new(Column {
            children,
            style: ContainerStyle::new(LayoutContainer::Column),
        })
    }

    /// `node` followed by `rest` in a column, or `node` alone.
    fn then(node: AnyView, rest: Vec<AnyView>) -> AnyView {
        if rest.is_empty() {
            return node;
        }
        let mut all = Vec::with_capacity(rest.len() + 1);
        all.push(node);
        all.extend(rest);
        Self::column(all)
    }

    /// A 14 px caption above `node`, as the Android and HTML renderers show it.
    fn captioned(label: Cow<'static, str>, node: AnyView) -> AnyView {
        if label.is_empty() {
            return node;
        }
        let mut style = ContainerStyle::new(LayoutContainer::Column);
        style.gap = 4.0;
        Box::new(Column {
            children: vec![Box::new(Text::new(label).size(14.0)), node],
            style,
        })
    }

    /// `node` with its visible label on the right, for switches and radios.
    fn beside(node: AnyView, label: Cow<'static, str>) -> AnyView {
        if label.is_empty() {
            return node;
        }
        let mut style = ContainerStyle::new(LayoutContainer::Row);
        style.gap = 8.0;
        style.align = Align::Center;
        Box::new(Row {
            children: vec![node, Box::new(Text::new(label))],
            style,
        })
    }

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
                    stable_gutter: false,
                    edge_fade: None,
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
        insets: schnellui::layout::EdgeInsets,
        child: Option<Self::Node>,
    ) -> Self::Node {
        Box::new(Pad { insets, child })
    }

    fn spacer(&mut self) -> Self::Node {
        Box::new(Spacer::new())
    }

    fn flex(
        &mut self,
        props: schnellui::layout::FlexChild,
        child: Option<Self::Node>,
    ) -> Self::Node {
        Box::new(Flex { child, flex: props })
    }

    fn responsive(
        &mut self,
        query: schnellui::layout::ResponsiveQuery,
        child: Self::Node,
    ) -> Self::Node {
        Box::new(crate::widgets::Responsive::new(query).child(child))
    }

    fn component_ref(
        &mut self,
        reference: schnellui::scene::ComponentRef,
        child: Self::Node,
    ) -> Self::Node {
        Box::new(crate::widgets::Referenced::new(reference).child(child))
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
            let mut input = crate::widgets::PasswordInput::new(props.value).label(props.label);
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
                let mut image = crate::widgets::Image::new(props.value).alt(props.label);
                if let (Some(width), Some(height)) = (props.width, props.height) {
                    image = image.size(width, height);
                }
                Box::new(image)
            }
            ComponentKind::Icon => Box::new(crate::widgets::Icon::new(props.label)),
            ComponentKind::ProgressBar => {
                let mut bar = crate::widgets::ProgressBar::new(props.number, props.min, props.max)
                    .name(props.label.clone());
                if let (Some(width), Some(height)) = (props.width, props.height) {
                    bar = bar.size(width, height);
                }
                Self::captioned(props.label, Box::new(bar))
            }
            ComponentKind::LoadingSpinner => {
                Box::new(crate::widgets::LoadingSpinner::new().name(props.label))
            }
            ComponentKind::Switch => {
                let mut widget = crate::widgets::Switch::new(props.checked)
                    .name(props.label.clone())
                    .disabled(props.disabled);
                if let Some(handler) = props.on_toggle.take() {
                    widget = widget.on_toggle(handler);
                }
                Self::beside(Box::new(widget), props.label)
            }
            ComponentKind::Radio => {
                let mut widget = crate::widgets::Radio::new(props.selected || props.checked)
                    .name(props.label.clone());
                if let Some(handler) = props.on_click.take().filter(|_| !props.disabled) {
                    widget = widget.on_select(handler);
                }
                Self::beside(Box::new(widget), props.label)
            }
            ComponentKind::Divider => Box::new(crate::widgets::Divider::new()),
            ComponentKind::Link => {
                let mut widget = crate::widgets::Link::new(props.label).disabled(props.disabled);
                if let Some(handler) = props.on_click.take() {
                    widget = widget.on_click(handler);
                }
                Box::new(widget)
            }
            ComponentKind::Badge => Box::new(crate::widgets::Badge::new(props.label)),
            ComponentKind::TextArea => {
                let placeholder = if props.detail.is_empty() {
                    props.label
                } else {
                    props.detail
                };
                let mut widget =
                    crate::widgets::TextArea::new(props.value).placeholder(placeholder);
                if let Some(handler) = props.on_input.take() {
                    widget = widget.on_input(handler);
                }
                Box::new(widget)
            }
            ComponentKind::Svg => {
                // `value` is SVG source here and an `<img>` data URL in the HTML
                // backend; trusted inline markup is the same source for the GPU.
                let markup = match props.markup.take() {
                    Some(markup) => markup.into_inner(),
                    None => props.value.into(),
                };
                let mut widget = crate::widgets::Svg::new(markup).alt(props.label);
                if let (Some(width), Some(height)) = (props.width, props.height) {
                    widget = widget.size(width, height);
                }
                Box::new(widget)
            }
            ComponentKind::Dialog => {
                let mut dialog = crate::widgets::Dialog::new(props.label);
                for child in children {
                    dialog = dialog.child(child);
                }
                // `ComponentProps` has no dismiss handler, so the click slot is
                // the one, as on Android.
                if let Some(handler) = props.on_click.take() {
                    dialog = dialog.on_dismiss(handler);
                }
                if let Some(width) = props.width {
                    dialog = dialog.width(width);
                }
                if let Some(height) = props.height {
                    dialog = dialog.height(height);
                }
                Box::new(dialog)
            }
            ComponentKind::TabBar => {
                let mut bar = crate::widgets::TabBar::new();
                for child in children {
                    bar = bar.child(child);
                }
                Box::new(bar)
            }
            ComponentKind::Tab => {
                let mut tab = crate::widgets::Tab::new(props.label).selected(props.selected);
                if let Some(handler) = props.on_click.take().filter(|_| !props.disabled) {
                    tab = tab.on_select(handler);
                }
                if let Some(width) = props.width {
                    tab = tab.width(width);
                }
                Box::new(tab)
            }
            ComponentKind::List => {
                let mut list = crate::widgets::List::new();
                for child in children {
                    list = list.child(child);
                }
                Box::new(list)
            }
            ComponentKind::ListItem => {
                let mut item = crate::widgets::ListItem::new(props.label).selected(props.selected);
                if let Some(handler) = props.on_click.take().filter(|_| !props.disabled) {
                    item = item.on_select(handler);
                }
                if children.is_empty() {
                    return Box::new(item);
                }
                // The label takes the free width and the children follow it, as
                // on Android.
                let mut all = Vec::with_capacity(children.len() + 1);
                all.push(Box::new(Flex {
                    child: Some(Box::new(item)),
                    flex: FlexChild {
                        grow: Some(1.0),
                        ..FlexChild::default()
                    },
                }) as AnyView);
                all.extend(children);
                let mut style = ContainerStyle::new(LayoutContainer::Row);
                style.gap = 8.0;
                style.align = Align::Center;
                Box::new(Row {
                    children: all,
                    style,
                })
            }
            ComponentKind::Dropdown => {
                let (claimed, rest) = Self::claim(&mut self.options, &mut self.retired, children);
                let mut options: Vec<PendingOption> = props
                    .items
                    .iter()
                    .map(|item| PendingOption {
                        label: item.clone(),
                        value: item.clone().into_owned(),
                        selected: false,
                    })
                    .collect();
                options.extend(claimed);
                let chosen = options.iter().position(|o| o.selected).or_else(|| {
                    (!props.value.is_empty())
                        .then(|| options.iter().position(|o| o.value == props.value))
                        .flatten()
                });
                // The retained dropdown is controlled: `open` comes from the
                // props and `on_toggle` hears the requested state.
                let mut dropdown =
                    crate::widgets::Dropdown::new(props.label.clone()).open(props.open);
                if let Some(width) = props.width {
                    dropdown = dropdown.min_width(width);
                }
                if let Some(height) = props.height {
                    dropdown = dropdown.min_height(height);
                }
                if let Some(mut toggle) = props.on_toggle.take() {
                    let open = props.open;
                    dropdown = dropdown.on_toggle(move || toggle(!open));
                }
                let on_input = props.on_input.take().map(|h| Rc::new(RefCell::new(h)));
                for (index, option) in options.into_iter().enumerate() {
                    let mut widget = crate::widgets::DropdownOption::new(option.label)
                        .selected(Some(index) == chosen);
                    if let Some(handler) = &on_input {
                        let handler = Rc::clone(handler);
                        let value = option.value;
                        widget = widget.on_select(move || (handler.borrow_mut())(&value));
                    }
                    dropdown = dropdown.option(widget);
                }
                Self::then(Self::captioned(props.label, Box::new(dropdown)), rest)
            }
            ComponentKind::DropdownOption => {
                let slot: AnyView = Box::new(OptionSlot {
                    label: props.label.clone(),
                });
                let value = if props.value.is_empty() {
                    props.label.clone().into_owned()
                } else {
                    props.value
                };
                self.options.push((
                    slot_key(&slot),
                    PendingOption {
                        label: props.label,
                        value,
                        selected: props.selected,
                    },
                ));
                slot
            }
            ComponentKind::Table => {
                let (rows, rest) = Self::claim(&mut self.rows, &mut self.retired, children);
                let mut table = crate::widgets::Table::new();
                let mut handlers = Vec::with_capacity(rows.len());
                let mut selected = None;
                for (index, row) in rows.into_iter().enumerate() {
                    if row.selected && selected.is_none() {
                        selected = Some(index);
                    }
                    let mut retained = crate::widgets::TableRow::new();
                    for cell in row.cells {
                        retained = retained.cell(cell);
                    }
                    table = table.push_row(retained);
                    handlers.push(row.on_click);
                }
                if let Some(index) = selected {
                    table = table.selected_row(index);
                }
                if handlers.iter().any(Option::is_some) {
                    table = table.on_select_row(move |index| {
                        if let Some(Some(handler)) = handlers.get_mut(index) {
                            handler();
                        }
                    });
                }
                Self::then(Box::new(table), rest)
            }
            ComponentKind::TableRow => {
                let slot: AnyView = Box::new(RowSlot {
                    cells: props.items.clone(),
                });
                self.rows.push((
                    slot_key(&slot),
                    PendingRow {
                        cells: props.items,
                        selected: props.selected,
                        on_click: props.on_click.take().filter(|_| !props.disabled),
                    },
                ));
                // Retained table cells hold text only, so a row's children are
                // not shown. They stay allocated with the claimed placeholders.
                self.retired.extend(children);
                slot
            }
            ComponentKind::RichText => {
                let mut doc = crate::widgets::RichDoc::new();
                if !props.label.is_empty() {
                    doc = doc.heading(2, [crate::widgets::RichSpan::plain(props.label)]);
                }
                if !props.value.is_empty() {
                    doc = doc.paragraph([crate::widgets::RichSpan::plain(props.value)]);
                }
                Self::then(Box::new(crate::widgets::RichText::new(doc)), children)
            }
            ComponentKind::ThemeProvider => {
                let theme = match props.value.as_str() {
                    "dark" => schnellui::theme::DARK,
                    "light" => schnellui::theme::LIGHT,
                    _ => return Self::column(children),
                };
                let child = Self::one_child(children).unwrap_or_else(|| Self::column(Vec::new()));
                Box::new(crate::widgets::ThemeProvider::new(theme, child))
            }
            ComponentKind::DragHandle => {
                let mut handle = crate::widgets::DragHandle::new(props.label);
                if let Some(handler) = props.on_click.take() {
                    handle = handle.on_drag_start(handler);
                }
                Box::new(handle)
            }
            // The dock and grouped-tab families need data that the retained
            // widgets take as typed values, not as rendered children. They keep
            // the generic tree as a labelled column.
            _ => {
                let mut all =
                    Vec::with_capacity(children.len() + usize::from(!props.label.is_empty()));
                if !props.label.is_empty() {
                    all.push(Box::new(Text::new(props.label)) as AnyView);
                }
                all.extend(children);
                Self::column(all)
            }
        }
    }
}

/// Lets an adapter-produced type-erased child be mounted as an ordinary retained
/// view. The extra box is consumed immediately and does not survive in the scene.
impl View for AnyView {
    fn build(
        self: Box<Self>,
        ctx: &mut crate::widgets::BuildCtx,
        parent: Option<schnellui::scene::WidgetId>,
    ) -> schnellui::scene::WidgetId {
        (*self).build(ctx, parent)
    }
}

#[cfg(test)]
mod tests {
    use schnellui::scene::{Scene, WidgetKind};
    use schnellui::template::{column, Template, Text};

    use super::*;
    use crate::widgets::BuildCtx;

    #[test]
    fn generic_template_builds_the_retained_tree() {
        let root = column()
            .gap(4.0)
            .child(Text::new("one"))
            .child(Text::new("two"))
            .render(&mut SceneTemplate::new());
        let mut scene = Scene::new();
        let mut layout = schnellui::layout::LayoutEngine::new();
        let mut text = schnellui::text::TextShaper::new();
        let mut atlas = schnellui::text::GlyphAtlas::new(128, 128);
        let mut ctx = BuildCtx {
            context: crate::widgets::Context::new(),
            runtime: crate::widgets::Runtime::new(),
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
