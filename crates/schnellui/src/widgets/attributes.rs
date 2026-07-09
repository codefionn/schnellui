//! Tracked property producers attached to existing retained nodes.
use super::*;
use schnellui::signal::{Subscription, SubscriptionGroup};
use slotmap::{Key, KeyData, SlotMap};

/// Properties supported by `view!` closure attributes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attribute {
    Color,
    Width,
    Height,
    MinWidth,
    MinHeight,
    MaxWidth,
    MaxHeight,
    Grow,
    Shrink,
    Basis,
    Gap,
    Padding,
    Disabled,
    Checked,
    FontSize,
}

/// Typed values crossing the macro's retained property boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AttributeValue {
    Number(f32),
    Bool(bool),
    Color(Color),
}
impl From<f32> for AttributeValue {
    fn from(v: f32) -> Self {
        Self::Number(v)
    }
}
impl From<f64> for AttributeValue {
    fn from(v: f64) -> Self {
        Self::Number(v as f32)
    }
}
impl From<i32> for AttributeValue {
    fn from(v: i32) -> Self {
        Self::Number(v as f32)
    }
}
impl From<u32> for AttributeValue {
    fn from(v: u32) -> Self {
        Self::Number(v as f32)
    }
}
impl From<bool> for AttributeValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}
impl From<Color> for AttributeValue {
    fn from(v: Color) -> Self {
        Self::Color(v)
    }
}

/// Node-transparent wrapper holding a property producer until mount.
pub struct ReactiveAttr {
    child: AnyView,
    attribute: Attribute,
    producer: Box<dyn FnMut() -> AttributeValue>,
}
impl ReactiveAttr {
    pub fn new<T: Into<AttributeValue> + 'static>(
        child: impl View,
        mut producer: impl FnMut() -> T + 'static,
        attribute: Attribute,
    ) -> Self {
        Self {
            child: Box::new(child),
            attribute,
            producer: Box::new(move || producer().into()),
        }
    }
}
impl View for ReactiveAttr {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let Self {
            child,
            attribute,
            mut producer,
        } = *self;
        let id = child.build(ctx, parent);
        let (key, subscription) = ctx.runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let bindings = &mut rt.attributes;
            let group = &bindings.group;
            let key = bindings.entries.insert_with_key(|key| Entry {
                node: id,
                attribute,
                producer: None,
                last: None,
                subscription: group.subscribe(key.data().as_ffi()),
            });
            (key, bindings.entries[key].subscription.clone())
        });
        let value = subscription.track(|| producer());
        if attribute == Attribute::FontSize {
            prepare_font(&ctx.runtime, ctx.scene, ctx.layout, id, ctx.scale);
            apply_font(
                &ctx.runtime,
                ctx.scene,
                ctx.layout,
                ctx.text,
                ctx.atlas,
                id,
                value,
            );
        } else {
            apply(&ctx.runtime, ctx.scene, ctx.layout, id, attribute, value);
        }
        ctx.runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let e = &mut rt.attributes.entries[key];
            e.producer = Some(producer);
            e.last = Some(value);
            if attribute == Attribute::Color {
                rt.attributes.colors.insert(id, value);
            }
        });
        id
    }
}
slotmap::new_key_type! { struct AttrKey; }
struct Entry {
    node: WidgetId,
    attribute: Attribute,
    producer: Option<Box<dyn FnMut() -> AttributeValue>>,
    last: Option<AttributeValue>,
    subscription: Subscription,
}
/// All attribute subscriptions for one widget runtime.
pub(crate) struct AttributeBindings {
    group: SubscriptionGroup,
    entries: SlotMap<AttrKey, Entry>,
    ready: Vec<u64>,
    colors: SecondaryMap<WidgetId, AttributeValue>,
    fonts: SecondaryMap<WidgetId, FontRecord>,
}
impl Default for AttributeBindings {
    fn default() -> Self {
        Self {
            group: SubscriptionGroup::new(),
            entries: SlotMap::with_key(),
            ready: Vec::new(),
            colors: SecondaryMap::new(),
            fonts: SecondaryMap::new(),
        }
    }
}
impl AttributeBindings {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.colors.clear();
        self.fonts.clear();
        self.ready.clear();
        self.group.clear();
    }
    pub(crate) fn forget(&mut self, id: WidgetId) {
        self.entries.retain(|_, e| e.node != id);
        self.colors.remove(id);
        self.fonts.remove(id);
    }
}

/// Drains only changed properties. Producers run with no widget registry borrow.
pub fn run_reactive_attributes(
    runtime: &Runtime,
    scene: &mut Scene,
    layout: &mut LayoutEngine,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
) {
    let mut ready = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let b = &mut rt.attributes;
        let mut ready = std::mem::take(&mut b.ready);
        b.group.drain_ready_into(&mut ready);
        ready
    });
    for raw in &ready {
        let key = AttrKey::from(KeyData::from_ffi(*raw));
        let taken = runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let e = rt.attributes.entries.get_mut(key)?;
            Some((
                e.node,
                e.attribute,
                e.subscription.clone(),
                e.last,
                e.producer.take()?,
            ))
        });
        let Some((id, attr, subscription, last, mut producer)) = taken else {
            continue;
        };
        if scene.node(id).is_none() {
            runtime.with(|rt| rt.borrow_mut().attributes.forget(id));
            continue;
        }
        let value = subscription.track(|| producer());
        if last != Some(value) {
            if attr == Attribute::FontSize {
                apply_font(runtime, scene, layout, shaper, atlas, id, value);
            } else {
                apply(runtime, scene, layout, id, attr, value);
            }
        }
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            if let Some(e) = rt.attributes.entries.get_mut(key) {
                e.producer = Some(producer);
                e.last = Some(value);
            }
            if attr == Attribute::Color {
                rt.attributes.colors.insert(id, value);
            }
        });
    }
    ready.clear();
    runtime.with(|rt| rt.borrow_mut().attributes.ready = ready);
}
/// Restores cached colors after text or interaction code re-emits primitives.
pub fn reapply_reactive_colors(runtime: &Runtime, scene: &mut Scene) {
    let mut index = 0;
    while index < scene.paint_dirty().len() {
        let id = scene.paint_dirty()[index];
        index += 1;
        let color = runtime.with(|rt| rt.borrow().attributes.colors.get(id).copied());
        if let Some(AttributeValue::Color(color)) = color {
            scene.set_color(id, color);
        }
    }
}
pub(crate) fn apply(
    runtime: &Runtime,
    scene: &mut Scene,
    layout: &mut LayoutEngine,
    id: WidgetId,
    attr: Attribute,
    value: AttributeValue,
) {
    match (attr, value) {
        (Attribute::Color, AttributeValue::Color(color)) => scene.set_color(id, color),
        (Attribute::Disabled | Attribute::Checked, AttributeValue::Bool(enabled)) => {
            let bit = if attr == Attribute::Disabled {
                StateFlags::DISABLED.0
            } else {
                StateFlags::CHECKED.0
            };
            let state = scene.a11y(id).map(|a| a.state).unwrap_or_default();
            scene.set_a11y_state(id, if enabled { state | bit } else { state & !bit });
            if attr == Attribute::Disabled {
                apply_disabled_paint(runtime, scene, id, enabled);
            }
            if attr == Attribute::Checked {
                emit_checkbox_paint(runtime, scene, id, enabled);
                scene.mark_dirty(id, DirtyFlags::PAINT);
            }
        }
        (attr, AttributeValue::Number(number)) => {
            if attr == Attribute::Gap {
                if let Some(mut style) = layout.container_style(id) {
                    style.gap = number;
                    layout.set_container(id, style);
                }
            } else {
                let mut flex = layout.flex(id);
                match attr {
                    Attribute::Padding => flex.padding = Some(EdgeInsets::all(number)),
                    Attribute::Width => flex.width = Some(number),
                    Attribute::Height => flex.height = Some(number),
                    Attribute::MinWidth => flex.min_width = Some(number),
                    Attribute::MinHeight => flex.min_height = Some(number),
                    Attribute::MaxWidth => flex.max_width = Some(number),
                    Attribute::MaxHeight => flex.max_height = Some(number),
                    Attribute::Grow => flex.grow = Some(number),
                    Attribute::Shrink => flex.shrink = Some(number),
                    Attribute::Basis => flex.basis = Some(number),
                    _ => panic!("unsupported numeric reactive property"),
                }
                layout.set_flex(id, flex);
            }
            scene.mark_dirty(id, DirtyFlags::LAYOUT);
        }
        _ => panic!("reactive property value has the wrong type"),
    }
}

fn apply_disabled_paint(runtime: &Runtime, scene: &mut Scene, id: WidgetId, disabled: bool) {
    let appearance = runtime.with(|rt| rt.borrow().button_appearances.get(id).copied());
    let Some(appearance) = appearance else {
        return;
    };
    let theme = theme_for(runtime, id);
    let solid = appearance == ButtonAppearance::Solid;
    let index = usize::from(solid && theme.shape.shadow > 0.0)
        + usize::from(solid && theme.shape.frame > 0.0);
    let color = if disabled {
        theme.disabled
    } else if solid {
        theme.accent
    } else {
        Color::TRANSPARENT
    };
    if let Some(Primitive::SolidRect { color: current, .. }) =
        scene.paint_mut(id).primitives.get_mut(index)
    {
        if *current != color {
            *current = color;
            scene.mark_dirty(id, DirtyFlags::PAINT);
        }
    }
}

/// Parses the supported scalar properties before a hot reload changes any nodes.
pub(crate) fn parse_static_attribute(
    name: &str,
    value: &str,
) -> Result<(Attribute, AttributeValue), String> {
    let attr = match name {
        "color" => Attribute::Color,
        "width" => Attribute::Width,
        "height" => Attribute::Height,
        "min_width" => Attribute::MinWidth,
        "min_height" => Attribute::MinHeight,
        "max_width" => Attribute::MaxWidth,
        "max_height" => Attribute::MaxHeight,
        "grow" => Attribute::Grow,
        "shrink" => Attribute::Shrink,
        "basis" => Attribute::Basis,
        "gap" => Attribute::Gap,
        "padding" | "all" => Attribute::Padding,
        "disabled" => Attribute::Disabled,
        "checked" => Attribute::Checked,
        _ => return Err(format!("property `{name}` requires a rebuild")),
    };
    let value = match attr {
        Attribute::Disabled | Attribute::Checked => AttributeValue::Bool(
            value
                .trim()
                .parse()
                .map_err(|_| format!("invalid boolean `{value}`"))?,
        ),
        Attribute::Color => {
            let normalized = value.replace(' ', "");
            let color = match normalized.as_str() {
                "Color::BLACK" | "schnellui::Color::BLACK" => Color::BLACK,
                "Color::WHITE" | "schnellui::Color::WHITE" => Color::WHITE,
                "Color::TRANSPARENT" => Color::TRANSPARENT,
                _ => return Err(format!("color expression `{value}` requires a rebuild")),
            };
            AttributeValue::Color(color)
        }
        _ => {
            let value = value.trim().trim_end_matches("f32").trim_end_matches("f64");
            let number: f32 = value
                .parse()
                .map_err(|_| format!("invalid numeric property `{value}`"))?;
            if !number.is_finite() {
                return Err(format!("non-finite property `{value}`"));
            }
            AttributeValue::Number(number)
        }
    };
    Ok((attr, value))
}

struct FontRecord {
    text: String,
    shared: Rc<RefCell<Size>>,
    scale: f32,
}
fn prepare_font(
    runtime: &Runtime,
    scene: &Scene,
    layout: &mut LayoutEngine,
    id: WidgetId,
    scale: f32,
) {
    let wrapped = runtime.with(|rt| rt.borrow().text_layouts.contains_key(id));
    if wrapped {
        return;
    }
    let shared = runtime
        .with(|rt| rt.borrow().slots.get(id).map(|slot| slot.shared.clone()))
        .unwrap_or_else(|| Rc::new(RefCell::new(Size::default())));
    let measure = shared.clone();
    layout.set_measure(id, Box::new(move |_| *measure.borrow()));
    let text = scene
        .a11y(id)
        .and_then(|a| a.name.as_ref().or(a.value.as_ref()))
        .cloned()
        .unwrap_or_default();
    runtime.with(|rt| {
        rt.borrow_mut().attributes.fonts.insert(
            id,
            FontRecord {
                text,
                shared,
                scale,
            },
        );
    });
}
fn apply_font(
    runtime: &Runtime,
    scene: &mut Scene,
    layout: &mut LayoutEngine,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    value: AttributeValue,
) {
    let AttributeValue::Number(size) = value else {
        panic!("font size requires a number");
    };
    let wrapped = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        if let Some(slot) = rt.slots.get_mut(id) {
            slot.size_px = size;
        }
        if let Some(text) = rt.text_layouts.get_mut(id) {
            text.size_px = size;
            text.text_changed();
            true
        } else {
            false
        }
    });
    if !wrapped {
        let record = runtime.with(|rt| rt.borrow_mut().attributes.fonts.remove(id));
        let Some(mut record) = record else {
            return;
        };
        if let Some(content) = scene
            .a11y(id)
            .and_then(|a| a.name.as_ref().or(a.value.as_ref()))
        {
            record.text.clear();
            record.text.push_str(content);
        }
        let size = emit_text_paint(
            scene,
            shaper,
            atlas,
            id,
            &record.text,
            size,
            theme_for(runtime, id).text,
            record.scale,
        );
        *record.shared.borrow_mut() = size;
        reposition_node(runtime, scene, id);
        runtime.with(|rt| {
            rt.borrow_mut().attributes.fonts.insert(id, record);
        });
    }
    scene.mark_dirty(id, DirtyFlags::LAYOUT);
    scene.mark_dirty(id, DirtyFlags::PAINT);
    let _ = layout;
}

/// Returns a retained font binding's last value for static-label hot reloads.
pub(crate) fn current_font_size(runtime: &Runtime, id: WidgetId) -> Option<f32> {
    runtime.with(|rt| {
        rt.borrow().attributes.entries.values().find_map(|entry| {
            if entry.node == id && entry.attribute == Attribute::FontSize {
                match entry.last {
                    Some(AttributeValue::Number(size)) => Some(size),
                    _ => None,
                }
            } else {
                None
            }
        })
    })
}

/// Restores a bound font after an in-place static text edit, without evaluating
/// the producer. Reloads may replace the intrinsic measure, so restore its cell.
pub(crate) fn reapply_retained_font(ctx: &mut BuildCtx, id: WidgetId) {
    let Some(size) = current_font_size(&ctx.runtime, id) else {
        return;
    };
    let shared = ctx.runtime.with(|rt| {
        rt.borrow()
            .attributes
            .fonts
            .get(id)
            .map(|record| record.shared.clone())
    });
    if let Some(shared) = shared {
        ctx.layout
            .set_measure(id, Box::new(move |_| *shared.borrow()));
    }
    apply_font(
        &ctx.runtime,
        ctx.scene,
        ctx.layout,
        ctx.text,
        ctx.atlas,
        id,
        AttributeValue::Number(size),
    );
}
