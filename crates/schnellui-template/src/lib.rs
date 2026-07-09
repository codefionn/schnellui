//! Renderer-generic component templates.
//!
//! A template is an ordinary, statically typed Rust value. Rendering consumes it
//! through [`TemplateRenderer`], whose associated `Node` lets a backend produce a
//! retained widget, an HTML fragment, or another native representation. Component
//! composition is generic and uses a nested tuple instead of type erasure, so the
//! same component definition can be mounted by more than one renderer.

use std::borrow::Cow;

pub use schnellui_a11y::Role;
pub use schnellui_layout::{
    em, px, Align, ContainerStyle, EdgeInsets, FlexChild, Justify, Length, ResponsiveQuery,
    ResponsiveTarget,
};
pub use schnellui_scene::ComponentRef;

/// The small set of layout primitives shared by renderers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Row,
    Column,
    Stack,
    Scroll,
}

/// Backend-neutral text wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WrapMode {
    #[default]
    NoWrap,
    Word,
    Anywhere,
}

/// Backend-neutral horizontal text alignment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

/// Every public widget-facing component that a template backend must account for.
///
/// [`ALL`](ComponentKind::ALL) is intentionally exhaustive: renderer gallery tests
/// use it as a coverage oracle, so adding a widget here without rendering it fails
/// visibly instead of silently falling back to an untested path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    Row,
    Column,
    Stack,
    DockArea,
    Pad,
    Spacer,
    Flex,
    Scroll,
    Text,
    DragHandle,
    Button,
    Checkbox,
    Slider,
    TextInput,
    Image,
    Icon,
    ProgressBar,
    LoadingSpinner,
    Switch,
    Radio,
    Divider,
    Link,
    Badge,
    Dialog,
    GroupedTabList,
    TabGroup,
    TabNode,
    TabBar,
    Tab,
    List,
    ListItem,
    Dropdown,
    DropdownOption,
    Table,
    TableRow,
    RichText,
    TextArea,
    Svg,
    ThemeProvider,
}

impl ComponentKind {
    pub const ALL: [Self; 39] = [
        Self::Row,
        Self::Column,
        Self::Stack,
        Self::DockArea,
        Self::Pad,
        Self::Spacer,
        Self::Flex,
        Self::Scroll,
        Self::Text,
        Self::DragHandle,
        Self::Button,
        Self::Checkbox,
        Self::Slider,
        Self::TextInput,
        Self::Image,
        Self::Icon,
        Self::ProgressBar,
        Self::LoadingSpinner,
        Self::Switch,
        Self::Radio,
        Self::Divider,
        Self::Link,
        Self::Badge,
        Self::Dialog,
        Self::GroupedTabList,
        Self::TabGroup,
        Self::TabNode,
        Self::TabBar,
        Self::Tab,
        Self::List,
        Self::ListItem,
        Self::Dropdown,
        Self::DropdownOption,
        Self::Table,
        Self::TableRow,
        Self::RichText,
        Self::TextArea,
        Self::Svg,
        Self::ThemeProvider,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Row => "Row",
            Self::Column => "Column",
            Self::Stack => "Stack",
            Self::DockArea => "DockArea",
            Self::Pad => "Pad",
            Self::Spacer => "Spacer",
            Self::Flex => "Flex",
            Self::Scroll => "Scroll",
            Self::Text => "Text",
            Self::DragHandle => "DragHandle",
            Self::Button => "Button",
            Self::Checkbox => "Checkbox",
            Self::Slider => "Slider",
            Self::TextInput => "TextInput",
            Self::Image => "Image",
            Self::Icon => "Icon",
            Self::ProgressBar => "ProgressBar",
            Self::LoadingSpinner => "LoadingSpinner",
            Self::Switch => "Switch",
            Self::Radio => "Radio",
            Self::Divider => "Divider",
            Self::Link => "Link",
            Self::Badge => "Badge",
            Self::Dialog => "Dialog",
            Self::GroupedTabList => "GroupedTabList",
            Self::TabGroup => "TabGroup",
            Self::TabNode => "TabNode",
            Self::TabBar => "TabBar",
            Self::Tab => "Tab",
            Self::List => "List",
            Self::ListItem => "ListItem",
            Self::Dropdown => "Dropdown",
            Self::DropdownOption => "DropdownOption",
            Self::Table => "Table",
            Self::TableRow => "TableRow",
            Self::RichText => "RichText",
            Self::TextArea => "TextArea",
            Self::Svg => "Svg",
            Self::ThemeProvider => "ThemeProvider",
        }
    }
}

/// A semantic target used by scenario drivers on every renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveTarget {
    pub role: Role,
    pub name: Option<String>,
}

impl DriveTarget {
    pub fn new(role: Role, name: impl Into<String>) -> Self {
        Self {
            role,
            name: Some(name.into()),
        }
    }

    pub const fn role(role: Role) -> Self {
        Self { role, name: None }
    }
}

/// Backend-neutral scenario action.
///
/// Retained rendering routes this through its existing AccessKit action path;
/// native HTML locates the same role+name and dispatches a browser event.
#[derive(Clone, Debug, PartialEq)]
pub enum DriveAction {
    Click(DriveTarget),
    SetValue(DriveTarget, String),
    Increment(DriveTarget),
    Decrement(DriveTarget),
}

impl DriveAction {
    pub fn click(role: Role, name: impl Into<String>) -> Self {
        Self::Click(DriveTarget::new(role, name))
    }

    pub fn set_value(role: Role, name: impl Into<String>, value: impl Into<String>) -> Self {
        Self::SetValue(DriveTarget::new(role, name), value.into())
    }
}

pub type TextProducer = Box<dyn FnMut() -> String + 'static>;
pub type ClickHandler = Box<dyn FnMut() + 'static>;
pub type ToggleHandler = Box<dyn FnMut(bool) + 'static>;
pub type ChangeHandler = Box<dyn FnMut(f32) + 'static>;
pub type InputHandler = Box<dyn FnMut(&str) + 'static>;

/// Shared properties used by the extended widget set.
///
/// Keeping this data renderer-neutral is the architectural seam: components and
/// callback ownership live here, while a backend only chooses native primitives.
pub struct ComponentProps {
    pub kind: ComponentKind,
    pub label: Cow<'static, str>,
    pub value: String,
    pub detail: Cow<'static, str>,
    pub items: Vec<Cow<'static, str>>,
    pub number: f32,
    pub min: f32,
    pub max: f32,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub checked: bool,
    pub selected: bool,
    pub open: bool,
    pub disabled: bool,
    pub on_click: Option<ClickHandler>,
    pub on_toggle: Option<ToggleHandler>,
    pub on_change: Option<ChangeHandler>,
    pub on_input: Option<InputHandler>,
}

impl ComponentProps {
    pub fn new(kind: ComponentKind) -> Self {
        Self {
            kind,
            label: Cow::Borrowed(""),
            value: String::new(),
            detail: Cow::Borrowed(""),
            items: Vec::new(),
            number: 0.0,
            min: 0.0,
            max: 100.0,
            width: None,
            height: None,
            checked: false,
            selected: false,
            open: false,
            disabled: false,
            on_click: None,
            on_toggle: None,
            on_change: None,
            on_input: None,
        }
    }
}

/// Visual emphasis for a button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonAppearance {
    #[default]
    Solid,
    Ghost,
}

/// Static or one-shot reactive text. Retained backends may keep the producer and
/// re-evaluate it; snapshot backends evaluate it once.
pub enum TextContent {
    Static(Cow<'static, str>),
    Dynamic(TextProducer),
}

/// Properties for a flex/overlay/scroll container.
#[derive(Clone, Copy, Debug)]
pub struct ContainerProps {
    pub kind: ContainerKind,
    pub justify: Justify,
    pub align: Align,
    pub gap: f32,
    pub wrap: bool,
    pub fill: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub min_width: Option<f32>,
    pub min_height: Option<f32>,
    /// Whether a vertical scrollbar is visible for a scroll container.
    pub scrollbar: bool,
    /// Whether a held pointer at a scroll viewport edge advances its content.
    pub edge_auto_scroll: bool,
}

impl ContainerProps {
    pub fn new(kind: ContainerKind) -> Self {
        Self {
            kind,
            justify: Justify::Start,
            align: Align::Start,
            gap: 0.0,
            wrap: false,
            fill: false,
            width: None,
            height: None,
            min_width: None,
            min_height: None,
            scrollbar: false,
            edge_auto_scroll: false,
        }
    }
}

/// Properties for a text leaf.
pub struct TextProps {
    pub content: TextContent,
    pub size: f32,
    pub role: Role,
    pub wrap: WrapMode,
    pub align: TextAlign,
    pub ellipsis: bool,
}

/// Properties for a native button.
pub struct ButtonProps {
    pub label: Cow<'static, str>,
    pub disabled: bool,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub appearance: ButtonAppearance,
    pub on_click: Option<ClickHandler>,
}

/// Properties for a native checkbox.
pub struct CheckboxProps {
    pub checked: bool,
    pub name: Option<Cow<'static, str>>,
    pub on_toggle: Option<ToggleHandler>,
}

/// Properties for a native range input.
pub struct SliderProps {
    pub value: f32,
    pub min: f32,
    pub max: f32,
    pub step: Option<f32>,
    pub name: Option<Cow<'static, str>>,
    pub disabled: bool,
    pub on_change: Option<ChangeHandler>,
}

/// Properties for a native single-line text input.
pub struct TextInputProps {
    pub value: String,
    pub label: Cow<'static, str>,
    pub on_input: Option<InputHandler>,
    pub password: bool,
}

/// The only interface a renderer implements for the base component set.
///
/// Component defaults and builder behavior live in this crate. A native HTML
/// backend therefore contains DOM/CSS emission only; the retained adapter contains
/// scene-widget construction only.
pub trait TemplateRenderer {
    type Node;

    fn container(&mut self, props: ContainerProps, children: Vec<Self::Node>) -> Self::Node;
    fn pad(&mut self, insets: EdgeInsets, child: Option<Self::Node>) -> Self::Node;
    fn spacer(&mut self) -> Self::Node;
    fn flex(&mut self, props: FlexChild, child: Option<Self::Node>) -> Self::Node;
    fn responsive(&mut self, query: ResponsiveQuery, child: Self::Node) -> Self::Node;
    fn component_ref(&mut self, reference: ComponentRef, child: Self::Node) -> Self::Node;
    fn text(&mut self, props: TextProps) -> Self::Node;
    fn button(&mut self, props: ButtonProps) -> Self::Node;
    fn checkbox(&mut self, props: CheckboxProps) -> Self::Node;
    fn slider(&mut self, props: SliderProps) -> Self::Node;
    fn text_input(&mut self, props: TextInputProps) -> Self::Node;
    fn component(&mut self, props: ComponentProps, children: Vec<Self::Node>) -> Self::Node;
}

/// A component value that can be consumed by any compatible renderer.
pub trait Template: Sized {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node;

    /// Shows this template only while a viewport or immediate-parent query
    /// matches. Renderers preserve the child's semantics and choose their native
    /// mechanism (`display: none` in retained layout, media/container CSS in HTML).
    fn show_when(self, query: ResponsiveQuery) -> Responsive<Self> {
        Responsive { query, child: self }
    }

    /// Attaches a stable reference to this component without changing its layout.
    fn with_ref(self, reference: ComponentRef) -> Referenced<Self> {
        Referenced {
            reference,
            child: self,
        }
    }
}

/// A statically typed child list. Repeated `.child(...)` calls form nested tuples.
pub trait TemplateChildren {
    fn render_children<R: TemplateRenderer>(self, renderer: &mut R, output: &mut Vec<R::Node>);
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl TemplateChildren for () {
    fn render_children<R: TemplateRenderer>(self, _renderer: &mut R, _output: &mut Vec<R::Node>) {}
    fn len(&self) -> usize {
        0
    }
}

impl<C, V> TemplateChildren for (C, V)
where
    C: TemplateChildren,
    V: Template,
{
    fn render_children<R: TemplateRenderer>(self, renderer: &mut R, output: &mut Vec<R::Node>) {
        self.0.render_children(renderer, output);
        output.push(self.1.render(renderer));
    }

    fn len(&self) -> usize {
        self.0.len() + 1
    }
}

/// A renderer-generic row, column, stack, or scroll view.
pub struct Container<C = ()> {
    props: ContainerProps,
    children: C,
}

impl Container<()> {
    pub fn row() -> Self {
        Self::new(ContainerKind::Row)
    }
    pub fn column() -> Self {
        Self::new(ContainerKind::Column)
    }
    pub fn stack() -> Self {
        Self::new(ContainerKind::Stack)
    }
    pub fn scroll() -> Self {
        Self::new(ContainerKind::Scroll)
    }
    fn new(kind: ContainerKind) -> Self {
        Self {
            props: ContainerProps::new(kind),
            children: (),
        }
    }
}

impl<C> Container<C> {
    pub fn child<V: Template>(self, child: V) -> Container<(C, V)> {
        Container {
            props: self.props,
            children: (self.children, child),
        }
    }
    pub fn gap(mut self, gap: f32) -> Self {
        self.props.gap = gap;
        self
    }
    pub fn justify(mut self, justify: Justify) -> Self {
        self.props.justify = justify;
        self
    }
    pub fn align(mut self, align: Align) -> Self {
        self.props.align = align;
        self
    }
    pub fn wrap(mut self) -> Self {
        self.props.wrap = true;
        self
    }
    pub fn fill(mut self) -> Self {
        self.props.fill = true;
        self
    }
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.props.width = Some(width);
        self.props.height = Some(height);
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.props.width = Some(width);
        self
    }
    pub fn height(mut self, height: f32) -> Self {
        self.props.height = Some(height);
        self
    }
    pub fn min_width(mut self, width: f32) -> Self {
        self.props.min_width = Some(width.max(0.0));
        self
    }
    pub fn min_height(mut self, height: f32) -> Self {
        self.props.min_height = Some(height.max(0.0));
        self
    }
    /// Shows or hides the vertical scrollbar on a scroll container.
    ///
    /// This setting is ignored by non-scroll containers. Scrollbars are hidden by
    /// default, preserving the original uncluttered viewport appearance.
    pub fn scrollbar(mut self, visible: bool) -> Self {
        self.props.scrollbar = visible;
        self
    }
    /// Enables or disables scrolling while a held pointer is inside the top or
    /// bottom edge of a scroll viewport.
    ///
    /// This setting is ignored by non-scroll containers and defaults to `false`.
    pub fn edge_auto_scroll(mut self, enabled: bool) -> Self {
        self.props.edge_auto_scroll = enabled;
        self
    }
}

impl<C: TemplateChildren> Template for Container<C> {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let mut children = Vec::with_capacity(self.children.len());
        self.children.render_children(renderer, &mut children);
        renderer.container(self.props, children)
    }
}

/// Convenient base-container constructors.
pub fn row() -> Container {
    Container::row()
}
pub fn column() -> Container {
    Container::column()
}
pub fn stack() -> Container {
    Container::stack()
}
pub fn scroll() -> Container {
    Container::scroll()
}

/// Renderer-generic representation for the widget families beyond the base set.
///
/// Its child type is a nested tuple, just like [`Container`], retaining static
/// composition across both the WGPU adapter and native HTML.
pub struct Widget<C = ()> {
    props: ComponentProps,
    children: C,
}

impl Widget<()> {
    pub fn new(kind: ComponentKind) -> Self {
        Self {
            props: ComponentProps::new(kind),
            children: (),
        }
    }
}

impl<C> Widget<C> {
    pub fn child<V: Template>(self, child: V) -> Widget<(C, V)> {
        Widget {
            props: self.props,
            children: (self.children, child),
        }
    }
    pub fn label(mut self, value: impl Into<Cow<'static, str>>) -> Self {
        self.props.label = value.into();
        self
    }
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.props.value = value.into();
        self
    }
    pub fn detail(mut self, value: impl Into<Cow<'static, str>>) -> Self {
        self.props.detail = value.into();
        self
    }
    pub fn item(mut self, value: impl Into<Cow<'static, str>>) -> Self {
        self.props.items.push(value.into());
        self
    }
    pub fn number(mut self, value: f32) -> Self {
        self.props.number = value;
        self
    }
    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.props.min = min;
        self.props.max = max;
        self
    }
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.props.width = Some(width);
        self.props.height = Some(height);
        self
    }
    pub fn checked(mut self, value: bool) -> Self {
        self.props.checked = value;
        self
    }
    pub fn selected(mut self, value: bool) -> Self {
        self.props.selected = value;
        self
    }
    pub fn open(mut self, value: bool) -> Self {
        self.props.open = value;
        self
    }
    pub fn disabled(mut self, value: bool) -> Self {
        self.props.disabled = value;
        self
    }
    pub fn on_click(mut self, handler: impl FnMut() + 'static) -> Self {
        self.props.on_click = Some(Box::new(handler));
        self
    }
    pub fn on_toggle(mut self, handler: impl FnMut(bool) + 'static) -> Self {
        self.props.on_toggle = Some(Box::new(handler));
        self
    }
    pub fn on_change(mut self, handler: impl FnMut(f32) + 'static) -> Self {
        self.props.on_change = Some(Box::new(handler));
        self
    }
    pub fn on_input(mut self, handler: impl FnMut(&str) + 'static) -> Self {
        self.props.on_input = Some(Box::new(handler));
        self
    }
}

impl<C: TemplateChildren> Template for Widget<C> {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let mut children = Vec::with_capacity(self.children.len());
        self.children.render_children(renderer, &mut children);
        renderer.component(self.props, children)
    }
}

macro_rules! widget_constructor {
    ($name:ident, $kind:ident) => {
        pub fn $name() -> Widget {
            Widget::new(ComponentKind::$kind)
        }
    };
}

widget_constructor!(dock_area, DockArea);
widget_constructor!(drag_handle, DragHandle);
widget_constructor!(image, Image);
widget_constructor!(icon, Icon);
widget_constructor!(progress_bar, ProgressBar);
widget_constructor!(loading_spinner, LoadingSpinner);
widget_constructor!(switch, Switch);
widget_constructor!(radio, Radio);
widget_constructor!(divider, Divider);
widget_constructor!(link, Link);
widget_constructor!(badge, Badge);
widget_constructor!(dialog, Dialog);
widget_constructor!(grouped_tab_list, GroupedTabList);
widget_constructor!(tab_group, TabGroup);
widget_constructor!(tab_node, TabNode);
widget_constructor!(tab_bar, TabBar);
widget_constructor!(tab, Tab);
widget_constructor!(list, List);
widget_constructor!(list_item, ListItem);
widget_constructor!(dropdown, Dropdown);
widget_constructor!(dropdown_option, DropdownOption);
widget_constructor!(table, Table);
widget_constructor!(table_row, TableRow);
widget_constructor!(rich_text, RichText);
widget_constructor!(text_area, TextArea);
widget_constructor!(svg, Svg);
widget_constructor!(theme_provider, ThemeProvider);

/// Type-level absence of a single child.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoChild;

/// Type-level presence of a single child.
pub struct OneChild<V>(V);

/// Internal composition seam used by single-child templates.
pub trait OptionalTemplate {
    fn render_optional<R: TemplateRenderer>(self, renderer: &mut R) -> Option<R::Node>;
}

impl OptionalTemplate for NoChild {
    fn render_optional<R: TemplateRenderer>(self, _renderer: &mut R) -> Option<R::Node> {
        None
    }
}

impl<V: Template> OptionalTemplate for OneChild<V> {
    fn render_optional<R: TemplateRenderer>(self, renderer: &mut R) -> Option<R::Node> {
        Some(self.0.render(renderer))
    }
}

/// A padded, renderer-generic single-child container.
pub struct Pad<C = NoChild> {
    insets: EdgeInsets,
    child: C,
}

impl Pad<NoChild> {
    pub fn all(value: f32) -> Self {
        Self {
            insets: EdgeInsets::all(value),
            child: NoChild,
        }
    }
    pub fn insets(insets: EdgeInsets) -> Self {
        Self {
            insets,
            child: NoChild,
        }
    }
}

impl<C> Pad<C> {
    pub fn child<V: Template>(self, child: V) -> Pad<OneChild<V>> {
        Pad {
            insets: self.insets,
            child: OneChild(child),
        }
    }
}

impl<C: OptionalTemplate> Template for Pad<C> {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let child = self.child.render_optional(renderer);
        renderer.pad(self.insets, child)
    }
}

/// Flexible empty space.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spacer;

impl Spacer {
    pub fn new() -> Self {
        Self
    }
}

impl Template for Spacer {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        renderer.spacer()
    }
}

/// A node-transparent flex wrapper.
pub struct Flex<C = NoChild> {
    props: FlexChild,
    child: C,
}

impl Flex<NoChild> {
    pub fn new() -> Self {
        Self {
            props: FlexChild::default(),
            child: NoChild,
        }
    }
}

impl Default for Flex<NoChild> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> Flex<C> {
    pub fn child<V: Template>(self, child: V) -> Flex<OneChild<V>> {
        Flex {
            props: self.props,
            child: OneChild(child),
        }
    }
    pub fn grow(mut self, value: f32) -> Self {
        self.props.grow = Some(value);
        self
    }
    pub fn shrink(mut self, value: f32) -> Self {
        self.props.shrink = Some(value);
        self
    }
    pub fn basis(mut self, value: f32) -> Self {
        self.props.basis = Some(value);
        self
    }
    pub fn min_width(mut self, value: f32) -> Self {
        self.props.min_width = Some(value.max(0.0));
        self
    }
    pub fn min_height(mut self, value: f32) -> Self {
        self.props.min_height = Some(value.max(0.0));
        self
    }
}

impl<C: OptionalTemplate> Template for Flex<C> {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let child = self.child.render_optional(renderer);
        renderer.flex(self.props, child)
    }
}

/// Renderer-generic, node-transparent responsive visibility wrapper.
pub struct Responsive<V> {
    query: ResponsiveQuery,
    child: V,
}

impl<V: Template> Template for Responsive<V> {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let child = self.child.render(renderer);
        renderer.responsive(self.query, child)
    }
}

/// Renderer-generic, node-transparent component-reference wrapper.
pub struct Referenced<V> {
    reference: ComponentRef,
    child: V,
}

impl<V: Template> Template for Referenced<V> {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        let child = self.child.render(renderer);
        renderer.component_ref(self.reference, child)
    }
}

/// Renderer-generic text.
pub struct Text {
    props: TextProps,
}

impl Text {
    pub fn new(text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            props: TextProps {
                content: TextContent::Static(text.into()),
                size: 16.0,
                role: Role::Label,
                wrap: WrapMode::NoWrap,
                align: TextAlign::Start,
                ellipsis: false,
            },
        }
    }
    pub fn dynamic(producer: impl FnMut() -> String + 'static) -> Self {
        let mut text = Self::new("");
        text.props.content = TextContent::Dynamic(Box::new(producer));
        text
    }
    pub fn size(mut self, size: f32) -> Self {
        self.props.size = size;
        self
    }
    pub fn role(mut self, role: Role) -> Self {
        self.props.role = role;
        self
    }
    pub fn wrap(mut self, wrap: WrapMode) -> Self {
        self.props.wrap = wrap;
        self
    }
    pub fn align(mut self, align: TextAlign) -> Self {
        self.props.align = align;
        self
    }
    pub fn ellipsis(mut self) -> Self {
        self.props.ellipsis = true;
        self
    }
}

impl Template for Text {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        renderer.text(self.props)
    }
}

/// Renderer-generic native button.
pub struct Button {
    props: ButtonProps,
}

impl Button {
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            props: ButtonProps {
                label: label.into(),
                disabled: false,
                width: None,
                height: None,
                appearance: ButtonAppearance::Solid,
                on_click: None,
            },
        }
    }
    pub fn on_click(mut self, handler: impl FnMut() + 'static) -> Self {
        self.props.on_click = Some(Box::new(handler));
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.props.disabled = disabled;
        self
    }
    pub fn width(mut self, width: f32) -> Self {
        self.props.width = Some(width);
        self
    }
    pub fn height(mut self, height: f32) -> Self {
        self.props.height = Some(height);
        self
    }
    pub fn appearance(mut self, appearance: ButtonAppearance) -> Self {
        self.props.appearance = appearance;
        self
    }
}

impl Template for Button {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        renderer.button(self.props)
    }
}

/// Renderer-generic native checkbox.
pub struct Checkbox {
    props: CheckboxProps,
}

impl Checkbox {
    pub fn new(checked: bool) -> Self {
        Self {
            props: CheckboxProps {
                checked,
                name: None,
                on_toggle: None,
            },
        }
    }
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.props.name = Some(name.into());
        self
    }
    pub fn on_toggle(mut self, handler: impl FnMut(bool) + 'static) -> Self {
        self.props.on_toggle = Some(Box::new(handler));
        self
    }
}

impl Template for Checkbox {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        renderer.checkbox(self.props)
    }
}

/// Renderer-generic native range input.
pub struct Slider {
    props: SliderProps,
}

impl Slider {
    pub fn new(value: f32, min: f32, max: f32) -> Self {
        Self {
            props: SliderProps {
                value,
                min,
                max,
                step: None,
                name: None,
                disabled: false,
                on_change: None,
            },
        }
    }
    pub fn step(mut self, step: f32) -> Self {
        self.props.step = Some(step);
        self
    }
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.props.name = Some(name.into());
        self
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.props.disabled = disabled;
        self
    }
    pub fn on_change(mut self, handler: impl FnMut(f32) + 'static) -> Self {
        self.props.on_change = Some(Box::new(handler));
        self
    }
}

impl Template for Slider {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        renderer.slider(self.props)
    }
}

/// Renderer-generic native text input.
pub struct TextInput {
    props: TextInputProps,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            props: TextInputProps {
                value: value.into(),
                label: Cow::Borrowed(""),
                on_input: None,
                password: false,
            },
        }
    }
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.props.label = label.into();
        self
    }
    pub fn placeholder(self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label(label)
    }
    pub fn on_input(mut self, handler: impl FnMut(&str) + 'static) -> Self {
        self.props.on_input = Some(Box::new(handler));
        self
    }
}

impl Template for TextInput {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        renderer.text_input(self.props)
    }
}

/// Renderer-generic native password input.
pub struct PasswordInput {
    props: TextInputProps,
}

impl PasswordInput {
    pub fn new(value: impl Into<String>) -> Self {
        Self {
            props: TextInputProps {
                value: value.into(),
                label: Cow::Borrowed(""),
                on_input: None,
                password: true,
            },
        }
    }
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.props.label = label.into();
        self
    }
    pub fn placeholder(self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label(label)
    }
    pub fn on_input(mut self, handler: impl FnMut(&str) + 'static) -> Self {
        self.props.on_input = Some(Box::new(handler));
        self
    }
}

impl Template for PasswordInput {
    fn render<R: TemplateRenderer>(self, renderer: &mut R) -> R::Node {
        renderer.text_input(self.props)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Tags;

    impl TemplateRenderer for Tags {
        type Node = String;
        fn container(&mut self, props: ContainerProps, children: Vec<String>) -> String {
            format!("{:?}({})", props.kind, children.join(","))
        }
        fn pad(&mut self, _: EdgeInsets, child: Option<String>) -> String {
            format!("Pad({})", child.unwrap_or_default())
        }
        fn spacer(&mut self) -> String {
            "Spacer".into()
        }
        fn flex(&mut self, _: FlexChild, child: Option<String>) -> String {
            format!("Flex({})", child.unwrap_or_default())
        }
        fn responsive(&mut self, _: ResponsiveQuery, child: String) -> String {
            format!("Responsive({child})")
        }
        fn component_ref(&mut self, _: ComponentRef, child: String) -> String {
            format!("Ref({child})")
        }
        fn text(&mut self, mut props: TextProps) -> String {
            match &mut props.content {
                TextContent::Static(value) => value.to_string(),
                TextContent::Dynamic(value) => value(),
            }
        }
        fn button(&mut self, props: ButtonProps) -> String {
            format!("Button({})", props.label)
        }
        fn checkbox(&mut self, props: CheckboxProps) -> String {
            format!("Checkbox({})", props.checked)
        }
        fn slider(&mut self, props: SliderProps) -> String {
            format!("Slider({})", props.value)
        }
        fn text_input(&mut self, props: TextInputProps) -> String {
            if props.password {
                "PasswordInput".into()
            } else {
                format!("Input({})", props.value)
            }
        }
        fn component(&mut self, props: ComponentProps, children: Vec<String>) -> String {
            format!("{}({})", props.kind.as_str(), children.join(","))
        }
    }

    #[test]
    fn one_tree_renders_through_a_generic_backend() {
        let view = column()
            .gap(8.0)
            .child(Text::new("hello"))
            .child(Button::new("continue"));
        assert_eq!(view.render(&mut Tags), "Column(hello,Button(continue))");
    }

    #[test]
    fn scroll_behavior_is_opt_in() {
        let defaults = scroll();
        assert!(!defaults.props.scrollbar);
        assert!(!defaults.props.edge_auto_scroll);

        let configured = scroll().scrollbar(true).edge_auto_scroll(true);
        assert!(configured.props.scrollbar);
        assert!(configured.props.edge_auto_scroll);
    }

    #[test]
    fn password_input_marks_the_renderer_contract_as_protected() {
        assert_eq!(
            PasswordInput::new("secret").render(&mut Tags),
            "PasswordInput"
        );
    }
}
