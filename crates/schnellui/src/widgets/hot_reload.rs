//! Static template replacement. Code and event closures remain compiled Rust.
use crate::widgets::*;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};

#[derive(Clone, Copy, Debug)]
pub struct Template {
    pub id: &'static str,
    pub nodes: &'static [TemplateNode],
}

impl Template {
    /// Reads static content from the hoisted skeleton during initial construction.
    pub fn text(&self, path: &[u32]) -> &'static str {
        self.nodes
            .iter()
            .find(|node| node.path == path)
            .and_then(|node| node.text)
            .expect("compiled static text site")
    }
}
#[derive(Clone, Copy, Debug)]
pub struct TemplateNode {
    pub path: &'static [u32],
    pub tag: &'static str,
    pub text: Option<&'static str>,
    pub attrs: &'static [TemplateAttr],
    pub dynamic: &'static str,
    pub inert: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct TemplateAttr {
    pub name: &'static str,
    pub value: &'static str,
    pub dynamic: bool,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TemplateBlueprint {
    pub id: String,
    pub nodes: Vec<BlueprintNode>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintNode {
    pub path: Vec<u32>,
    pub tag: String,
    pub text: Option<String>,
    pub attrs: Vec<BlueprintAttr>,
    pub dynamic: String,
    pub inert: bool,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintAttr {
    pub name: String,
    pub value: String,
    pub dynamic: bool,
}
impl From<&Template> for TemplateBlueprint {
    fn from(template: &Template) -> Self {
        Self {
            id: template.id.into(),
            nodes: template
                .nodes
                .iter()
                .map(|node| BlueprintNode {
                    path: node.path.to_vec(),
                    tag: node.tag.into(),
                    text: node.text.map(str::to_owned),
                    dynamic: node.dynamic.into(),
                    inert: node.inert,
                    attrs: node
                        .attrs
                        .iter()
                        .map(|attr| BlueprintAttr {
                            name: attr.name.into(),
                            value: attr.value.into(),
                            dynamic: attr.dynamic,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}
#[derive(Default, Clone)]
pub(crate) struct TemplateRegistry(Rc<RefCell<Vec<Rc<RefCell<MountedTemplate>>>>>);
struct MountedTemplate {
    blueprint: TemplateBlueprint,
    ids: HashMap<Vec<u32>, WidgetId>,
}
#[derive(Clone)]
struct MountScope(Rc<RefCell<MountedTemplate>>);

/// A macro's typed builder plus its hoisted immutable template skeleton.
pub struct TemplateView<V> {
    view: V,
    template: &'static Template,
}
impl<V: View> TemplateView<V> {
    pub fn new(view: V, template: &'static Template) -> Self {
        Self { view, template }
    }
    pub fn template(&self) -> &'static Template {
        self.template
    }
}
impl<V> std::ops::Deref for TemplateView<V> {
    type Target = V;
    fn deref(&self) -> &V {
        &self.view
    }
}
impl<V> std::ops::DerefMut for TemplateView<V> {
    fn deref_mut(&mut self) -> &mut V {
        &mut self.view
    }
}
impl<V: View> View for TemplateView<V> {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let mounted = Rc::new(RefCell::new(MountedTemplate {
            blueprint: self.template.into(),
            ids: HashMap::new(),
        }));
        let previous = ctx.context.clone();
        ctx.context = previous.with(MountScope(mounted.clone()));
        let id = Box::new(self.view).build(ctx, parent);
        ctx.context = previous;
        ctx.runtime.templates.0.borrow_mut().push(mounted);
        id
    }
}
/// Registers the exact retained root of a source node without adding scene nodes.
pub struct TemplateNodeView<V> {
    view: V,
    path: &'static [u32],
}
impl<V: View> TemplateNodeView<V> {
    pub fn new(view: V, path: &'static [u32]) -> Self {
        Self { view, path }
    }
}
impl<V> std::ops::Deref for TemplateNodeView<V> {
    type Target = V;
    fn deref(&self) -> &V {
        &self.view
    }
}
impl<V> std::ops::DerefMut for TemplateNodeView<V> {
    fn deref_mut(&mut self) -> &mut V {
        &mut self.view
    }
}
impl<V: View> View for TemplateNodeView<V> {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let id = Box::new(self.view).build(ctx, parent);
        if let Some(mount) = ctx.context.get::<MountScope>() {
            mount.0.borrow_mut().ids.insert(self.path.to_vec(), id);
        }
        id
    }
}
pub(crate) fn reset(runtime: &Runtime) {
    runtime.templates.0.borrow_mut().clear();
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadError {
    UnknownTemplate(String),
    RebuildRequired(String),
    Invalid(String),
}
impl std::fmt::Display for ReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTemplate(s) => write!(f, "unknown template {s}"),
            Self::RebuildRequired(s) => write!(f, "rebuild required: {s}"),
            Self::Invalid(s) => write!(f, "invalid template: {s}"),
        }
    }
}
impl std::error::Error for ReloadError {}
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    pub mounts: usize,
    pub changed: usize,
    pub inserted: usize,
    pub removed: usize,
}
fn fingerprint(node: &BlueprintNode) -> String {
    let mut key = format!("{}|{}", node.tag, node.dynamic);
    for attr in node.attrs.iter().filter(|attr| attr.dynamic) {
        key.push_str(&format!("|{}={}", attr.name, attr.value));
    }
    key
}
fn has_code(node: &BlueprintNode) -> bool {
    !node.dynamic.is_empty() || node.attrs.iter().any(|a| a.dynamic)
}
struct Plan {
    ids: HashMap<Vec<u32>, WidgetId>,
    attrs: Vec<(Vec<u32>, Attribute, AttributeValue)>,
}
fn plan(mount: &MountedTemplate, next: &TemplateBlueprint) -> Result<Plan, ReloadError> {
    let invalid = |s: &str| ReloadError::Invalid(s.into());
    let rebuild = |s: &str| ReloadError::RebuildRequired(s.into());
    let mut unique = HashSet::new();
    let mut child_counts: HashMap<Vec<u32>, u32> = HashMap::new();
    let mut previous_path: Option<&[u32]> = None;
    for node in &next.nodes {
        if previous_path.is_some_and(|previous| previous >= node.path.as_slice()) {
            return Err(invalid("nodes must be in source preorder"));
        }
        previous_path = Some(&node.path);
        if node.path.len() > 1 {
            let parent = node.path[..node.path.len() - 1].to_vec();
            let count = child_counts.entry(parent).or_default();
            if node.path.last() != Some(count) {
                return Err(invalid("child indices must be contiguous"));
            }
            *count += 1;
        }
        if node.path.is_empty() || !unique.insert(node.path.clone()) {
            return Err(invalid("empty or duplicate node path"));
        }
        if node.path.len() > 1 && !unique.contains(&node.path[..node.path.len() - 1]) {
            return Err(invalid("missing parent node"));
        }
    }
    if next.nodes.first().map(|n| n.path.as_slice()) != Some(&[0]) {
        return Err(invalid("root must have path [0]"));
    }
    if next.nodes.iter().filter(|n| n.path.len() == 1).count() != 1 {
        return Err(invalid("exactly one root required"));
    }
    if mount.blueprint.nodes.first().map(|n| &n.tag) != next.nodes.first().map(|n| &n.tag) {
        return Err(rebuild("root widget type changed"));
    }
    let old_code: Vec<_> = mount
        .blueprint
        .nodes
        .iter()
        .filter(|n| has_code(n))
        .collect();
    let new_code: Vec<_> = next.nodes.iter().filter(|n| has_code(n)).collect();
    if old_code.len() != new_code.len()
        || old_code
            .iter()
            .zip(&new_code)
            .any(|(a, b)| fingerprint(a) != fingerprint(b))
    {
        return Err(rebuild("dynamic expression or event binding changed"));
    }
    let mut ids = HashMap::new();
    // Preserve dynamic nodes and their ancestors even when static siblings shift.
    for (old, new) in old_code.iter().zip(new_code) {
        if old.path.len() != new.path.len() {
            return Err(rebuild("dynamic node moved between containers"));
        }
        for len in 1..=old.path.len() {
            let old_path = &old.path[..len];
            let new_path = &new.path[..len];
            let a = mount
                .blueprint
                .nodes
                .iter()
                .find(|n| n.path == old_path)
                .ok_or_else(|| invalid("old parent missing"))?;
            let b = next
                .nodes
                .iter()
                .find(|n| n.path == new_path)
                .ok_or_else(|| invalid("new parent missing"))?;
            if a.tag != b.tag {
                return Err(rebuild("dynamic ancestor type changed"));
            }
            let id = *mount
                .ids
                .get(old_path)
                .ok_or_else(|| rebuild("node is not independently addressable"))?;
            if ids
                .insert(new_path.to_vec(), id)
                .is_some_and(|existing| existing != id)
            {
                return Err(rebuild("dynamic nodes changed parent ownership"));
            }
        }
    }
    for node in &next.nodes {
        if ids.contains_key(&node.path) {
            continue;
        }
        if let Some(old) = mount
            .blueprint
            .nodes
            .iter()
            .find(|old| old.path == node.path && old.tag == node.tag)
        {
            if let Some(id) = mount.ids.get(&old.path) {
                let same_parent = node.path.len() == 1
                    || ids.get(&node.path[..node.path.len() - 1])
                        == mount.ids.get(&old.path[..old.path.len() - 1]);
                if same_parent && !ids.values().any(|used| used == id) {
                    ids.insert(node.path.clone(), *id);
                }
            }
        }
        if !ids.contains_key(&node.path)
            && !matches!(
                node.tag.as_str(),
                "text" | "column" | "row" | "stack" | "spacer"
            )
        {
            return Err(rebuild("new static widget type is not supported"));
        }
    }
    let mut attrs = Vec::new();
    for node in &next.nodes {
        let old = ids
            .get(&node.path)
            .and_then(|id| mount.ids.iter().find(|(_, old_id)| old_id == &id))
            .and_then(|(path, _)| mount.blueprint.nodes.iter().find(|n| n.path == *path));
        if old.is_some_and(|old| {
            old.attrs
                .iter()
                .any(|a| !a.dynamic && !node.attrs.iter().any(|b| b.name == a.name))
        }) {
            return Err(rebuild("removing an attribute needs its compiled default"));
        }
        for attr in node.attrs.iter().filter(|a| !a.dynamic) {
            if old.is_some_and(|old| old.attrs.iter().any(|a| a == attr)) {
                continue;
            }
            if (node.tag == "text" && attr.name == "size")
                || (node.tag == "button" && attr.name == "text_size")
            {
                crate::widgets::attributes::parse_static_attribute("width", &attr.value)
                    .map_err(ReloadError::RebuildRequired)?;
                continue;
            }
            let (kind, value) =
                crate::widgets::attributes::parse_static_attribute(&attr.name, &attr.value)
                    .map_err(ReloadError::RebuildRequired)?;
            attrs.push((node.path.clone(), kind, value));
        }
        if old.is_some_and(|old| old.text != node.text)
            && !matches!(node.tag.as_str(), "text" | "button")
        {
            return Err(rebuild("text content on this widget is not supported"));
        }
    }
    Ok(Plan { ids, attrs })
}
/// Validates every live instance before applying a static template replacement.
/// Dynamic closures, retained widget ids, input buffers and signal subscriptions survive.
pub fn reload_template(
    ctx: &mut BuildCtx,
    next: TemplateBlueprint,
) -> Result<ReloadReport, ReloadError> {
    ctx.runtime.templates.0.borrow_mut().retain(|mount| {
        mount
            .borrow()
            .ids
            .get(&vec![0])
            .is_some_and(|id| ctx.scene.node(*id).is_some())
    });
    let mounts: Vec<_> = ctx
        .runtime
        .templates
        .0
        .borrow()
        .iter()
        .filter(|mount| mount.borrow().blueprint.id == next.id)
        .cloned()
        .collect();
    if mounts.is_empty() {
        return Err(ReloadError::UnknownTemplate(next.id));
    }
    if mounts.iter().any(|mount| {
        mount
            .borrow()
            .ids
            .values()
            .any(|id| ctx.scene.node(*id).is_none())
    }) {
        return Err(ReloadError::RebuildRequired(
            "mounted template lost source nodes".into(),
        ));
    }
    let plans: Vec<_> = mounts
        .iter()
        .map(|mount| plan(&mount.borrow(), &next))
        .collect::<Result<_, _>>()?;
    let mut report = ReloadReport::default();
    for (mount, mut plan) in mounts.into_iter().zip(plans) {
        let mut mount = mount.borrow_mut();
        let retained: HashSet<_> = plan.ids.values().copied().collect();
        for node in &next.nodes {
            if plan.ids.contains_key(&node.path) {
                continue;
            }
            let parent = if node.path.len() == 1 {
                None
            } else {
                plan.ids.get(&node.path[..node.path.len() - 1]).copied()
            };
            let view: Box<dyn View> = match node.tag.as_str() {
                "text" => Box::new(Text::new(node.text.clone().unwrap_or_default())),
                "column" => Box::new(Column::new()),
                "row" => Box::new(Row::new()),
                "stack" => Box::new(Stack::new()),
                _ => Box::new(Spacer::new()),
            };
            let id = view.build(ctx, parent);
            ctx.scene.mark_dirty(id, DirtyFlags::LAYOUT);
            if let Some(parent) = parent {
                ctx.scene.mark_dirty(parent, DirtyFlags::LAYOUT);
            }
            plan.ids.insert(node.path.clone(), id);
            report.inserted += 1;
        }
        for (path, id) in &mount.ids {
            if retained.contains(id) {
                continue;
            }
            if mount.ids.iter().any(|(ancestor, ancestor_id)| {
                ancestor.len() < path.len()
                    && path.starts_with(ancestor)
                    && !retained.contains(ancestor_id)
            }) {
                continue;
            }
            let removed_ids = ctx.scene.subtree_nodes(*id);
            purge_nodes(&ctx.runtime, ctx.scene, &removed_ids);
            if let Some(removed) = ctx.scene.remove_subtree(*id) {
                ctx.layout.remove_nodes(&removed.nodes);
                if let Some(parent) = removed.parent {
                    ctx.scene.mark_dirty(parent, DirtyFlags::LAYOUT);
                }
                report.removed += removed.nodes.len();
            }
        }
        for node in &next.nodes {
            let id = plan.ids[&node.path];
            let old = mount
                .ids
                .iter()
                .find(|(_, old_id)| **old_id == id)
                .and_then(|(path, _)| mount.blueprint.nodes.iter().find(|old| old.path == *path));
            if (old.is_none()
                || old.is_some_and(|old| {
                    old.text != node.text
                        || old
                            .attrs
                            .iter()
                            .find(|a| a.name == "size" || a.name == "text_size")
                            != node
                                .attrs
                                .iter()
                                .find(|a| a.name == "size" || a.name == "text_size")
                }))
                && matches!(node.tag.as_str(), "text" | "button")
            {
                {
                    let empty = String::new();
                    let text = node.text.as_ref().unwrap_or(&empty);
                    let role = ctx
                        .scene
                        .a11y(id)
                        .map(|a| a.role)
                        .unwrap_or(Role::Label.as_u16());
                    let a = ctx.scene.a11y_mut(id);
                    if role == Role::Status.as_u16() {
                        a.value = Some(text.clone());
                    } else {
                        a.name = Some(text.clone());
                    }
                    let wrapped = ctx.runtime.with(|rt| {
                        let mut rt = rt.borrow_mut();
                        if let Some(layout) = rt.text_layouts.get_mut(id) {
                            layout.text.clone_from(text);
                            if let Some(size) = node
                                .attrs
                                .iter()
                                .find(|a| a.name == "size" && !a.dynamic)
                                .and_then(|a| a.value.trim_end_matches("f32").parse::<f32>().ok())
                            {
                                layout.size_px = size;
                            }
                            layout.text_changed();
                            true
                        } else {
                            false
                        }
                    });
                    if !wrapped {
                        let number = |name: &str, default: f32| {
                            node.attrs
                                .iter()
                                .find(|a| a.name == name)
                                .and_then(|a| a.value.trim_end_matches("f32").parse::<f32>().ok())
                                .unwrap_or(default)
                        };
                        let optional = |name: &str| {
                            node.attrs
                                .iter()
                                .find(|a| a.name == name)
                                .and_then(|a| a.value.trim_end_matches("f32").parse::<f32>().ok())
                        };
                        let size = if node.tag == "button" {
                            let appearance = ctx
                                .runtime
                                .with(|rt| rt.borrow().button_appearances.get(id).copied())
                                .unwrap_or_default();
                            let disabled = ctx
                                .scene
                                .a11y(id)
                                .is_some_and(|a| a.state & StateFlags::DISABLED.0 != 0);
                            let ts = emit_button_paint(
                                &ctx.runtime,
                                ctx.scene,
                                ctx.text,
                                ctx.atlas,
                                id,
                                text,
                                !node.attrs.iter().any(|a| a.name == "icon_only"),
                                disabled,
                                optional("width"),
                                optional("height"),
                                appearance,
                                optional("text_size"),
                                ctx.scale,
                            );
                            sized_button_intrinsic(
                                &ctx.runtime,
                                id,
                                ts,
                                optional("width"),
                                optional("height"),
                                appearance,
                            )
                        } else {
                            emit_text_paint(
                                ctx.scene,
                                ctx.text,
                                ctx.atlas,
                                id,
                                text,
                                crate::widgets::attributes::current_font_size(&ctx.runtime, id)
                                    .unwrap_or_else(|| number("size", 16.0)),
                                theme_for(&ctx.runtime, id).text,
                                ctx.scale,
                            )
                        };
                        ctx.layout.set_measure(id, Box::new(move |_| size));
                    }
                    crate::widgets::attributes::reapply_retained_font(ctx, id);
                    ctx.scene.mark_dirty(id, DirtyFlags::LAYOUT);
                    ctx.scene.mark_dirty(id, DirtyFlags::PAINT);
                    ctx.scene.mark_dirty(id, DirtyFlags::A11Y);
                    report.changed += 1;
                }
            }
        }
        for (path, kind, value) in plan.attrs {
            crate::widgets::attributes::apply(
                &ctx.runtime,
                ctx.scene,
                ctx.layout,
                plan.ids[&path],
                kind,
                value,
            );
            report.changed += 1;
        }
        for node in &next.nodes {
            let children: Vec<_> = next
                .nodes
                .iter()
                .filter(|child| {
                    child.path.len() == node.path.len() + 1 && child.path.starts_with(&node.path)
                })
                .map(|child| plan.ids[&child.path])
                .collect();
            if !children.is_empty() {
                let parent = plan.ids[&node.path];
                if ctx.scene.set_child_order(parent, &children) {
                    ctx.scene.mark_dirty(parent, DirtyFlags::LAYOUT);
                }
            }
        }
        mount.blueprint = next.clone();
        mount.ids = plan.ids;
        report.mounts += 1;
    }
    reapply_reactive_colors(&ctx.runtime, ctx.scene);
    Ok(report)
}
/// Nonblocking TCP dev endpoint. One newline-delimited JSON blueprint per request.
/// Bind to loopback; this protocol intentionally cannot execute Rust code.
///
/// The first line on a connection must be a blueprint: anything else — plain
/// HTTP from a web page included — drops the connection before further lines
/// are read, so a request body cannot smuggle a blueprint past the line that
/// fails to parse.
pub struct HotReloadServer {
    listener: TcpListener,
    /// Stream, buffered bytes, and whether a blueprint was already accepted on
    /// this connection.
    clients: Vec<(TcpStream, Vec<u8>, bool)>,
}
impl HotReloadServer {
    /// Binds the endpoint. The protocol is unauthenticated, so a non-loopback
    /// address is refused: anyone who can connect can rewrite the running UI.
    pub fn bind(address: impl ToSocketAddrs) -> std::io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        if !listener.local_addr()?.ip().is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "hot reload binds only to loopback addresses",
            ));
        }
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            clients: Vec::new(),
        })
    }
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
    pub fn poll(&mut self, ctx: &mut BuildCtx) -> Vec<Result<ReloadReport, ReloadError>> {
        for _ in 0..16 {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(true).is_ok() {
                        self.clients.push((stream, Vec::new(), false));
                    }
                }
                Err(_) => break,
            }
        }
        let mut results = Vec::new();
        self.clients.retain_mut(|(stream, buffer, accepted)| {
            let mut bytes = [0; 4096];
            let mut closed = false;
            for _ in 0..256 {
                match stream.read(&mut bytes) {
                    Ok(0) => {
                        closed = true;
                        break;
                    }
                    Ok(n) => buffer.extend_from_slice(&bytes[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => {
                        closed = true;
                        break;
                    }
                }
                if buffer.len() > 1024 * 1024 {
                    closed = true;
                    break;
                }
            }
            while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
                let result = serde_json::from_slice::<TemplateBlueprint>(&buffer[..end])
                    .map_err(|e| ReloadError::Invalid(e.to_string()))
                    .and_then(|next| reload_template(ctx, next));
                // The first line must be a blueprint. Anything else — a plain
                // HTTP request reaching this port, say — drops the connection
                // before its body lines can be interpreted.
                if !*accepted && result.is_err() {
                    closed = true;
                    break;
                }
                *accepted = true;
                let response = match &result {
                    Ok(report) => format!(
                        "ok {} {} {} {}\n",
                        report.mounts, report.changed, report.inserted, report.removed
                    ),
                    Err(error) => format!("error {error}\n"),
                };
                let _ = stream.write_all(response.as_bytes());
                results.push(result);
                buffer.drain(..=end);
            }
            !closed
        });
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_reload_refuses_non_loopback_addresses() {
        let error = HotReloadServer::bind("0.0.0.0:0")
            .err()
            .expect("wildcard bind refused");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(HotReloadServer::bind("127.0.0.1:0").is_ok());
    }
    static TEMPLATE: Template = Template {
        id: "test.rs:1:1",
        nodes: &[
            TemplateNode {
                path: &[0],
                tag: "column",
                text: None,
                attrs: &[],
                dynamic: "",
                inert: false,
            },
            TemplateNode {
                path: &[0, 0],
                tag: "text",
                text: Some("heading"),
                attrs: &[],
                dynamic: "",
                inert: true,
            },
            TemplateNode {
                path: &[0, 1],
                tag: "text",
                text: None,
                attrs: &[],
                dynamic: "count",
                inert: false,
            },
        ],
    };
    fn setup() -> (
        Runtime,
        Scene,
        LayoutEngine,
        TextShaper,
        GlyphAtlas,
        schnellui::signal::Signal<i32>,
        WidgetId,
        WidgetId,
    ) {
        let runtime = Runtime::new();
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let count = schnellui::signal::create_signal(7i32);
        let view = TemplateView::new(
            TemplateNodeView::new(
                Column::new()
                    .child(TemplateNodeView::new(
                        Text::new(TEMPLATE.text(&[0, 0])),
                        &[0, 0],
                    ))
                    .child(TemplateNodeView::new(
                        Text::dynamic(move || count.get().to_string()),
                        &[0, 1],
                    )),
                &[0],
            ),
            &TEMPLATE,
        );
        let root = Box::new(view).build(
            &mut BuildCtx {
                context: Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            },
            None,
        );
        scene.set_root(root);
        let dynamic = scene.node(root).unwrap().children[1];
        (runtime, scene, layout, text, atlas, count, root, dynamic)
    }
    #[test]
    fn static_insert_text_edit_and_remove_preserve_dynamic_id_and_subscription() {
        let (runtime, mut scene, mut layout, mut text, mut atlas, count, root, dynamic) = setup();
        layout.sync_tree(&scene, root);
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 400.0,
                height: 300.0,
            },
        );
        scene.clear_dirty();
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[1].text = Some("edited".into());
        next.nodes[2].path = vec![0, 2];
        let mut inserted = next.nodes[1].clone();
        inserted.path = vec![0, 1];
        inserted.text = Some("inserted".into());
        next.nodes.insert(2, inserted);
        let report = reload_template(
            &mut BuildCtx {
                context: Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            },
            next.clone(),
        )
        .unwrap();
        assert_eq!(report.inserted, 1);
        assert!(scene.layout_dirty().contains(&root));
        layout.sync_dirty_nodes(&scene, scene.layout_dirty());
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 400.0,
                height: 300.0,
            },
        );
        let inserted = scene.node(root).unwrap().children[1];
        assert!(scene.layout(inserted).unwrap().rect.height > 0.0);
        let dynamic_y = scene.layout(dynamic).unwrap().rect.y;
        scene.clear_dirty();
        assert_eq!(scene.node(root).unwrap().children[2], dynamic);
        assert_eq!(
            scene
                .a11y(scene.node(root).unwrap().children[0])
                .unwrap()
                .name
                .as_deref(),
            Some("edited")
        );
        count.set(9);
        schnellui::signal::Runtime::flush();
        run_dynamic_slots(&runtime, &mut scene, &mut text, &mut atlas);
        assert_eq!(scene.a11y(dynamic).unwrap().name.as_deref(), Some("9"));
        next.nodes.remove(2);
        next.nodes[2].path = vec![0, 1];
        let report = reload_template(
            &mut BuildCtx {
                context: Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            },
            next,
        )
        .unwrap();
        assert_eq!(report.removed, 1);
        assert!(scene.layout_dirty().contains(&root));
        layout.sync_dirty_nodes(&scene, scene.layout_dirty());
        layout.compute(
            &mut scene,
            root,
            Size {
                width: 400.0,
                height: 300.0,
            },
        );
        assert!(scene.layout(dynamic).unwrap().rect.y < dynamic_y);
        assert_eq!(scene.node(root).unwrap().children[1], dynamic);
    }
    #[test]
    fn dynamic_code_and_unsupported_attribute_rejections_are_atomic() {
        let (runtime, mut scene, mut layout, mut text, mut atlas, _count, root, dynamic) = setup();
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[1].text = Some("edited".into());
        next.nodes[2].dynamic = "count + 1".into();
        let mut ctx = BuildCtx {
            context: Context::new(),
            runtime: runtime.clone(),
            scene: &mut scene,
            layout: &mut layout,
            text: &mut text,
            atlas: &mut atlas,
            scale: 1.0,
        };
        assert!(matches!(
            reload_template(&mut ctx, next),
            Err(ReloadError::RebuildRequired(_))
        ));
        assert_eq!(
            ctx.scene
                .a11y(ctx.scene.node(root).unwrap().children[0])
                .unwrap()
                .name
                .as_deref(),
            Some("heading")
        );
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[0].attrs.push(BlueprintAttr {
            name: "imaginary".into(),
            value: "1".into(),
            dynamic: false,
        });
        assert!(reload_template(&mut ctx, next).is_err());
        assert!(ctx.scene.node(dynamic).is_some());
    }
    #[test]
    fn socket_applies_blueprint_to_mounted_view() {
        let (runtime, mut scene, mut layout, mut text, mut atlas, _count, root, _dynamic) = setup();
        let mut server = HotReloadServer::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(server.local_addr().unwrap()).unwrap();
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[1].text = Some("socket edit".into());
        serde_json::to_writer(&mut client, &next).unwrap();
        client.write_all(b"\n").unwrap();
        let mut results = Vec::new();
        for _ in 0..100 {
            results.extend(server.poll(&mut BuildCtx {
                context: Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            }));
            if !results.is_empty() {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
        assert_eq!(
            scene
                .a11y(scene.node(root).unwrap().children[0])
                .unwrap()
                .name
                .as_deref(),
            Some("socket edit")
        );
    }

    #[test]
    fn socket_drops_a_connection_whose_first_line_is_not_a_blueprint() {
        let (runtime, mut scene, mut layout, mut text, mut atlas, _count, root, _dynamic) = setup();
        let mut server = HotReloadServer::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(server.local_addr().unwrap()).unwrap();
        // A browser-shaped request with a blueprint smuggled into its body must
        // never reach the template.
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[1].text = Some("smuggled".into());
        let mut request = b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".to_vec();
        serde_json::to_writer(&mut request, &next).unwrap();
        request.extend_from_slice(b"\n");
        client.write_all(&request).unwrap();
        let mut results = Vec::new();
        for _ in 0..100 {
            results.extend(server.poll(&mut BuildCtx {
                context: Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            }));
            if !results.is_empty() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(results.is_empty());
        assert_ne!(
            scene
                .a11y(scene.node(root).unwrap().children[0])
                .unwrap()
                .name
                .as_deref(),
            Some("smuggled")
        );
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;
    #[test]
    fn static_text_reload_keeps_size_wrapping_and_changes_static_gap() {
        static TEMPLATE: Template = Template {
            id: "style",
            nodes: &[
                TemplateNode {
                    path: &[0],
                    tag: "column",
                    text: None,
                    attrs: &[TemplateAttr {
                        name: "gap",
                        value: "8.0",
                        dynamic: false,
                    }],
                    dynamic: "",
                    inert: true,
                },
                TemplateNode {
                    path: &[0, 0],
                    tag: "text",
                    text: Some("old"),
                    attrs: &[TemplateAttr {
                        name: "size",
                        value: "28.0",
                        dynamic: false,
                    }],
                    dynamic: "",
                    inert: true,
                },
                TemplateNode {
                    path: &[0, 1],
                    tag: "text",
                    text: Some("wrapped"),
                    attrs: &[TemplateAttr {
                        name: "wrap",
                        value: "word",
                        dynamic: true,
                    }],
                    dynamic: "",
                    inert: false,
                },
            ],
        };
        let runtime = Runtime::new();
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let mut ctx = BuildCtx {
            context: Context::new(),
            runtime: runtime.clone(),
            scene: &mut scene,
            layout: &mut layout,
            text: &mut text,
            atlas: &mut atlas,
            scale: 1.0,
        };
        let root = Box::new(TemplateView::new(
            TemplateNodeView::new(
                Column::new()
                    .gap(8.0)
                    .child(TemplateNodeView::new(Text::new("old").size(28.0), &[0, 0]))
                    .child(TemplateNodeView::new(
                        Text::new("wrapped").size(23.0).wrap(WrapMode::Word),
                        &[0, 1],
                    )),
                &[0],
            ),
            &TEMPLATE,
        ))
        .build(&mut ctx, None);
        ctx.scene.set_root(root);
        let plain = ctx.scene.node(root).unwrap().children[0];
        let wrapped = ctx.scene.node(root).unwrap().children[1];
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[0].attrs[0].value = "19.0".into();
        next.nodes[1].text = Some("new text".into());
        next.nodes[2].text = Some("new wrapped text".into());
        reload_template(&mut ctx, next).unwrap();
        assert_eq!(ctx.layout.container_style(root).unwrap().gap, 19.0);
        let expected_height = ctx.text.shape("new text", 28.0, None).height;
        let painted = ctx.scene.paint(plain).unwrap();
        assert!(!painted.primitives.is_empty());
        ctx.layout.sync_tree(ctx.scene, root);
        let record = runtime.with(|rt| {
            let rt = rt.borrow();
            let entry = rt.text_layouts.get(wrapped).unwrap();
            (entry.text.clone(), entry.size_px, entry.wrap)
        });
        assert_eq!(record, ("new wrapped text".into(), 23.0, WrapMode::Word));
        ctx.layout.compute(
            ctx.scene,
            root,
            Size {
                width: 400.0,
                height: 300.0,
            },
        );
        assert!((ctx.scene.layout(plain).unwrap().rect.height - expected_height).abs() < 0.01);
    }
    #[test]
    fn button_label_reload_keeps_chrome_and_handler() {
        static TEMPLATE: Template = Template {
            id: "button",
            nodes: &[TemplateNode {
                path: &[0],
                tag: "button",
                text: Some("old"),
                attrs: &[TemplateAttr {
                    name: "on:click",
                    value: "click",
                    dynamic: true,
                }],
                dynamic: "",
                inert: false,
            }],
        };
        let runtime = Runtime::new();
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let calls = Rc::new(std::cell::Cell::new(0));
        let clicked = calls.clone();
        let mut ctx = BuildCtx {
            context: Context::new(),
            runtime: runtime.clone(),
            scene: &mut scene,
            layout: &mut layout,
            text: &mut text,
            atlas: &mut atlas,
            scale: 1.0,
        };
        let id = Box::new(TemplateView::new(
            TemplateNodeView::new(
                Button::new("old").on_click(move || clicked.set(clicked.get() + 1)),
                &[0],
            ),
            &TEMPLATE,
        ))
        .build(&mut ctx, None);
        ctx.scene.set_root(id);
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[0].text = Some("new label".into());
        reload_template(&mut ctx, next).unwrap();
        assert!(matches!(
            ctx.scene.paint(id).unwrap().primitives.first(),
            Some(Primitive::SolidRect { .. })
        ));
        assert_eq!(
            ctx.scene.a11y(id).unwrap().name.as_deref(),
            Some("new label")
        );
        assert!(dispatch_click(&runtime, ctx.scene, id));
        assert_eq!(calls.get(), 1);
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    #[test]
    fn malformed_paths_reject_before_text_mutation() {
        static TEMPLATE: Template = Template {
            id: "malformed",
            nodes: &[TemplateNode {
                path: &[0],
                tag: "text",
                text: Some("old"),
                attrs: &[],
                dynamic: "",
                inert: true,
            }],
        };
        let runtime = Runtime::new();
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let mut ctx = BuildCtx {
            context: Context::new(),
            runtime: runtime.clone(),
            scene: &mut scene,
            layout: &mut layout,
            text: &mut text,
            atlas: &mut atlas,
            scale: 1.0,
        };
        let root = Box::new(TemplateView::new(
            TemplateNodeView::new(Text::new("old"), &[0]),
            &TEMPLATE,
        ))
        .build(&mut ctx, None);
        ctx.scene.set_root(root);
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[0].text = Some("new".into());
        let mut extra = next.nodes[0].clone();
        extra.path = vec![0, 3];
        next.nodes.push(extra);
        assert!(matches!(
            reload_template(&mut ctx, next),
            Err(ReloadError::Invalid(_))
        ));
        assert_eq!(ctx.scene.a11y(root).unwrap().name.as_deref(), Some("old"));
        let mut server = HotReloadServer::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(server.local_addr().unwrap()).unwrap();
        let mut next = TemplateBlueprint::from(&TEMPLATE);
        next.nodes[0].text = Some("via fragments".into());
        let message = serde_json::to_vec(&next).unwrap();
        let split = message.len() / 2;
        client.write_all(&message[..split]).unwrap();
        assert!(server.poll(&mut ctx).is_empty());
        client.write_all(&message[split..]).unwrap();
        client.write_all(b"\n{broken}\n").unwrap();
        let mut results = Vec::new();
        for _ in 0..1000 {
            results.extend(server.poll(&mut ctx));
            if results.len() == 2 {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(results.len(), 2);
        assert!(results[0].is_ok());
        assert!(matches!(results[1], Err(ReloadError::Invalid(_))));
        assert_eq!(
            ctx.scene.a11y(root).unwrap().name.as_deref(),
            Some("via fragments")
        );
    }
}

#[cfg(test)]
mod replacement_tests {
    use super::*;
    #[test]
    fn replacing_static_parent_rebuilds_its_descendants() {
        let node = |path: Vec<u32>, tag: &str| BlueprintNode {
            path,
            tag: tag.into(),
            text: None,
            attrs: vec![],
            dynamic: String::new(),
            inert: true,
        };
        let old = TemplateBlueprint {
            id: "replace".into(),
            nodes: vec![
                node(vec![0], "column"),
                node(vec![0, 0], "column"),
                node(vec![0, 0, 0], "text"),
            ],
        };
        let mut scene = Scene::new();
        let root = scene.insert(WidgetKind::Column, None);
        let container = scene.insert(WidgetKind::Column, Some(root));
        let text = scene.insert(WidgetKind::Text, Some(container));
        let mount = MountedTemplate {
            blueprint: old.clone(),
            ids: HashMap::from([
                (vec![0], root),
                (vec![0, 0], container),
                (vec![0, 0, 0], text),
            ]),
        };
        let mut next = old;
        next.nodes[1].tag = "row".into();
        let plan = plan(&mount, &next).unwrap();
        assert_eq!(plan.ids.len(), 1);
        assert_eq!(plan.ids[&vec![0]], root);
    }
}
