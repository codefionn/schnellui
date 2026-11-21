//! The renderer that owns the tree and speaks ops.

use std::cell::RefCell;
#[cfg(feature = "render-android-svg")]
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use schnellui::signal::{add_change_listener, remove_change_listener, ChangeListenerId};
use schnellui::template::Template;

use super::diff::{diff_root, emit_full, IdAlloc};
use super::ops::{self, event, kind, prop, Op};
use super::queries;
#[cfg(feature = "render-android-svg")]
use super::svg::SvgCache;
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
    /// The index of the option a Dropdown reports.
    Select(i32),
    Dismiss,
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
            event::SELECT => Some(Event::Select(i)),
            event::DISMISS => Some(Event::Dismiss),
            _ => None,
        }
    }
}

type View = Box<dyn FnMut(&mut AndroidTemplate) -> AndroidNode>;

type Wake = Arc<dyn Fn() + Send + Sync>;

thread_local! {
    /// The wake states whose renderer is running a render, dispatch, resize or
    /// update on this thread. A write made here is already part of that render
    /// and must not wake the host. A write from any other thread still does.
    static BUSY_HERE: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Decides when the host must be told to call `nativeUpdate`.
///
/// The renderer registers a signal change listener while it is alive. A change
/// made while no render, dispatch, resize or update runs on the writing thread
/// calls the wake callback once. The callback stays quiet until the next
/// render starts. A memo that recomputes inside the view changes the global
/// version too, but it does so on the rendering thread, so it stays quiet
/// like any other write made while the renderer is busy.
pub(crate) struct WakeState {
    callback: Mutex<Option<Wake>>,
    /// The callback fired and no render has started since.
    fired: AtomicBool,
}

impl WakeState {
    fn key(&self) -> usize {
        self as *const Self as usize
    }

    fn busy_here(&self) -> bool {
        let key = self.key();
        BUSY_HERE.with(|busy| busy.borrow().contains(&key))
    }

    /// Called by the change listener after a write, on the writing thread.
    /// Fires the callback unless this thread is inside the renderer or the
    /// callback already fired. Returns whether it fired.
    pub(crate) fn notify(&self) -> bool {
        if self.busy_here() {
            return false;
        }
        let callback = match self.callback.lock() {
            Ok(callback) => callback.clone(),
            Err(_) => None,
        };
        let Some(callback) = callback else {
            return false;
        };
        if self.fired.swap(true, Ordering::AcqRel) {
            return false;
        }
        callback();
        true
    }
}

/// Marks the renderer busy on this thread while a render runs, so changes made
/// by the handler or the view itself do not wake the host.
struct Busy(Arc<WakeState>);

impl Busy {
    fn new(state: &Arc<WakeState>) -> Self {
        BUSY_HERE.with(|busy| busy.borrow_mut().push(state.key()));
        Self(state.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        let key = self.0.key();
        BUSY_HERE.with(|busy| {
            let mut busy = busy.borrow_mut();
            if let Some(at) = busy.iter().rposition(|k| *k == key) {
                busy.remove(at);
            }
        });
    }
}

/// Mounts a view closure, keeps the rendered tree and turns changes into op
/// buffers.
pub struct AndroidRenderer {
    view: View,
    viewport: Viewport,
    tree: Option<AndroidNode>,
    ids: IdAlloc,
    #[cfg(feature = "render-android-svg")]
    svg: Rc<RefCell<SvgCache>>,
    wake: Arc<WakeState>,
    listener: ChangeListenerId,
}

impl Drop for AndroidRenderer {
    fn drop(&mut self) {
        remove_change_listener(self.listener);
        if let Ok(mut callback) = self.wake.callback.lock() {
            *callback = None;
        }
    }
}

impl AndroidRenderer {
    /// Mounts `view`. The closure runs once per render, so it reads its
    /// signals each time.
    pub fn new<V: Template + 'static>(
        viewport: Viewport,
        mut view: impl FnMut() -> V + 'static,
    ) -> Self {
        let wake = Arc::new(WakeState {
            callback: Mutex::new(None),
            fired: AtomicBool::new(false),
        });
        let listener = {
            let wake = Arc::downgrade(&wake);
            add_change_listener(move || {
                if let Some(wake) = wake.upgrade() {
                    wake.notify();
                }
            })
        };
        Self {
            view: Box::new(move |template| view().render(template)),
            viewport,
            tree: None,
            ids: IdAlloc(0),
            #[cfg(feature = "render-android-svg")]
            svg: Rc::default(),
            wake,
            listener,
        }
    }

    /// Sets the callback that asks the host for an update. It runs on any
    /// thread, when a signal changed outside `dispatch`, `resize`, `update`
    /// and `render`. It fires once and stays quiet until the next render.
    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        if let Ok(mut callback) = self.wake.callback.lock() {
            *callback = Some(Arc::new(wake));
        }
    }

    /// How many SVGs were rasterised so far. Tests use it to see the cache work.
    #[cfg(all(test, feature = "render-android-svg"))]
    pub(crate) fn svg_rasterisations(&self) -> usize {
        self.svg.borrow().rasterisations
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Renders again, diffs against the previous tree and keeps the result.
    fn rerender(&mut self) -> Vec<Op> {
        let _busy = Busy::new(&self.wake);
        // Re-arm before the view runs: a change made by another thread while it
        // runs is newer than this render and wakes the host again.
        self.wake.fired.store(false, Ordering::Release);
        #[cfg(feature = "render-android-svg")]
        let mut template = AndroidTemplate::with_cache(self.viewport, self.svg.clone());
        #[cfg(not(feature = "render-android-svg"))]
        let mut template = AndroidTemplate::new(self.viewport);
        let mut new = (self.view)(&mut template);
        let mut ops = Vec::new();
        new.prune_options();
        diff_root(self.tree.as_ref(), &mut new, &mut self.ids, &mut ops);
        queries::resolve(self.tree.as_ref(), &mut new, &mut ops);
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
        // The handler's signal changes are rendered below, so they must not wake.
        let busy = Busy::new(&self.wake);
        let target = self.tree.as_mut()?.find_mut(node)?;
        match event {
            Event::Click => {
                let handler = target.handlers.click.as_mut()?;
                if target.kind == kind::RADIO {
                    target.props.insert(prop::CHECKED, Value::I32(1));
                }
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
            Event::Select(index) => {
                let handler = target.handlers.select.as_mut()?;
                let value = target.option_values.get(usize::try_from(index).ok()?)?;
                target.props.insert(prop::SELECTED_INDEX, Value::I32(index));
                handler(value);
            }
            Event::Dismiss => {
                let handler = target.handlers.dismiss.as_mut()?;
                handler();
            }
        }
        let result = self.finish();
        drop(busy);
        result
    }

    /// Renders again after a signal changed outside the host's calls and
    /// returns the diff, or `None` when nothing changed. This is what
    /// `nativeUpdate` runs.
    pub fn update(&mut self) -> Option<Vec<u8>> {
        let _busy = Busy::new(&self.wake);
        self.finish()
    }

    /// Sets a new viewport size and returns the diff, if anything changed.
    pub fn resize(&mut self, width: f32, height: f32) -> Option<Vec<u8>> {
        if self.viewport.width == width && self.viewport.height == height {
            return None;
        }
        self.viewport.width = width;
        self.viewport.height = height;
        let _busy = Busy::new(&self.wake);
        self.finish()
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        let ops = self.rerender();
        (!ops.is_empty()).then(|| ops::encode(&ops))
    }
}
