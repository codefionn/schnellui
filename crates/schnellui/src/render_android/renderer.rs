//! The renderer that owns the tree and speaks ops.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use schnellui::signal::Runtime;
use schnellui::template::Template;

use super::diff::{diff_root, emit_full, IdAlloc};
use super::ops::{self, event, kind, prop, Op};
use super::queries;
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

/// Decides when the host must be told to call `nativeUpdate`.
///
/// The signal runtime has no hook that fires when a signal is set. It does keep
/// a global version that every change bumps. The renderer records the version
/// it last rendered at, and a change beyond that, made while no dispatch,
/// resize, update or render is running, calls the wake callback once. The
/// callback stays quiet until the next render.
pub(crate) struct WakeState {
    callback: Mutex<Option<Wake>>,
    /// The signal version the last render saw.
    seen: AtomicU64,
    /// How many of render, dispatch, resize and update are running.
    busy: AtomicUsize,
    /// The callback fired and no render has happened since.
    fired: AtomicBool,
    polling: AtomicBool,
}

impl WakeState {
    /// Fires the callback if a signal changed since the last render. Safe to
    /// call from any thread. Returns whether it fired.
    pub(crate) fn check(&self) -> bool {
        if self.busy.load(Ordering::Acquire) > 0 {
            return false;
        }
        if Runtime::global_version() == self.seen.load(Ordering::Acquire) {
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

/// Marks the renderer busy while a render runs, so changes made by the handler
/// or the view itself do not wake the host.
struct Busy(Arc<WakeState>);

impl Busy {
    fn new(state: &Arc<WakeState>) -> Self {
        state.busy.fetch_add(1, Ordering::AcqRel);
        Self(state.clone())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.busy.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Mounts a view closure, keeps the rendered tree and turns changes into op
/// buffers.
pub struct AndroidRenderer {
    view: View,
    viewport: Viewport,
    tree: Option<AndroidNode>,
    ids: IdAlloc,
    svg: Rc<RefCell<SvgCache>>,
    wake: Arc<WakeState>,
}

impl Drop for AndroidRenderer {
    fn drop(&mut self) {
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
        Self {
            view: Box::new(move |template| view().render(template)),
            viewport,
            tree: None,
            ids: IdAlloc(0),
            svg: Rc::default(),
            wake: Arc::new(WakeState {
                callback: Mutex::new(None),
                seen: AtomicU64::new(Runtime::global_version()),
                busy: AtomicUsize::new(0),
                fired: AtomicBool::new(false),
                polling: AtomicBool::new(false),
            }),
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

    /// Checks for a signal change and fires the wake callback if there is one.
    /// The polling thread calls this. Returns whether it fired.
    pub fn check_wake(&self) -> bool {
        self.wake.check()
    }

    /// Starts a thread that calls [`check_wake`] every `interval`. It stops
    /// when the renderer is dropped. Calling this twice starts one thread.
    ///
    /// [`check_wake`]: Self::check_wake
    pub fn start_wake_polling(&self, interval: Duration) {
        if self.wake.polling.swap(true, Ordering::AcqRel) {
            return;
        }
        let state: Weak<WakeState> = Arc::downgrade(&self.wake);
        let spawned = std::thread::Builder::new()
            .name("schnellui-wake".into())
            .spawn(move || {
                while let Some(state) = state.upgrade() {
                    state.check();
                    drop(state);
                    std::thread::sleep(interval);
                }
            });
        if spawned.is_err() {
            self.wake.polling.store(false, Ordering::Release);
        }
    }

    #[cfg(test)]
    pub(crate) fn wake_state(&self) -> Arc<WakeState> {
        self.wake.clone()
    }

    /// How many SVGs were rasterised so far. Tests use it to see the cache work.
    #[cfg(test)]
    pub(crate) fn svg_rasterisations(&self) -> usize {
        self.svg.borrow().rasterisations
    }

    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Renders again, diffs against the previous tree and keeps the result.
    fn rerender(&mut self) -> Vec<Op> {
        let _busy = Busy::new(&self.wake);
        // Read before the view runs: a change made by another thread while it
        // runs is newer than this render and wakes the host again.
        let version = Runtime::global_version();
        let mut template = AndroidTemplate::with_cache(self.viewport, self.svg.clone());
        let mut new = (self.view)(&mut template);
        let mut ops = Vec::new();
        new.prune_options();
        diff_root(self.tree.as_ref(), &mut new, &mut self.ids, &mut ops);
        queries::resolve(self.tree.as_ref(), &mut new, &mut ops);
        self.tree = Some(new);
        self.wake.seen.store(version, Ordering::Release);
        self.wake.fired.store(false, Ordering::Release);
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
