//! The renderer that owns the tree and speaks ops.

use schnellui::template::Template;

use super::diff::{diff_root, emit_full, IdAlloc};
use super::ops::{self, event, prop, Op};
use super::template::AndroidTemplate;
use super::tree::{AndroidNode, Value};

/// The host's window size and display state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Width in dp.
    pub width: f32,
    /// Height in dp.
    pub height: f32,
    pub density: f32,
    /// Whether the system is in dark mode.
    pub dark: bool,
}

impl Viewport {
    pub fn new(width: f32, height: f32, density: f32, dark: bool) -> Self {
        Self {
            width,
            height,
            density,
            dark,
        }
    }
}

/// A view event reported by the host.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Click,
    Toggle(bool),
    Change(f32),
    Input(String),
}

impl Event {
    /// Builds an event from the JNI arguments. `None` for an unknown code, or
    /// for an INPUT without a string.
    pub fn from_raw(code: i32, i: i32, f: f32, s: Option<String>) -> Option<Event> {
        match u8::try_from(code).ok()? {
            event::CLICK => Some(Event::Click),
            event::TOGGLE => Some(Event::Toggle(i != 0)),
            event::CHANGE => Some(Event::Change(f)),
            event::INPUT => s.map(Event::Input),
            _ => None,
        }
    }
}

type View = Box<dyn FnMut(&mut AndroidTemplate) -> AndroidNode>;

/// Mounts a view closure, keeps the rendered tree and turns changes into op
/// buffers.
pub struct AndroidRenderer {
    view: View,
    viewport: Viewport,
    tree: Option<AndroidNode>,
    ids: IdAlloc,
}

impl AndroidRenderer {
    /// Mounts `view`. The closure runs once per render, so it reads its
    /// signals each time.
    pub fn new<V: Template + 'static>(
        viewport: Viewport,
        mut view: impl FnMut() -> V + 'static,
    ) -> Self {
        Self {
            view: Box::new(move |template| view().render(template)),
            viewport,
            tree: None,
            ids: IdAlloc(0),
        }
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Renders again, diffs against the previous tree and keeps the result.
    fn rerender(&mut self) -> Vec<Op> {
        let mut template = AndroidTemplate::new(self.viewport);
        let mut new = (self.view)(&mut template);
        let mut ops = Vec::new();
        diff_root(self.tree.as_ref(), &mut new, &mut self.ids, &mut ops);
        self.tree = Some(new);
        ops
    }

    /// A buffer that builds the whole tree from nothing and ends with SET_ROOT.
    /// Node ids carry over from the previous render.
    pub fn render(&mut self) -> Vec<u8> {
        self.rerender();
        let mut ops = Vec::new();
        if let Some(tree) = &self.tree {
            emit_full(tree, &mut ops);
        }
        ops::encode(&ops)
    }

    /// Records the value the host reports, runs the handler, renders again and
    /// returns the diff. `None` when the node is unknown, has no handler for
    /// this event, or nothing changed.
    pub fn dispatch(&mut self, node: u32, event: Event) -> Option<Vec<u8>> {
        let target = self.tree.as_mut()?.find_mut(node)?;
        match event {
            Event::Click => {
                let handler = target.handlers.click.as_mut()?;
                handler();
            }
            Event::Toggle(on) => {
                let handler = target.handlers.toggle.as_mut()?;
                target.props.insert(prop::CHECKED, Value::I32(on as i32));
                handler(on);
            }
            Event::Change(value) => {
                let handler = target.handlers.change.as_mut()?;
                target.props.insert(prop::VALUE, Value::F32(value));
                handler(value);
            }
            Event::Input(text) => {
                let handler = target.handlers.input.as_mut()?;
                target.props.insert(prop::TEXT, Value::Str(text.clone()));
                handler(&text);
            }
        }
        self.finish()
    }

    /// Sets a new viewport size and returns the diff, if anything changed.
    pub fn resize(&mut self, width: f32, height: f32) -> Option<Vec<u8>> {
        if self.viewport.width == width && self.viewport.height == height {
            return None;
        }
        self.viewport.width = width;
        self.viewport.height = height;
        self.finish()
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        let ops = self.rerender();
        (!ops.is_empty()).then(|| ops::encode(&ops))
    }
}
