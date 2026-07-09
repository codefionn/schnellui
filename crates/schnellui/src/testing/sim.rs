//! Deterministic simulation of a running app (SOUL §7.3, §7.5).
//!
//! A [`Simulator`] owns an [`App`] and a virtual clock. It replays what the
//! native host does between frames: input runs its handler and then one frame,
//! a running animation gets a frame every `frame_interval`, and an idle app
//! sleeps until its next trailing scroll deadline. Nothing reads the wall
//! clock, so a run depends only on its inputs and the time steps it takes.
//!
//! [`SimRng`] and [`for_each_seed`] turn one scenario into many randomized
//! runs. A failing run reports its seed, and `SCHNELLUI_SIM_SEED=<n>` replays
//! exactly that run. [`SimHost`] lets the seed pick the host conditions too:
//! viewport, scale factor, frame rate and how late frames arrive.

use std::fmt;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::time::{Duration, Instant};

use schnellui::a11y::Role;
use schnellui::a11y::{A11yNodeDump, A11yTreeDump};
use schnellui::accesskit_action::{Action, ActionData, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::{ComponentRef, Point, Primitive, WidgetId};
use schnellui::widgets::View;
use schnellui::{App, TestValue, ThemeMode, UiKey};

use crate::testing::find_by_role_name;

/// The native host's animation pacing (`windowed::ANIMATION_REDRAW_INTERVAL`).
pub const DEFAULT_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// A simulation step that could not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimError {
    /// No visible widget has this role and name.
    NotFound { role: Role, name: Option<String> },
    /// The app still wanted frames when the time limit ran out.
    NeverIdle { limit: Duration },
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimError::NotFound { role, name } => {
                write!(f, "no visible widget with role {role:?} and name {name:?}")
            }
            SimError::NeverIdle { limit } => {
                write!(f, "app still animating after {limit:?}")
            }
        }
    }
}

impl std::error::Error for SimError {}

/// An [`App`] driven on a virtual clock.
pub struct Simulator {
    app: App,
    start: Instant,
    elapsed: Duration,
    frame_interval: Duration,
    /// Virtual time of the last frame, which paces the next animation frame.
    last_frame: Duration,
    frames: u64,
    /// Upper bound on how late an animation frame arrives, and the generator
    /// that picks each delay.
    jitter: Duration,
    jitter_rng: SimRng,
    /// How late the next animation frame arrives.
    next_jitter: Duration,
}

impl Simulator {
    /// Takes over `app`, pins its clock at virtual time zero and runs the first
    /// frame, like a host presenting a freshly mounted window.
    pub fn new(app: App) -> Simulator {
        let mut sim = Simulator {
            app,
            start: Instant::now(),
            elapsed: Duration::ZERO,
            frame_interval: DEFAULT_FRAME_INTERVAL,
            last_frame: Duration::ZERO,
            frames: 0,
            jitter: Duration::ZERO,
            jitter_rng: SimRng::new(0),
            next_jitter: Duration::ZERO,
        };
        sim.frame();
        sim
    }

    /// Mounts `root` at a fixed viewport and starts simulating it.
    pub fn mount(root: impl View, width: u32, height: u32) -> Simulator {
        Simulator::new(App::mount_with_size(root, width, height))
    }

    pub fn app(&self) -> &App {
        &self.app
    }

    /// Direct app access for setup the simulator has no verb for. Run
    /// [`Simulator::frame`] afterwards if the change should settle.
    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    pub fn into_app(self) -> App {
        self.app
    }

    /// Virtual time since the simulation started.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Virtual time of the most recent frame.
    pub fn last_frame_at(&self) -> Duration {
        self.last_frame
    }

    /// Frames run so far, including the first one.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Sets the animation frame rate. The host default is about 60 Hz.
    pub fn set_frame_rate(&mut self, hz: u32) {
        assert!(hz > 0, "frame rate must be positive");
        self.frame_interval = Duration::from_secs(1) / hz;
    }

    pub fn frame_interval(&self) -> Duration {
        self.frame_interval
    }

    /// Makes each animation frame arrive up to `max` after its slot, like a
    /// host that misses vsync or gets scheduled late. The delays come from
    /// `seed`, so a run still replays exactly. Zero turns jitter off.
    pub fn set_frame_jitter(&mut self, max: Duration, seed: u64) {
        self.jitter = max;
        self.jitter_rng = SimRng::new(seed);
        self.next_jitter = self.draw_jitter();
    }

    fn draw_jitter(&mut self) -> Duration {
        if self.jitter.is_zero() {
            return Duration::ZERO;
        }
        let max = self.jitter.as_micros() as u64;
        Duration::from_micros(self.jitter_rng.below(max + 1))
    }

    fn instant(&self) -> Instant {
        self.start + self.elapsed
    }

    /// Runs one frame at the current virtual time without advancing it.
    pub fn frame(&mut self) {
        self.app.frame_at(self.instant());
        self.last_frame = self.elapsed;
        self.frames += 1;
        self.next_jitter = self.draw_jitter();
    }

    /// Whether the app wants no frame and has no pending scroll callback.
    pub fn is_idle(&self) -> bool {
        !self.app.wants_animation_frame() && self.app.next_scroll_callback_deadline().is_none()
    }

    /// The virtual time the host would wake at next, if any.
    fn next_wake(&self) -> Option<Duration> {
        let animation = self
            .app
            .wants_animation_frame()
            .then(|| (self.last_frame + self.frame_interval + self.next_jitter).max(self.elapsed));
        let scroll = self.app.next_scroll_callback_deadline().map(|deadline| {
            deadline
                .saturating_duration_since(self.start)
                .max(self.elapsed)
        });
        match (animation, scroll) {
            (Some(a), Some(s)) => Some(a.min(s)),
            (a, s) => a.or(s),
        }
    }

    /// Advances virtual time by `duration`, running every frame the host would
    /// run in that span. An idle app runs none.
    pub fn advance(&mut self, duration: Duration) {
        let target = self.elapsed + duration;
        while let Some(wake) = self.next_wake().filter(|wake| *wake <= target) {
            self.elapsed = wake;
            self.frame();
        }
        self.elapsed = target;
        self.app.set_clock(Some(self.instant()));
    }

    /// Advances by `frames` frame intervals.
    pub fn advance_frames(&mut self, frames: u32) {
        self.advance(self.frame_interval * frames);
    }

    /// Advances until [`Simulator::is_idle`] holds and returns the virtual time
    /// it took. Fails when the app is still busy after `limit`.
    pub fn run_until_idle(&mut self, limit: Duration) -> Result<Duration, SimError> {
        let begin = self.elapsed;
        let deadline = begin + limit;
        while let Some(wake) = self.next_wake() {
            if wake > deadline {
                self.elapsed = deadline;
                self.app.set_clock(Some(self.instant()));
                return Err(SimError::NeverIdle { limit });
            }
            self.elapsed = wake;
            self.frame();
        }
        Ok(self.elapsed - begin)
    }

    // --- input: each verb runs its handler, then the frame the host would ---

    fn after_input<R>(&mut self, result: R) -> R {
        self.frame();
        result
    }

    /// The first visible widget with `role` and, if given, `name`.
    pub fn find(&self, role: Role, name: Option<&str>) -> Result<WidgetId, SimError> {
        self.app
            .find_widget(role, name)
            .ok_or_else(|| SimError::NotFound {
                role,
                name: name.map(str::to_owned),
            })
    }

    /// Sends an AccessKit action to a widget found by role and name.
    pub fn action(
        &mut self,
        action: Action,
        role: Role,
        name: Option<&str>,
        data: Option<ActionData>,
    ) -> Result<bool, SimError> {
        let id = self.find(role, name)?;
        let handled = self.app.dispatch_action(&ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: schnellui::a11y::to_access_id(id),
            data,
        });
        Ok(self.after_input(handled))
    }

    /// Clicks a widget found by role and name, through the AccessKit path.
    pub fn click(&mut self, role: Role, name: Option<&str>) -> Result<bool, SimError> {
        self.action(Action::Click, role, name, None)
    }

    /// Clicks the deepest interactive widget under a window point, like a pointer.
    pub fn click_at(&mut self, point: Point) -> bool {
        let handled = match self.app.hit_test(point) {
            Some(id) => self.app.dispatch_click(id),
            None => false,
        };
        self.after_input(handled)
    }

    /// Moves keyboard focus to a widget found by role and name.
    pub fn focus(&mut self, role: Role, name: Option<&str>) -> Result<bool, SimError> {
        let id = self.find(role, name)?;
        let focused = self.app.focus(Some(id));
        Ok(self.after_input(focused))
    }

    /// Clears keyboard focus.
    pub fn blur(&mut self) -> bool {
        let changed = self.app.focus(None);
        self.after_input(changed)
    }

    /// Presses one key with browser semantics.
    pub fn key(&mut self, key: UiKey<'_>) -> bool {
        let handled = self.app.dispatch_key(key);
        self.after_input(handled)
    }

    /// Types `text` one character per key event into the focused widget.
    pub fn type_text(&mut self, text: &str) {
        let mut buf = [0u8; 4];
        for c in text.chars() {
            let _ = self.app.dispatch_key(UiKey::Char(c.encode_utf8(&mut buf)));
            self.frame();
        }
    }

    /// Sends one mouse-wheel delta at a window point.
    pub fn wheel(&mut self, point: Point, delta_y: f32) -> bool {
        let moved = self.app.dispatch_wheel_at(point, delta_y);
        self.after_input(moved)
    }

    /// Sets a registered test signal by name and runs a frame.
    pub fn set_signal(&mut self, name: &str, value: impl Into<TestValue>) -> bool {
        let set = self.app.set_signal(name, value);
        self.after_input(set)
    }

    /// Replaces one referenced branch, as a host applying a
    /// `WindowUpdate::Subtrees` does, then runs a frame.
    pub fn replace_subtree(&mut self, target: ComponentRef, view: impl View) -> WidgetId {
        let root = self
            .app
            .replace_subtree(target, view)
            .expect("replacement target is mounted");
        self.after_input(root)
    }

    /// Resizes the logical viewport, as a window resize does, and runs a frame.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.app.resize(width, height);
        self.after_input(())
    }

    /// Applies the platform reduced-motion preference and runs a frame.
    pub fn set_reduced_motion(&mut self, reduce: bool) -> bool {
        let changed = self.app.apply_reduced_motion(reduce);
        self.after_input(changed)
    }

    // --- observation ---

    /// The accessibility tree as an assistive technology would read it.
    pub fn tree(&self) -> A11yTreeDump {
        schnellui::a11y::dump_tree(self.app.scene())
    }

    /// The accessible value of a widget found by role and name.
    pub fn value(&self, role: Role, name: Option<&str>) -> Option<String> {
        let tree = self.tree();
        find_by_role_name(&tree, role.label(), name).and_then(|node| node.value.clone())
    }

    /// Every accessible name under `role`, in reading order.
    pub fn names(&self, role: Role) -> Vec<String> {
        fn walk(node: &A11yNodeDump, role: &str, out: &mut Vec<String>) {
            if node.role == role {
                if let Some(name) = &node.name {
                    out.push(name.clone());
                }
            }
            for child in &node.children {
                walk(child, role, out);
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.tree().root {
            walk(root, role.label(), &mut out);
        }
        out
    }

    /// The accessibility node holding keyboard focus, if any.
    pub fn focused(&self) -> Option<A11yNodeDump> {
        fn walk(node: &A11yNodeDump, id: u64) -> Option<&A11yNodeDump> {
            if node.id == id {
                return Some(node);
            }
            node.children.iter().find_map(|child| walk(child, id))
        }
        let tree = self.tree();
        let id = tree.focus?;
        tree.root.as_ref().and_then(|root| walk(root, id)).cloned()
    }

    /// Whether a widget's accessible state includes `state`, such as `"checked"`.
    pub fn has_state(&self, role: Role, name: Option<&str>, state: &str) -> bool {
        let tree = self.tree();
        find_by_role_name(&tree, role.label(), name)
            .is_some_and(|node| node.state.iter().any(|s| s == state))
    }

    /// A textual fingerprint of a widget's painted primitives. Two equal
    /// fingerprints mean the widget draws the same frame.
    pub fn paint_fingerprint(&self, id: WidgetId) -> String {
        self.app
            .scene()
            .paint(id)
            .map(|paint| format!("{:?}", paint.primitives))
            .unwrap_or_default()
    }
}

/// A small, fast, seedable generator (SplitMix64). Simulations use it instead
/// of an external crate so a seed means the same run on every platform.
#[derive(Clone, Debug)]
pub struct SimRng {
    state: u64,
}

impl SimRng {
    pub fn new(seed: u64) -> SimRng {
        SimRng { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform integer in `0..bound`. `bound` must be positive.
    pub fn below(&mut self, bound: u64) -> u64 {
        assert!(bound > 0, "empty range");
        // Rejection sampling keeps small ranges unbiased.
        let zone = u64::MAX - u64::MAX % bound;
        loop {
            let x = self.next_u64();
            if x < zone {
                return x % bound;
            }
        }
    }

    /// A uniform index into a slice of length `len`.
    pub fn index(&mut self, len: usize) -> usize {
        self.below(len as u64) as usize
    }

    /// A uniform integer in `lo..=hi`.
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        assert!(lo <= hi, "empty range");
        lo + self.below((hi - lo) as u64 + 1) as i64
    }

    /// `true` with probability `numerator / denominator`.
    pub fn chance(&mut self, numerator: u64, denominator: u64) -> bool {
        self.below(denominator) < numerator
    }

    /// A uniform pick from a non-empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.index(items.len())]
    }

    /// A uniform duration in `lo..=hi` milliseconds.
    pub fn millis(&mut self, lo: u64, hi: u64) -> Duration {
        Duration::from_millis(self.range_i64(lo as i64, hi as i64) as u64)
    }

    /// Shuffles `items` in place (Fisher-Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.index(i + 1));
        }
    }

    /// An independent generator seeded from this one. Use it for a part of a
    /// run whose draws should not shift when another part draws more.
    pub fn fork(&mut self) -> SimRng {
        SimRng::new(self.next_u64())
    }
}

/// The host conditions a simulation runs under. [`SimHost::random`] lets a
/// seed pick them, so one scenario also covers odd window sizes, HiDPI
/// scales, slow and fast displays and frames that arrive late.
#[derive(Clone, Debug, PartialEq)]
pub struct SimHost {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub frame_rate: u32,
    /// Upper bound on how late an animation frame arrives.
    pub jitter: Duration,
    pub jitter_seed: u64,
}

impl Default for SimHost {
    /// An 800x600 window at scale 1, 60 Hz, with frames on time.
    fn default() -> SimHost {
        SimHost {
            width: 800,
            height: 600,
            scale: 1.0,
            frame_rate: 60,
            jitter: Duration::ZERO,
            jitter_seed: 0,
        }
    }
}

impl SimHost {
    /// Common display refresh rates.
    pub const FRAME_RATES: [u32; 5] = [24, 30, 60, 120, 144];
    /// Common scale factors, including fractional ones.
    pub const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 2.0, 3.0];

    /// A random host whose viewport is between `min_width`x`min_height` and
    /// twice that. Half the hosts deliver frames on time; the rest are up to
    /// one frame interval late.
    pub fn random(rng: &mut SimRng, min_width: u32, min_height: u32) -> SimHost {
        let width = min_width + rng.below(u64::from(min_width) + 1) as u32;
        let height = min_height + rng.below(u64::from(min_height) + 1) as u32;
        let scale = *rng.pick(&Self::SCALES);
        let frame_rate = *rng.pick(&Self::FRAME_RATES);
        let jitter = if rng.chance(1, 2) {
            Duration::ZERO
        } else {
            let interval = 1_000_000 / u64::from(frame_rate);
            Duration::from_micros(rng.below(interval + 1))
        };
        SimHost {
            width,
            height,
            scale,
            frame_rate,
            jitter,
            jitter_seed: rng.next_u64(),
        }
    }

    /// Mounts `root` on this host and starts simulating it.
    pub fn mount(&self, root: impl View) -> Simulator {
        self.simulate(App::mount_with_size_scaled(
            root,
            self.width,
            self.height,
            self.scale,
        ))
    }

    /// Mounts a themed app on this host. `view` is called again on every
    /// theme rebuild, as with [`App::mount_themed`].
    pub fn mount_themed<V, F>(&self, mode: impl Into<ThemeMode>, view: F) -> Simulator
    where
        V: View + 'static,
        F: FnMut() -> V + 'static,
    {
        self.simulate(App::mount_themed_with_size_scaled(
            mode,
            view,
            self.width,
            self.height,
            self.scale,
        ))
    }

    /// Starts simulating an app already mounted at this host's size and scale.
    pub fn simulate(&self, app: App) -> Simulator {
        let mut sim = Simulator::new(app);
        sim.set_frame_rate(self.frame_rate);
        sim.set_frame_jitter(self.jitter, self.jitter_seed);
        sim
    }

    /// The longest gap between two animation frames on this host.
    pub fn slowest_frame(&self) -> Duration {
        Duration::from_secs(1) / self.frame_rate + self.jitter
    }
}

/// The accessibility tree with everything mount-specific removed: widget ids,
/// the focus pointer and the `focused` state. Two apps showing the same UI
/// have equal structures however they got there.
pub fn structure(tree: &A11yTreeDump) -> A11yTreeDump {
    fn strip(node: &A11yNodeDump) -> A11yNodeDump {
        A11yNodeDump {
            id: 0,
            state: node
                .state
                .iter()
                .filter(|s| s.as_str() != "focused")
                .cloned()
                .collect(),
            children: node.children.iter().map(strip).collect(),
            ..node.clone()
        }
    }
    A11yTreeDump {
        focus: None,
        root: tree.root.as_ref().map(strip),
    }
}

/// One painted primitive reduced to its kind and rounded rect, in tree order.
pub type PaintBox = (u8, i32, i32, i32, i32);

/// Every rect-shaped primitive of the visible tree in pre-order, rounded to a
/// tenth of a pixel. Two apps showing the same UI paint the same boxes in the
/// same places, so this compares geometry without a GPU.
pub fn paint_layout(app: &App) -> Vec<PaintBox> {
    fn round(v: f32) -> i32 {
        (v * 10.0).round() as i32
    }
    let scene = app.scene();
    let mut out = Vec::new();
    let mut stack: Vec<WidgetId> = scene.root().into_iter().collect();
    while let Some(id) = stack.pop() {
        if !scene.is_visible(id) {
            continue;
        }
        if let Some(paint) = scene.paint(id) {
            for primitive in &paint.primitives {
                let (kind, rect) = match primitive {
                    Primitive::SolidRect { rect, .. } => (0, rect),
                    Primitive::GlyphQuad { rect, .. } => (1, rect),
                    Primitive::ImageQuad { rect, .. } => (2, rect),
                    Primitive::Line { .. } => continue,
                };
                out.push((
                    kind,
                    round(rect.x),
                    round(rect.y),
                    round(rect.width),
                    round(rect.height),
                ));
            }
        }
        if let Some(node) = scene.node(id) {
            stack.extend(node.children.iter().rev());
        }
    }
    out
}

fn env_u64(name: &str) -> Option<u64> {
    let raw = std::env::var(name).ok()?;
    let raw = raw.trim();
    let parsed = match raw.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => raw.parse(),
    };
    Some(parsed.unwrap_or_else(|_| panic!("{name}={raw:?} is not an integer")))
}

/// Runs `run` once per seed. The seeds derive from `name`, so separate
/// simulations explore different inputs, and stay the same between runs.
///
/// `SCHNELLUI_SIM_SEED` runs only that seed. `SCHNELLUI_SIM_CASES` overrides
/// `cases`. A panicking run is re-raised after printing its seed and the
/// command that replays it.
pub fn for_each_seed(name: &str, cases: u64, mut run: impl FnMut(&mut SimRng)) {
    let seeds: Vec<u64> = match env_u64("SCHNELLUI_SIM_SEED") {
        Some(seed) => vec![seed],
        None => {
            let base = name.bytes().fold(0xCBF2_9CE4_8422_2325u64, |hash, byte| {
                (hash ^ byte as u64).wrapping_mul(0x0100_0000_01B3)
            });
            let mut seeds = SimRng::new(base);
            let cases = env_u64("SCHNELLUI_SIM_CASES").unwrap_or(cases);
            (0..cases).map(|_| seeds.next_u64()).collect()
        }
    };
    for seed in seeds {
        let mut rng = SimRng::new(seed);
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| run(&mut rng))) {
            eprintln!(
                "simulation {name:?} failed with seed {seed:#x}; \
                 replay with SCHNELLUI_SIM_SEED={seed:#x}"
            );
            resume_unwind(panic);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = SimRng::new(7);
        let mut b = SimRng::new(7);
        for _ in 0..1000 {
            let x = a.range_i64(-3, 3);
            assert_eq!(x, b.range_i64(-3, 3));
            assert!((-3..=3).contains(&x));
        }
        let mut seen = [false; 7];
        for _ in 0..1000 {
            seen[(a.range_i64(-3, 3) + 3) as usize] = true;
        }
        assert!(
            seen.iter().all(|s| *s),
            "every value in a small range appears"
        );
    }

    #[test]
    fn shuffle_permutes_and_reaches_every_order() {
        let mut rng = SimRng::new(3);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let mut items = [0, 1, 2];
            rng.shuffle(&mut items);
            let mut sorted = items;
            sorted.sort();
            assert_eq!(sorted, [0, 1, 2]);
            seen.insert(items);
        }
        assert_eq!(seen.len(), 6);
    }

    #[test]
    fn random_hosts_stay_in_bounds() {
        let mut rng = SimRng::new(11);
        for _ in 0..200 {
            let host = SimHost::random(&mut rng, 100, 50);
            assert!((100..=200).contains(&host.width));
            assert!((50..=100).contains(&host.height));
            assert!(host.jitter <= Duration::from_secs(1) / host.frame_rate);
        }
    }

    #[test]
    fn for_each_seed_reports_the_failing_seed() {
        let mut seen = Vec::new();
        let result = catch_unwind(AssertUnwindSafe(|| {
            for_each_seed("report", 5, |rng| {
                seen.push(rng.next_u64());
                assert!(seen.len() < 3);
            })
        }));
        assert!(result.is_err());
        assert_eq!(seen.len(), 3, "stops at the first failing seed");
    }
}
