//! # schnellui-signal
//!
//! The reactive core (SOUL §3.1): a **push-then-pull, lazily-evaluated,
//! mark-and-sweep coloring graph** stored in a single process-global generational
//! arena. Handles ([`Signal`], [`Memo`], [`Effect`]) are `Copy` and carry only a
//! [`NodeId`] slotmap key — no `T: Copy` bound, no refcount bump on read.
//! Callback-free [`Subscription`] handles additionally bridge tracked `!Send`
//! producers to owner-local ready queues without storing those producers here.
//!
//! ## Contract for implementers (SOUL §3.1)
//! - The runtime is process-global, `static`, lock-guarded (`parking_lot::Mutex`)
//!   so it stays `Send + Sync` (Directive #7 — never a thread-local store).
//! - **Never hold the lock across user code.** Acquire → `mem::take` the compute
//!   fn / copy inputs → release → run → re-acquire to write back.
//! - **PUSH** (`set`): mark direct observers `Dirty`, deeper descendants `Check`,
//!   *queue* effects/subscriptions — no recomputation.
//! - **PULL** (`Runtime::flush`, or on read): `update_if_necessary` walks sources
//!   in read order, stops at the first `Dirty`, recomputes, runs the opt-in
//!   equality gate, and only re-marks observers if the value actually changed;
//!   settled subscriptions append their opaque key to their owner's ready queue.
//! - Edge lists are stable per-node [`smallvec::SmallVec`]s; cleared-and-refilled
//!   with unsubscribe-then-retrack for zero-alloc dependency tracking in steady
//!   state (§3.1, §4) — same deps every run ⇒ zero alloc, zero free.

use std::any::Any;
use std::marker::PhantomData;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::Arc;
use std::thread::ThreadId;

use once_cell::sync::Lazy;
use parking_lot::{Mutex, ReentrantMutex};
use slotmap::{SecondaryMap, SlotMap};
use smallvec::SmallVec;

slotmap::new_key_type! {
    /// A `Copy` handle into the reactive arena. Every [`Signal`], [`Memo`] and
    /// [`Effect`] is just one of these keys (SOUL §3.1).
    pub struct NodeId;
    /// Internal identity for a [`SubscriptionGroup`].
    struct SubscriptionGroupId;
}

/// Node coloring, packed into one byte of bitflags — never several bools
/// (SOUL §3.1). `Clean`/`Check`/`Dirty` are mutually exclusive *color* states;
/// the high bits carry orthogonal status.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct NodeFlags(u8);

impl NodeFlags {
    /// value valid, nothing to do.
    pub const CLEAN: NodeFlags = NodeFlags(0b0000_0000);
    /// a *transitive* ancestor changed; must verify sources before trusting cache.
    pub const CHECK: NodeFlags = NodeFlags(0b0000_0001);
    /// a *direct* source changed; must recompute.
    pub const DIRTY: NodeFlags = NodeFlags(0b0000_0010);
    /// mask over the color bits (`CLEAN`/`CHECK`/`DIRTY`).
    pub const COLOR_MASK: NodeFlags = NodeFlags(0b0000_0011);
    /// this node subscribes to reads while its compute runs. In this runtime the
    /// flag doubles as the *effect* (sink) marker: effects carry it and are queued
    /// on push; memos track deps the same way but omit it so they stay pull-only.
    pub const TRACKING: NodeFlags = NodeFlags(0b0000_0100);
    /// disposed by its owner scope; access returns `None` via `try_*`.
    pub const DISPOSED: NodeFlags = NodeFlags(0b0000_1000);
    /// currently executing (cycle guard).
    pub const RUNNING: NodeFlags = NodeFlags(0b0001_0000);
    /// A manual subscription has already put its key in its group's ready queue.
    /// It stays coalesced until the producer is tracked again.
    const NOTIFIED: NodeFlags = NodeFlags(0b0010_0000);

    /// Returns the color component (`CLEAN`/`CHECK`/`DIRTY`).
    #[inline]
    pub fn color(self) -> NodeFlags {
        NodeFlags(self.0 & Self::COLOR_MASK.0)
    }

    /// Replaces the color component, leaving status bits untouched.
    #[inline]
    pub fn set_color(&mut self, color: NodeFlags) {
        self.0 = (self.0 & !Self::COLOR_MASK.0) | (color.0 & Self::COLOR_MASK.0);
    }

    /// `true` if every bit in `flag` is set.
    #[inline]
    pub fn contains(self, flag: NodeFlags) -> bool {
        (self.0 & flag.0) == flag.0
    }

    /// Sets every bit in `flag`.
    #[inline]
    pub fn insert(&mut self, flag: NodeFlags) {
        self.0 |= flag.0;
    }

    /// Clears every bit in `flag`.
    #[inline]
    pub fn remove(&mut self, flag: NodeFlags) {
        self.0 &= !flag.0;
    }
}

/// Type-erased boxed cell; `Send + Sync` so the runtime stays `Send` (§3.1).
type AnyCell = Box<dyn Any + Send + Sync>;
/// Opt-in equality gate (`None` = the compute decides change) (SOUL §3.1).
type EqualsFn = fn(&dyn Any, &dyn Any) -> bool;
/// A memo/effect compute: writes into the cell, returns whether it changed.
type ComputeFn = Box<dyn FnMut(&mut dyn Any) -> bool + Send>;

/// One arena node (SOUL §3.1). Signals have `compute == None`; memos/effects own one.
struct Node {
    value: Option<AnyCell>,
    compute: Option<ComputeFn>,
    /// inline the common single-dep case (Sycamore's trick).
    sources: SmallVec<[NodeId; 1]>,
    observers: SmallVec<[NodeId; 2]>,
    /// bumped only on a real value change.
    version: u64,
    flags: NodeFlags,
    equals: Option<EqualsFn>,
    /// owner scope, for tree disposal.
    owner: Option<NodeId>,
    /// `Some` for a callback-free, UI-local manual subscription. Its key and
    /// ready queue live in [`Runtime`], so this arena node never holds UI data.
    subscription: Option<SubscriptionGroupId>,
    subscription_key: Option<u64>,
}

impl Node {
    fn signal(value: AnyCell, equals: Option<EqualsFn>) -> Node {
        Node {
            value: Some(value),
            compute: None,
            sources: SmallVec::new(),
            observers: SmallVec::new(),
            version: 0,
            flags: NodeFlags::default(),
            equals,
            owner: None,
            subscription: None,
            subscription_key: None,
        }
    }

    /// A derived compute node (memo/effect). Born `DIRTY` so the first pull runs
    /// it. Effects additionally carry `TRACKING` (the sink marker, §3.1).
    fn compute_node(
        value: AnyCell,
        compute: ComputeFn,
        equals: Option<EqualsFn>,
        tracking: bool,
    ) -> Node {
        let mut flags = NodeFlags::default();
        flags.set_color(NodeFlags::DIRTY);
        if tracking {
            flags.insert(NodeFlags::TRACKING);
        }
        Node {
            value: Some(value),
            compute: Some(compute),
            sources: SmallVec::new(),
            observers: SmallVec::new(),
            version: 0,
            flags,
            equals,
            owner: None,
            subscription: None,
            subscription_key: None,
        }
    }

    fn subscription(group: SubscriptionGroupId, key: u64) -> Node {
        Node {
            value: None,
            compute: None,
            sources: SmallVec::new(),
            observers: SmallVec::new(),
            version: 0,
            flags: NodeFlags::default(),
            equals: None,
            owner: None,
            subscription: Some(group),
            subscription_key: Some(key),
        }
    }
}

/// Runtime-owned data for callback-free subscriptions. The group owns the node
/// identities and the grow-only ready-key buffer; UI code remains outside it.
struct SubscriptionGroupState {
    subscriptions: SmallVec<[NodeId; 4]>,
    ready: Vec<u64>,
}

/// The process-global reactive arena (SOUL §3.1). Access it through the free
/// functions and handle methods; the single lock is acquired per operation and
/// **never** held across user code.
pub struct Runtime {
    nodes: SlotMap<NodeId, Node>,
    /// scope/effect → nodes it owns, for O(scope) disposal.
    scopes: SecondaryMap<NodeId, SmallVec<[NodeId; 4]>>,
    /// effects queued by the push phase, drained by [`Runtime::flush`]. Retains
    /// capacity across frames (grow-only pool) — zero alloc in steady state.
    pending_effects: Vec<NodeId>,
    /// Manual subscriptions awaiting pull/settling at the next flush.
    pending_subscriptions: Vec<NodeId>,
    /// double-buffer for [`Runtime::flush`]: the queued effects are drained here
    /// so re-queues during the flush land in a fresh `pending_effects` without a
    /// realloc. Grow-only, cleared-and-refilled.
    flush_scratch: Vec<NodeId>,
    subscription_flush_scratch: Vec<NodeId>,
    subscription_groups: SlotMap<SubscriptionGroupId, SubscriptionGroupState>,
    /// pooled work stack for the push propagation walk. Grow-only, `mem::take`n
    /// out per call and put back — zero alloc once warm.
    propagate_stack: Vec<(NodeId, bool)>,
    /// The node whose compute is currently running on each participating thread,
    /// for automatic dependency tracking (§3.3). Context remains runtime-owned
    /// rather than thread-local, but parallel producers cannot steal one another's
    /// observer while user code runs outside the arena lock.
    current_observers: SmallVec<[(ThreadId, NodeId); 4]>,
    /// Ownership scope installed for each participating thread. Like observer
    /// tracking, scopes are runtime-owned but must be thread-local in effect:
    /// user code runs outside the arena lock, so one app/test must never attach
    /// newly created nodes under another thread's scope.
    current_scopes: SmallVec<[(ThreadId, NodeId); 4]>,
    /// O(1) "did anything change anywhere" gate (§3.1).
    global_version: u64,
}

static RUNTIME: Lazy<Mutex<Runtime>> = Lazy::new(|| {
    Mutex::new(Runtime {
        nodes: SlotMap::with_key(),
        scopes: SecondaryMap::new(),
        pending_effects: Vec::new(),
        pending_subscriptions: Vec::new(),
        flush_scratch: Vec::new(),
        subscription_flush_scratch: Vec::new(),
        subscription_groups: SlotMap::with_key(),
        propagate_stack: Vec::new(),
        current_observers: SmallVec::new(),
        current_scopes: SmallVec::new(),
        global_version: 0,
    })
});

/// Serializes complete flushes without keeping the arena lock across user code.
/// A second app/thread must not observe the pending queues as empty while the
/// first flusher still owns their drained work in local scratch buffers. It is
/// reentrant so an effect may settle newly queued work on the same thread.
static FLUSH_LOCK: Lazy<ReentrantMutex<()>> = Lazy::new(|| ReentrantMutex::new(()));

impl Runtime {
    /// Runs the **pull** phase: drains the queued-effect set and re-runs each dirty
    /// effect via `update_if_necessary`, settling the graph glitch-free (SOUL §3.1).
    /// Called at every frame boundary by the umbrella `App::frame` (§8.1).
    ///
    /// Buffer discipline (§4): the queued effects are drained into a persistent
    /// scratch (both buffers retain capacity), so an effect that re-queues another
    /// effect mid-flush does so without a heap allocation once warm.
    pub fn flush() {
        let _flush = FLUSH_LOCK.lock();
        loop {
            let (mut scratch, mut subscription_scratch) = {
                let mut rt = RUNTIME.lock();
                if rt.pending_effects.is_empty() && rt.pending_subscriptions.is_empty() {
                    break;
                }
                // Take the scratch pool and refill it from the drained queue. A
                // same-thread nested flush simply borrows a fresh empty buffer.
                let mut scratch = std::mem::take(&mut rt.flush_scratch);
                scratch.clear();
                scratch.append(&mut rt.pending_effects);
                let mut subscription_scratch = std::mem::take(&mut rt.subscription_flush_scratch);
                subscription_scratch.clear();
                subscription_scratch.append(&mut rt.pending_subscriptions);
                (scratch, subscription_scratch)
            };
            for &eff in &scratch {
                update_if_necessary(eff);
            }
            // Subscription nodes have no callback. Pulling them settles memo
            // chains first, then records a ready key only when their source did
            // actually change.
            for &subscription in &subscription_scratch {
                update_if_necessary(subscription);
            }
            scratch.clear();
            subscription_scratch.clear();
            let mut rt = RUNTIME.lock();
            rt.flush_scratch = scratch;
            rt.subscription_flush_scratch = subscription_scratch;
        }
    }

    /// O(1) gate: the monotonic global version, bumped on every real value change.
    pub fn global_version() -> u64 {
        RUNTIME.lock().global_version
    }

    /// Number of live nodes in the arena (diagnostics/tests).
    pub fn node_count() -> usize {
        RUNTIME.lock().nodes.len()
    }
}

/// A reactive value cell. `Copy` handle carrying only a [`NodeId`] (SOUL §3.1).
pub struct Signal<T: 'static> {
    id: NodeId,
    _pd: PhantomData<fn() -> T>,
}

impl<T: 'static> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: 'static> Copy for Signal<T> {}

impl<T: Send + Sync + 'static> Signal<T> {
    /// The underlying arena key (also this node's a11y/paint id, §6.2).
    #[inline]
    pub fn id(self) -> NodeId {
        self.id
    }

    /// Reads the current value by clone, running the pull phase first (SOUL §3.1).
    /// Records a dependency if a compute is currently tracking (§3.3).
    pub fn get(self) -> T
    where
        T: Clone,
    {
        self.try_get().expect("Signal::get on a disposed node")
    }

    /// Non-panicking read: `None` if the owning scope was disposed (SOUL §3.1 —
    /// "disposed access returns `None` via `try_*`").
    pub fn try_get(self) -> Option<T>
    where
        T: Clone,
    {
        let mut rt = RUNTIME.lock();
        track_read(&mut rt, self.id);
        let node = rt.nodes.get(self.id)?;
        if node.flags.contains(NodeFlags::DISPOSED) {
            return None;
        }
        node.value
            .as_ref()
            .and_then(|v| v.downcast_ref::<T>())
            .cloned()
    }

    /// Reads by reference through a closure without cloning `T`. The value is
    /// lifted out of the arena and the lock released **before** `f` runs, so `f`
    /// may freely re-enter the runtime (§3.1 borrow-safety — never hold the lock
    /// across user code).
    pub fn with<R>(self, f: impl FnOnce(&T) -> R) -> R {
        let cell = {
            let mut rt = RUNTIME.lock();
            track_read(&mut rt, self.id);
            rt.nodes
                .get_mut(self.id)
                .filter(|n| !n.flags.contains(NodeFlags::DISPOSED))
                .and_then(|n| n.value.take())
                .expect("Signal::with on a disposed node")
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let v = cell.downcast_ref::<T>().expect("Signal type mismatch");
            f(v)
        }));
        if let Some(n) = RUNTIME.lock().nodes.get_mut(self.id) {
            n.value = Some(cell);
        }
        match result {
            Ok(value) => value,
            Err(payload) => resume_unwind(payload),
        }
    }

    /// Reads the current value by reference without recording a dependency.
    ///
    /// This is useful when a reactive producer already depends on a narrower
    /// invalidation signal but needs this signal only as a payload source. The
    /// runtime lock is released before `reader` runs. Panics after disposal; use
    /// [`Signal::try_peek`] when disposal is expected.
    pub fn peek<R>(self, reader: impl FnOnce(&T) -> R) -> R {
        self.try_peek(reader)
            .expect("Signal::peek on a disposed node")
    }

    /// Reads the current value by reference without recording a dependency, or
    /// returns `None` when the signal has been disposed.
    pub fn try_peek<R>(self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        let cell = {
            let mut rt = RUNTIME.lock();
            rt.nodes
                .get_mut(self.id)
                .filter(|n| !n.flags.contains(NodeFlags::DISPOSED))
                .and_then(|n| n.value.take())
        }?;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let v = cell.downcast_ref::<T>().expect("Signal type mismatch");
            reader(v)
        }));
        if let Some(n) = RUNTIME.lock().nodes.get_mut(self.id) {
            n.value = Some(cell);
        }
        match result {
            Ok(value) => Some(value),
            Err(payload) => resume_unwind(payload),
        }
    }

    /// Alias for [`Signal::peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn read_untracked<R>(self, reader: impl FnOnce(&T) -> R) -> R {
        self.peek(reader)
    }

    /// Alias for [`Signal::try_peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn try_read_untracked<R>(self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        self.try_peek(reader)
    }

    /// **PUSH**: overwrites the value, runs the opt-in equality gate, and on a real
    /// change marks direct observers `Dirty` / deeper `Check` and queues effects —
    /// no recomputation here (SOUL §3.1). The existing box is reused in place, so a
    /// `Copy`-typed signal `set` allocates nothing.
    pub fn set(self, value: T) {
        let mut rt = RUNTIME.lock();
        let node = match rt.nodes.get_mut(self.id) {
            Some(n) if !n.flags.contains(NodeFlags::DISPOSED) => n,
            _ => return,
        };
        let equals = node.equals;
        let slot = match node.value.as_mut().and_then(|v| v.downcast_mut::<T>()) {
            Some(s) => s,
            None => return,
        };
        // Opt-in equality gate is *free* on set: compare before overwrite (§3.1).
        if let Some(eq) = equals {
            if eq(slot as &dyn Any, &value as &dyn Any) {
                return; // no change → no version bump, no propagation
            }
        }
        *slot = value; // reuse the existing allocation
        node.version = node.version.wrapping_add(1);
        rt.global_version = rt.global_version.wrapping_add(1);
        propagate_change(&mut rt, self.id);
    }

    /// **PUSH** via in-place mutation. Always fires (mutation is assumed to change);
    /// callers wanting change-suppression use an `equals`-gated signal (SOUL §3.1).
    /// The value box is lifted out (its allocation reused) so mutation runs without
    /// the lock and cannot re-enter under it.
    pub fn update(self, f: impl FnOnce(&mut T)) {
        let _ = self.try_transaction(f);
    }

    /// Mutates the value in place and returns the closure's result.
    ///
    /// The transaction is panic-safe: its value cell is restored to the arena
    /// before an unwind resumes, so later reads and mutations remain valid. A
    /// panic may follow a partial mutation, which is treated like any other
    /// mutation and therefore marks consumers dirty. Panics after disposal; use
    /// [`Signal::try_transaction`] when disposal is expected.
    pub fn transaction<R>(self, f: impl FnOnce(&mut T) -> R) -> R {
        self.try_transaction(f)
            .expect("Signal::transaction on a disposed node")
    }

    /// Mutates the value in place and returns the closure's result, or returns
    /// `None` without calling `f` after disposal.
    pub fn try_transaction<R>(self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut taken: Box<T> = {
            let mut rt = RUNTIME.lock();
            let node = match rt.nodes.get_mut(self.id) {
                Some(n) if !n.flags.contains(NodeFlags::DISPOSED) => n,
                _ => return None,
            };
            let any = node.value.take().expect("signal value already borrowed");
            any.downcast::<T>().expect("Signal type mismatch")
        };

        let result = catch_unwind(AssertUnwindSafe(|| f(&mut taken)));

        let mut rt = RUNTIME.lock();
        let restored = if let Some(node) = rt.nodes.get_mut(self.id) {
            node.value = Some(taken);
            node.version = node.version.wrapping_add(1);
            true
        } else {
            false
        };
        if restored {
            rt.global_version = rt.global_version.wrapping_add(1);
            propagate_change(&mut rt, self.id);
        }
        drop(rt);
        match result {
            Ok(value) if restored => Some(value),
            Ok(_) => None,
            Err(payload) => resume_unwind(payload),
        }
    }
}

/// A cached derived value: `compute` runs lazily on demand, stops propagation
/// when its value does not change (SOUL §3.1). `Copy` handle over a [`NodeId`].
pub struct Memo<T: 'static> {
    id: NodeId,
    _pd: PhantomData<fn() -> T>,
}

impl<T: 'static> Clone for Memo<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: 'static> Copy for Memo<T> {}

impl<T: 'static> Memo<T> {
    /// The underlying arena key.
    #[inline]
    pub fn id(self) -> NodeId {
        self.id
    }

    /// Reads the memo, running `update_if_necessary` first (SOUL §3.1 PULL).
    pub fn get(self) -> T
    where
        T: Clone,
    {
        self.try_get().expect("Memo::get on a disposed node")
    }

    /// Non-panicking read: `None` if disposed (SOUL §3.1). Settles the node
    /// (`update_if_necessary`) before cloning the cache, then subscribes the
    /// currently-tracking observer.
    pub fn try_get(self) -> Option<T>
    where
        T: Clone,
    {
        update_if_necessary(self.id);
        let mut rt = RUNTIME.lock();
        track_read(&mut rt, self.id);
        let node = rt.nodes.get(self.id)?;
        if node.flags.contains(NodeFlags::DISPOSED) {
            return None;
        }
        node.value
            .as_ref()
            .and_then(|v| v.downcast_ref::<Option<T>>())
            .and_then(|opt| opt.clone())
    }

    /// Reads by reference through a closure without cloning `T`. Settles the memo,
    /// lifts the cell out, releases the lock, then runs `f` (§3.1 borrow-safety).
    pub fn with<R>(self, f: impl FnOnce(&T) -> R) -> R {
        update_if_necessary(self.id);
        let cell = {
            let mut rt = RUNTIME.lock();
            track_read(&mut rt, self.id);
            rt.nodes
                .get_mut(self.id)
                .filter(|n| !n.flags.contains(NodeFlags::DISPOSED))
                .and_then(|n| n.value.take())
                .expect("Memo::with on a disposed node")
        };
        let result = catch_unwind(AssertUnwindSafe(|| {
            let opt = cell
                .downcast_ref::<Option<T>>()
                .expect("Memo cell type mismatch");
            let val = opt.as_ref().expect("Memo read before first computation");
            f(val)
        }));
        if let Some(n) = RUNTIME.lock().nodes.get_mut(self.id) {
            n.value = Some(cell);
        }
        match result {
            Ok(value) => value,
            Err(payload) => resume_unwind(payload),
        }
    }

    /// Reads the settled cached value without recording a dependency.
    ///
    /// The memo still tracks its own sources while it settles; only the caller's
    /// observer is excluded. Panics after disposal; use [`Memo::try_peek`] when
    /// disposal is expected.
    pub fn peek<R>(self, reader: impl FnOnce(&T) -> R) -> R {
        self.try_peek(reader)
            .expect("Memo::peek on a disposed node")
    }

    /// Reads the settled cached value without recording a dependency, or returns
    /// `None` after disposal.
    pub fn try_peek<R>(self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        update_if_necessary(self.id);
        let cell = {
            let mut rt = RUNTIME.lock();
            rt.nodes
                .get_mut(self.id)
                .filter(|n| !n.flags.contains(NodeFlags::DISPOSED))
                .and_then(|n| n.value.take())
        }?;
        let result = catch_unwind(AssertUnwindSafe(|| {
            let opt = cell
                .downcast_ref::<Option<T>>()
                .expect("Memo cell type mismatch");
            let val = opt.as_ref().expect("Memo read before first computation");
            reader(val)
        }));
        if let Some(n) = RUNTIME.lock().nodes.get_mut(self.id) {
            n.value = Some(cell);
        }
        match result {
            Ok(value) => Some(value),
            Err(payload) => resume_unwind(payload),
        }
    }

    /// Alias for [`Memo::peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn read_untracked<R>(self, reader: impl FnOnce(&T) -> R) -> R {
        self.peek(reader)
    }

    /// Alias for [`Memo::try_peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn try_read_untracked<R>(self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        self.try_peek(reader)
    }
}

/// A reactive side effect (a `RenderEffect`-shaped node, §3.3): runs on creation,
/// re-runs when a tracked source changes, cancels on dispose.
#[derive(Clone, Copy)]
pub struct Effect {
    id: NodeId,
}

/// A callback-free owner for UI-local reactive producers.
///
/// A group is intentionally `!Send`: it is designed for a retained UI tree on
/// one thread. The reactive arena stores only dependency edges and integer keys,
/// never the producer closures or other UI-local data.
pub struct SubscriptionGroup {
    id: SubscriptionGroupId,
    _not_send: PhantomData<Rc<()>>,
}

impl Default for SubscriptionGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriptionGroup {
    /// Creates an empty group with a reusable ready-key queue.
    pub fn new() -> Self {
        let id = RUNTIME
            .lock()
            .subscription_groups
            .insert(SubscriptionGroupState {
                subscriptions: SmallVec::new(),
                ready: Vec::new(),
            });
        Self {
            id,
            _not_send: PhantomData,
        }
    }

    /// Creates a manual dependency tracker whose changes make `key` ready.
    pub fn subscribe(&self, key: u64) -> Subscription {
        let mut rt = RUNTIME.lock();
        if !rt.subscription_groups.contains_key(self.id) {
            unreachable!("SubscriptionGroup used after drop");
        }
        let id = rt.nodes.insert(Node::subscription(self.id, key));
        rt.subscription_groups
            .get_mut(self.id)
            .expect("subscription group disappeared")
            .subscriptions
            .push(id);
        Subscription {
            handle: Arc::new(SubscriptionHandle { id, key }),
            _not_send: PhantomData,
        }
    }

    /// Appends ready keys and clears the group's queue. Keys remain coalesced on
    /// their subscriptions until those subscriptions are tracked again.
    pub fn drain_ready_into(&self, out: &mut Vec<u64>) {
        let mut rt = RUNTIME.lock();
        let Some(group) = rt.subscription_groups.get_mut(self.id) else {
            return;
        };
        out.extend_from_slice(&group.ready);
        group.ready.clear();
    }

    /// Invalidates a subscription and every clone of its handle. The group stays
    /// usable and may receive new subscriptions afterwards.
    pub fn unsubscribe(&self, subscription: Subscription) {
        subscription.unsubscribe();
    }

    /// Removes every subscription and pending key while retaining this group's
    /// queue allocation for reuse (for example, after a UI runtime reset).
    pub fn clear(&self) {
        let mut rt = RUNTIME.lock();
        let Some(group) = rt.subscription_groups.get_mut(self.id) else {
            return;
        };
        let subscriptions = std::mem::take(&mut group.subscriptions);
        group.ready.clear();
        for id in subscriptions {
            dispose_node(&mut rt, id);
        }
    }
}

impl Drop for SubscriptionGroup {
    fn drop(&mut self) {
        let mut rt = RUNTIME.lock();
        let Some(group) = rt.subscription_groups.remove(self.id) else {
            return;
        };
        for id in group.subscriptions {
            dispose_node(&mut rt, id);
        }
    }
}

/// One manually tracked, callback-free reactive producer in a
/// [`SubscriptionGroup`]. Dropping its final handle removes all dependency edges.
#[derive(Clone)]
pub struct Subscription {
    handle: Arc<SubscriptionHandle>,
    _not_send: PhantomData<Rc<()>>,
}

/// The shared registration behind cloneable [`Subscription`] handles. The last
/// handle cleans up automatically; explicit unsubscription invalidates it for
/// every clone immediately.
struct SubscriptionHandle {
    id: NodeId,
    key: u64,
}

impl Subscription {
    /// Runs `f` with this subscription as the dependency observer. Old edges are
    /// replaced before the call, and the ready/coalesced state is consumed. `f`
    /// is unconstrained (`!Send` is fine) and always executes without the runtime
    /// lock held.
    pub fn track<R>(&self, f: impl FnOnce() -> R) -> R {
        let previous = {
            let mut rt = RUNTIME.lock();
            let valid = rt.nodes.get(self.handle.id).is_some_and(|node| {
                node.subscription.is_some() && !node.flags.contains(NodeFlags::DISPOSED)
            });
            if valid {
                clear_subscription_ready(&mut rt, self.handle.id, self.handle.key);
                unsubscribe_sources(&mut rt, self.handle.id);
                let node = rt
                    .nodes
                    .get_mut(self.handle.id)
                    .expect("subscription disappeared");
                node.flags.set_color(NodeFlags::CLEAN);
                node.flags.insert(NodeFlags::RUNNING);
                let previous = replace_current_observer(&mut rt, Some(self.handle.id));
                Some(previous)
            } else {
                None
            }
        };

        // The runtime lock is deliberately absent while user/UI code executes.
        // Catch only long enough to restore graph context, then resume the same
        // panic so callers observe ordinary Rust unwind behavior.
        let result = catch_unwind(AssertUnwindSafe(f));

        if let Some(previous) = previous {
            let mut rt = RUNTIME.lock();
            if let Some(node) = rt.nodes.get_mut(self.handle.id) {
                node.flags.remove(NodeFlags::RUNNING);
                node.flags.set_color(NodeFlags::CLEAN);
            }
            replace_current_observer(&mut rt, previous);
        }
        match result {
            Ok(value) => value,
            Err(payload) => resume_unwind(payload),
        }
    }

    /// Removes dependency edges and any undrained ready key while retaining this
    /// subscription for a future [`track`](Self::track).
    pub fn clear(&self) {
        let mut rt = RUNTIME.lock();
        if rt.nodes.contains_key(self.handle.id) {
            clear_subscription_ready(&mut rt, self.handle.id, self.handle.key);
            unsubscribe_sources(&mut rt, self.handle.id);
            if let Some(node) = rt.nodes.get_mut(self.handle.id) {
                node.flags.set_color(NodeFlags::CLEAN);
            }
        }
    }

    /// Immediately removes this subscription from its group and from the graph.
    pub fn unsubscribe(&self) {
        let mut rt = RUNTIME.lock();
        remove_subscription(&mut rt, self.handle.id, self.handle.key);
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        let mut rt = RUNTIME.lock();
        remove_subscription(&mut rt, self.id, self.key);
    }
}

impl Effect {
    /// The underlying arena key.
    #[inline]
    pub fn id(self) -> NodeId {
        self.id
    }
}

/// An ownership scope (owner tree, SOUL §3.1). Disposing it removes every node it
/// owns from the arena; subsequent access returns `None` via `try_*`.
#[derive(Clone, Copy)]
pub struct Scope {
    id: NodeId,
}

impl Scope {
    /// The underlying arena key.
    #[inline]
    pub fn id(self) -> NodeId {
        self.id
    }

    /// Disposes this scope and every node it owns (SOUL §3.1).
    pub fn dispose(self) {
        dispose(self);
    }
}

// --- free constructors (SOUL §3.1 API) ---

/// Creates a signal with no equality gate: every `set` fires (SOUL §3.1).
pub fn create_signal<T: Send + Sync + 'static>(value: T) -> Signal<T> {
    let mut rt = RUNTIME.lock();
    let id = rt.nodes.insert(Node::signal(Box::new(value), None));
    attach_to_current_scope(&mut rt, id);
    Signal {
        id,
        _pd: PhantomData,
    }
}

/// Creates a signal with an **opt-in** equality gate — `set` only fires when
/// `equals(old, new)` is `false` (SOUL §3.1). Pass a bitwise comparator to dodge
/// the documented `NaN`-re-fires-forever footgun.
pub fn create_signal_equals<T: Send + Sync + 'static>(value: T, equals: EqualsFn) -> Signal<T> {
    let mut rt = RUNTIME.lock();
    let id = rt.nodes.insert(Node::signal(Box::new(value), Some(equals)));
    attach_to_current_scope(&mut rt, id);
    Signal {
        id,
        _pd: PhantomData,
    }
}

/// Creates a memo (derived, cached, lazily pulled) (SOUL §3.1).
/// `compute` reads other signals/memos; those reads auto-subscribe (§3.3).
///
/// Without an equality gate a memo reports *changed* every time it recomputes, so
/// it does not by itself prune propagation — opt in via [`create_memo_equals`].
pub fn create_memo<T: Send + Sync + Clone + 'static>(
    compute: impl FnMut() -> T + Send + 'static,
) -> Memo<T> {
    make_memo(compute, None)
}

/// Creates a memo with an **opt-in** equality gate (SOUL §3.1): the boundary stops
/// propagation when the recomputed value compares equal to the retained one
/// (double-buffered — the previous value is kept when unchanged). Pass a bitwise
/// comparator for `NaN`-safe pruning.
pub fn create_memo_equals<T: Send + Sync + Clone + 'static>(
    compute: impl FnMut() -> T + Send + 'static,
    equals: EqualsFn,
) -> Memo<T> {
    make_memo(compute, Some(equals))
}

fn make_memo<T: Send + Sync + Clone + 'static>(
    mut compute: impl FnMut() -> T + Send + 'static,
    equals: Option<EqualsFn>,
) -> Memo<T> {
    // The wrapper writes into an `Option<T>` cell (None until first computed). On
    // recompute it runs the opt-in equality gate, keeping the old value on a match
    // (double-buffer) and reporting `changed` accordingly (§3.1).
    let wrapper: ComputeFn = Box::new(move |slot: &mut dyn Any| {
        let new_val = compute(); // user code — runs with the lock released
        let cell = slot
            .downcast_mut::<Option<T>>()
            .expect("Memo cell type mismatch");
        let changed = match (equals, cell.as_ref()) {
            (Some(eq), Some(old)) => !eq(old as &dyn Any, &new_val as &dyn Any),
            _ => true, // no gate, or first computation → changed
        };
        if changed {
            *cell = Some(new_val);
        }
        changed
    });
    let cell: AnyCell = Box::new(None::<T>);
    let mut rt = RUNTIME.lock();
    let id = rt
        .nodes
        .insert(Node::compute_node(cell, wrapper, equals, false));
    attach_to_current_scope(&mut rt, id);
    Memo {
        id,
        _pd: PhantomData,
    }
}

/// Creates an effect: runs once now (creating any nodes), re-runs on tracked
/// change, cancels when its scope disposes (SOUL §3.3).
pub fn create_effect(f: impl FnMut() + Send + 'static) -> Effect {
    let mut f = f;
    let wrapper: ComputeFn = Box::new(move |_slot: &mut dyn Any| {
        f(); // user code — runs with the lock released
        false // effects are sinks: they never propagate a value change
    });
    let id = {
        let mut rt = RUNTIME.lock();
        let id = rt.nodes.insert(Node::compute_node(
            Box::new(()),
            wrapper,
            None,
            /* tracking = effect */ true,
        ));
        attach_to_current_scope(&mut rt, id);
        id
    };
    // First run is synchronous (§3.3), outside the lock so it can read signals.
    update_if_necessary(id);
    Effect { id }
}

/// Opens a new ownership scope as a child of the current one (SOUL §3.1).
pub fn create_scope() -> Scope {
    let mut rt = RUNTIME.lock();
    let id = rt.nodes.insert(Node::signal(Box::new(()), None));
    rt.scopes.insert(id, SmallVec::new());
    attach_to_current_scope(&mut rt, id);
    Scope { id }
}

/// Runs `f` with `scope` installed as the current ownership scope, so signals,
/// memos and effects created inside `f` are owned by `scope` and torn down when it
/// is disposed (SOUL §3.1). The previous scope is restored afterwards.
pub fn run_in_scope<R>(scope: Scope, f: impl FnOnce() -> R) -> R {
    let prev = {
        let mut rt = RUNTIME.lock();
        replace_current_scope(&mut rt, Some(scope.id))
    };
    // Scope restoration is part of the runtime invariant: a panic from user
    // code must not leave this thread creating future nodes under a stale owner.
    let result = catch_unwind(AssertUnwindSafe(f));
    let mut rt = RUNTIME.lock();
    replace_current_scope(&mut rt, prev);
    drop(rt);
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

/// Runs `f` without the calling thread's current reactive observer.
///
/// Reads inside `f` do not become dependencies of the enclosing memo, effect,
/// or manual subscription. The prior observer is restored even if `f` panics.
/// Prefer [`Signal::peek`] or [`Memo::peek`] for one untracked closure read.
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    let previous = {
        let mut rt = RUNTIME.lock();
        replace_current_observer(&mut rt, None)
    };
    let result = catch_unwind(AssertUnwindSafe(f));
    let mut rt = RUNTIME.lock();
    replace_current_observer(&mut rt, previous);
    drop(rt);
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

/// Disposes a scope and every node it owns (recursively), marking them `DISPOSED`,
/// unsubscribing their graph edges, and removing them from the arena (SOUL §3.1).
/// Disposal is a teardown event and may allocate (§4).
pub fn dispose(scope: Scope) {
    let mut rt = RUNTIME.lock();
    dispose_node(&mut rt, scope.id);
}

// --- internal helpers ---

/// Returns the observer installed for the calling thread. User computes run
/// without the arena lock, so several threads may legitimately have active
/// tracking contexts at once.
fn current_observer(rt: &Runtime) -> Option<NodeId> {
    let thread = std::thread::current().id();
    rt.current_observers
        .iter()
        .find_map(|(owner, observer)| (*owner == thread).then_some(*observer))
}

/// Replaces the calling thread's observer and returns its previous value. The
/// small inline table keeps the ordinary one-thread UI path allocation-free.
fn replace_current_observer(rt: &mut Runtime, next: Option<NodeId>) -> Option<NodeId> {
    let thread = std::thread::current().id();
    let position = rt
        .current_observers
        .iter()
        .position(|(owner, _)| *owner == thread);
    match (position, next) {
        (Some(index), Some(observer)) => Some(std::mem::replace(
            &mut rt.current_observers[index].1,
            observer,
        )),
        (Some(index), None) => Some(rt.current_observers.swap_remove(index).1),
        (None, Some(observer)) => {
            rt.current_observers.push((thread, observer));
            None
        }
        (None, None) => None,
    }
}

/// Returns the ownership scope installed for the calling thread.
fn current_scope(rt: &Runtime) -> Option<NodeId> {
    let thread = std::thread::current().id();
    rt.current_scopes
        .iter()
        .find_map(|(owner, scope)| (*owner == thread).then_some(*scope))
}

/// Replaces the calling thread's ownership scope and returns its previous one.
/// The inline table mirrors `current_observers` so the ordinary UI path remains
/// allocation-free after warmup.
fn replace_current_scope(rt: &mut Runtime, next: Option<NodeId>) -> Option<NodeId> {
    let thread = std::thread::current().id();
    let position = rt
        .current_scopes
        .iter()
        .position(|(owner, _)| *owner == thread);
    match (position, next) {
        (Some(index), Some(scope)) => {
            Some(std::mem::replace(&mut rt.current_scopes[index].1, scope))
        }
        (Some(index), None) => Some(rt.current_scopes.swap_remove(index).1),
        (None, Some(scope)) => {
            rt.current_scopes.push((thread, scope));
            None
        }
        (None, None) => None,
    }
}

/// Records `source` as a dependency of the currently-tracking observer (§3.3).
/// A `contains` guard keeps re-tracking the same dep allocation-free.
fn track_read(rt: &mut Runtime, source: NodeId) {
    let Some(observer) = current_observer(rt) else {
        return;
    };
    if observer == source {
        return; // a compute reading its own cell: ignore (no self-edge)
    }
    if let Some(node) = rt.nodes.get_mut(source) {
        if !node.observers.contains(&observer) {
            node.observers.push(observer);
        }
    } else {
        return;
    }
    if let Some(obs) = rt.nodes.get_mut(observer) {
        if !obs.sources.contains(&source) {
            obs.sources.push(source);
        }
    }
}

/// Removes all source edges from `observer`, retaining its source buffer for the
/// next tracking pass. Shared by computed nodes and manual subscriptions.
fn unsubscribe_sources(rt: &mut Runtime, observer: NodeId) {
    let len = match rt.nodes.get(observer) {
        Some(node) => node.sources.len(),
        None => return,
    };
    for i in 0..len {
        let source = rt.nodes[observer].sources[i];
        if let Some(source_node) = rt.nodes.get_mut(source) {
            if let Some(pos) = source_node.observers.iter().position(|&id| id == observer) {
                source_node.observers.swap_remove(pos);
            }
        }
    }
    if let Some(node) = rt.nodes.get_mut(observer) {
        node.sources.clear();
    }
}

/// Clears one subscription's notification bit and removes its key from the
/// group's undrained queue when no sibling subscription still represents it.
fn clear_subscription_ready(rt: &mut Runtime, id: NodeId, key: u64) {
    let Some(group_id) = rt.nodes.get(id).and_then(|node| node.subscription) else {
        return;
    };
    let was_notified = rt
        .nodes
        .get(id)
        .is_some_and(|node| node.flags.contains(NodeFlags::NOTIFIED));
    if !was_notified {
        return;
    }
    if let Some(node) = rt.nodes.get_mut(id) {
        node.flags.remove(NodeFlags::NOTIFIED);
    }
    let sibling_is_notified = rt.subscription_groups.get(group_id).is_some_and(|group| {
        group.subscriptions.iter().any(|&other| {
            other != id
                && rt.nodes.get(other).is_some_and(|node| {
                    node.flags.contains(NodeFlags::NOTIFIED)
                        && node.subscription == Some(group_id)
                        && node.subscription_key == Some(key)
                })
        })
    });
    if !sibling_is_notified {
        if let Some(group) = rt.subscription_groups.get_mut(group_id) {
            group.ready.retain(|&ready_key| ready_key != key);
        }
    }
}

/// Marks a settled manual subscription ready. The flag is set even after its key
/// has been drained, which suppresses duplicate work until the producer retracks.
fn queue_subscription_ready(rt: &mut Runtime, id: NodeId) {
    let Some((group_id, key, already_notified)) = rt.nodes.get(id).and_then(|node| {
        node.subscription
            .zip(node.subscription_key)
            .map(|(group, key)| (group, key, node.flags.contains(NodeFlags::NOTIFIED)))
    }) else {
        return;
    };
    if already_notified {
        return;
    }
    if let Some(node) = rt.nodes.get_mut(id) {
        node.flags.insert(NodeFlags::NOTIFIED);
    }
    if let Some(group) = rt.subscription_groups.get_mut(group_id) {
        if !group.ready.contains(&key) {
            group.ready.push(key);
        }
    }
}

/// Removes a subscription's graph node and ownership entry. This is idempotent
/// so explicit unsubscribe, dropping, and group teardown compose safely.
fn remove_subscription(rt: &mut Runtime, id: NodeId, key: u64) {
    let Some(group_id) = rt.nodes.get(id).and_then(|node| node.subscription) else {
        return;
    };
    clear_subscription_ready(rt, id, key);
    if let Some(group) = rt.subscription_groups.get_mut(group_id) {
        if let Some(pos) = group.subscriptions.iter().position(|&member| member == id) {
            group.subscriptions.swap_remove(pos);
        }
    }
    dispose_node(rt, id);
}

/// **PUSH** propagation from a changed node: direct observers → `Dirty`, deeper
/// descendants → `Check`, effects queued (SOUL §3.1). Iterative, holds the lock
/// only over arena bookkeeping (no user code), uses a pooled scratch stack so it
/// allocates nothing once warm.
fn propagate_change(rt: &mut Runtime, changed: NodeId) {
    let mut stack = std::mem::take(&mut rt.propagate_stack);
    stack.clear();
    if let Some(n) = rt.nodes.get(changed) {
        for &o in &n.observers {
            stack.push((o, true));
        }
    }
    while let Some((id, direct)) = stack.pop() {
        let is_effect = {
            let Some(node) = rt.nodes.get_mut(id) else {
                continue;
            };
            if node.flags.contains(NodeFlags::DISPOSED) {
                continue;
            }
            // A running node already observes fresh inputs (demand-driven pull);
            // re-marking it would spuriously re-run it — skip (§3.1 glitch-free).
            if node.flags.contains(NodeFlags::RUNNING) {
                continue;
            }
            let cur = node.flags.color();
            if cur == NodeFlags::DIRTY {
                continue; // already dirty; its subtree is already marked
            }
            if cur == NodeFlags::CHECK && !direct {
                continue; // already check; deeper re-mark is redundant
            }
            node.flags.set_color(if direct {
                NodeFlags::DIRTY
            } else {
                NodeFlags::CHECK
            });
            (node.compute.is_some() && node.flags.contains(NodeFlags::TRACKING))
                || node.subscription.is_some()
        };
        if is_effect {
            if rt
                .nodes
                .get(id)
                .is_some_and(|node| node.subscription.is_some())
            {
                if !rt.pending_subscriptions.contains(&id) {
                    rt.pending_subscriptions.push(id);
                }
            } else if !rt.pending_effects.contains(&id) {
                rt.pending_effects.push(id);
            }
        }
        if let Some(node) = rt.nodes.get(id) {
            for &o in &node.observers {
                stack.push((o, false));
            }
        }
    }
    rt.propagate_stack = stack;
}

/// **PULL**: ensures `id` holds a valid value (SOUL §3.1). `Clean` → nothing;
/// `Check` → verify sources in read order, stopping at the first that resolves
/// `Dirty`; `Dirty` (or a source turned us `Dirty`) → recompute. Re-locks per
/// step and per source — **never** holds the lock across the recursion into user
/// computes.
fn update_if_necessary(id: NodeId) {
    let color = {
        let rt = RUNTIME.lock();
        match rt.nodes.get(id) {
            Some(n) if n.flags.contains(NodeFlags::DISPOSED) => return,
            Some(n) if n.flags.contains(NodeFlags::RUNNING) => return,
            Some(n) => n.flags.color(),
            None => return,
        }
    };

    if color == NodeFlags::CLEAN {
        return;
    }

    if color == NodeFlags::CHECK {
        let mut i = 0usize;
        loop {
            let src = {
                let rt = RUNTIME.lock();
                let Some(n) = rt.nodes.get(id) else {
                    return;
                };
                if i >= n.sources.len() {
                    break;
                }
                n.sources[i]
            };
            update_if_necessary(src);
            // A source that actually changed will have marked us `Dirty`.
            let dirty = {
                let rt = RUNTIME.lock();
                rt.nodes
                    .get(id)
                    .map(|n| n.flags.color() == NodeFlags::DIRTY)
                    .unwrap_or(false)
            };
            if dirty {
                break;
            }
            i += 1;
        }
    }

    let final_color = {
        let rt = RUNTIME.lock();
        rt.nodes
            .get(id)
            .map(|n| n.flags.color())
            .unwrap_or(NodeFlags::CLEAN)
    };
    if final_color == NodeFlags::DIRTY {
        recompute(id);
    } else if final_color == NodeFlags::CHECK {
        // No source resolved dirty → the cache is trustworthy after all.
        if let Some(n) = RUNTIME.lock().nodes.get_mut(id) {
            n.flags.set_color(NodeFlags::CLEAN);
        }
    }
}

/// Recomputes a dirty compute node (SOUL §3.1). Unsubscribes the node's old edges
/// then re-tracks during the run (dynamic deps), lifts the compute + cell out and
/// releases the lock before running user code, writes back and — only on a real
/// change — bumps versions and propagates. Zero-alloc on repeat: edge lists are
/// cleared-and-refilled in place, retaining capacity.
fn recompute(id: NodeId) {
    let taken = {
        let mut rt = RUNTIME.lock();
        let (disposed, running, has_compute, is_subscription) = match rt.nodes.get(id) {
            Some(n) => (
                n.flags.contains(NodeFlags::DISPOSED),
                n.flags.contains(NodeFlags::RUNNING),
                n.compute.is_some(),
                n.subscription.is_some(),
            ),
            None => return,
        };
        if disposed || running {
            return;
        }
        if !has_compute {
            if is_subscription {
                if let Some(n) = rt.nodes.get_mut(id) {
                    n.flags.set_color(NodeFlags::CLEAN);
                }
                queue_subscription_ready(&mut rt, id);
                return;
            }
            // A plain signal can never be dirty legitimately; treat as clean.
            if let Some(n) = rt.nodes.get_mut(id) {
                n.flags.set_color(NodeFlags::CLEAN);
            }
            return;
        }

        unsubscribe_sources(&mut rt, id);

        let (compute, cell) = {
            let n = rt.nodes.get_mut(id).unwrap();
            n.flags.insert(NodeFlags::RUNNING);
            (n.compute.take(), n.value.take())
        };
        let prev = replace_current_observer(&mut rt, Some(id));
        (compute, cell, prev)
    };

    let (mut compute, mut cell, prev) = taken;

    // --- user code runs here, lock released, deps re-track via track_read ---
    let changed = match (compute.as_mut(), cell.as_mut()) {
        (Some(c), Some(v)) => c(&mut **v),
        _ => false,
    };

    let mut rt = RUNTIME.lock();
    replace_current_observer(&mut rt, prev);
    let existed = if let Some(n) = rt.nodes.get_mut(id) {
        n.compute = compute;
        n.value = cell;
        n.flags.remove(NodeFlags::RUNNING);
        n.flags.set_color(NodeFlags::CLEAN);
        if changed {
            n.version = n.version.wrapping_add(1);
        }
        true
    } else {
        false // node was disposed mid-compute; drop compute + cell
    };
    if existed && changed {
        rt.global_version = rt.global_version.wrapping_add(1);
        propagate_change(&mut rt, id);
    }
}

/// Attaches a freshly created node to the current owner (effect or scope), if any
/// (§3.1). The owner's child list is created on demand.
fn attach_to_current_scope(rt: &mut Runtime, id: NodeId) {
    let Some(owner) = current_observer(rt).or_else(|| current_scope(rt)) else {
        return;
    };
    if owner == id {
        return;
    }
    if let Some(node) = rt.nodes.get_mut(id) {
        node.owner = Some(owner);
    }
    match rt.scopes.get_mut(owner) {
        Some(list) => list.push(id),
        None => {
            let mut list: SmallVec<[NodeId; 4]> = SmallVec::new();
            list.push(id);
            rt.scopes.insert(owner, list);
        }
    }
}

/// Recursively disposes `id` and everything it owns, unsubscribing graph edges and
/// removing the nodes from the arena (§3.1).
fn dispose_node(rt: &mut Runtime, id: NodeId) {
    let children = rt.scopes.get(id).cloned().unwrap_or_default();
    for child in children {
        dispose_node(rt, child);
    }
    rt.scopes.remove(id);

    // Detach from the graph so dangling ids never re-mark or re-run this node.
    if let Some(node) = rt.nodes.get(id) {
        let sources = node.sources.clone();
        let observers = node.observers.clone();
        for s in sources {
            if let Some(sn) = rt.nodes.get_mut(s) {
                if let Some(pos) = sn.observers.iter().position(|&x| x == id) {
                    sn.observers.swap_remove(pos);
                }
            }
        }
        for o in observers {
            if let Some(on) = rt.nodes.get_mut(o) {
                if let Some(pos) = on.sources.iter().position(|&x| x == id) {
                    on.sources.swap_remove(pos);
                }
            }
        }
    }
    if let Some(node) = rt.nodes.get_mut(id) {
        node.flags.insert(NodeFlags::DISPOSED);
    }
    rt.pending_effects.retain(|&x| x != id);
    rt.pending_subscriptions.retain(|&x| x != id);
    rt.nodes.remove(id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering::SeqCst};
    use std::sync::Arc;

    /// The reactive arena is process-global; serialize tests that create effects,
    /// memos, or flush so cross-test queued work never pollutes another test's
    /// pull phase (or its allocation measurement).
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// A `fn`-pointer equality gate over `i32` for the opt-in comparators.
    fn eq_i32(a: &dyn Any, b: &dyn Any) -> bool {
        a.downcast_ref::<i32>() == b.downcast_ref::<i32>()
    }

    #[test]
    fn node_flags_color_pack() {
        let mut f = NodeFlags::default();
        assert_eq!(f.color(), NodeFlags::CLEAN);
        f.insert(NodeFlags::TRACKING);
        f.set_color(NodeFlags::DIRTY);
        // color and status bits are orthogonal
        assert_eq!(f.color(), NodeFlags::DIRTY);
        assert!(f.contains(NodeFlags::TRACKING));
        f.set_color(NodeFlags::CHECK);
        assert_eq!(f.color(), NodeFlags::CHECK);
        assert!(f.contains(NodeFlags::TRACKING));
        f.remove(NodeFlags::TRACKING);
        assert!(!f.contains(NodeFlags::TRACKING));
        assert_eq!(f.color(), NodeFlags::CHECK);
    }

    #[test]
    fn signal_get_set_update() {
        let s = create_signal(1i32);
        assert_eq!(s.get(), 1);
        s.set(41);
        assert_eq!(s.get(), 41);
        s.update(|v| *v += 1);
        assert_eq!(s.get(), 42);
    }

    #[test]
    fn signal_with_reads_by_ref() {
        let s = create_signal(String::from("hi"));
        let len = s.with(|v| v.len());
        assert_eq!(len, 2);
        // value is restored after `with` lifts it out and back.
        assert_eq!(s.get(), "hi");
    }

    #[test]
    fn signal_transaction_returns_value_and_restores_after_panic() {
        let s = create_signal(vec![1]);
        assert_eq!(
            s.transaction(|values| {
                values.push(2);
                values.len()
            }),
            2
        );

        assert!(catch_unwind(AssertUnwindSafe(|| {
            s.transaction(|values| {
                values.push(3);
                panic!("expected transaction panic");
            });
        }))
        .is_err());
        assert_eq!(s.get(), vec![1, 2, 3]);
        assert_eq!(
            s.transaction(|values| {
                values.push(4);
                values.len()
            }),
            4
        );
    }

    #[test]
    fn transaction_and_peek_report_disposal_without_running_closures() {
        let scope = create_scope();
        let signal = run_in_scope(scope, || create_signal(7i32));
        scope.dispose();
        let calls = AtomicUsize::new(0);
        assert_eq!(
            signal.try_transaction(|value| {
                calls.fetch_add(1, SeqCst);
                *value += 1;
            }),
            None
        );
        assert_eq!(
            signal.try_peek(|value| {
                calls.fetch_add(1, SeqCst);
                *value
            }),
            None
        );
        assert_eq!(calls.load(SeqCst), 0);
    }

    #[test]
    fn run_in_scope_restores_the_calling_threads_owner_after_a_panic() {
        let _g = TEST_LOCK.lock();
        let scope = create_scope();
        let child = std::sync::Mutex::new(None);
        assert!(catch_unwind(AssertUnwindSafe(|| {
            run_in_scope(scope, || {
                *child.lock().unwrap() = Some(create_signal(1i32));
                panic!("expected scope panic");
            });
        }))
        .is_err());
        let unrelated = create_signal(2i32);
        scope.dispose();
        assert_eq!(child.lock().unwrap().unwrap().try_get(), None);
        assert_eq!(unrelated.try_get(), Some(2));
    }

    #[test]
    fn parallel_scopes_never_cross_own_or_dispose_each_others_signals() {
        let _g = TEST_LOCK.lock();
        let (first_installed_tx, first_installed_rx) = std::sync::mpsc::channel();
        let (second_installed_tx, second_installed_rx) = std::sync::mpsc::channel();
        let (first_created_tx, first_created_rx) = std::sync::mpsc::channel();

        let first = std::thread::spawn(move || {
            let scope = create_scope();
            let signal = run_in_scope(scope, || {
                first_installed_tx.send(()).unwrap();
                second_installed_rx.recv().unwrap();
                let signal = create_signal(10i32);
                first_created_tx.send(()).unwrap();
                signal
            });
            (scope, signal)
        });
        let second = std::thread::spawn(move || {
            first_installed_rx.recv().unwrap();
            let scope = create_scope();
            let signal = run_in_scope(scope, || {
                second_installed_tx.send(()).unwrap();
                first_created_rx.recv().unwrap();
                create_signal(20i32)
            });
            (scope, signal)
        });

        let (first_scope, first_signal) = first.join().unwrap();
        let (second_scope, second_signal) = second.join().unwrap();
        first_scope.dispose();
        assert_eq!(first_signal.try_get(), None);
        assert_eq!(second_signal.try_get(), Some(20));
        second_scope.dispose();
        assert_eq!(second_signal.try_get(), None);
    }

    #[test]
    fn peek_and_untrack_do_not_create_dependencies() {
        let _g = TEST_LOCK.lock();
        let source = create_signal(1i32);
        let via_peek = create_memo(move || source.peek(|value| *value));
        assert_eq!(via_peek.get(), 1);
        source.set(2);
        assert_eq!(via_peek.get(), 1);

        let tracked = create_signal(10i32);
        let untracked_source = create_signal(20i32);
        let mixed = create_memo(move || tracked.get() + untrack(|| untracked_source.get()));
        assert_eq!(mixed.get(), 30);
        untracked_source.set(30);
        assert_eq!(mixed.get(), 30);
        tracked.set(11);
        assert_eq!(mixed.get(), 41);
    }

    #[test]
    fn set_bumps_global_version() {
        let before = Runtime::global_version();
        let s = create_signal(0u8);
        s.set(5);
        assert!(Runtime::global_version() > before);
    }

    #[test]
    fn dispose_makes_try_get_none() {
        let scope = create_scope();
        // manually park a node under the scope to prove disposal semantics
        {
            let mut rt = RUNTIME.lock();
            let id = rt.nodes.insert(Node::signal(Box::new(7i32), None));
            rt.scopes.get_mut(scope.id).unwrap().push(id);
        }
        scope.dispose();
        // scope node itself is gone
        assert!(RUNTIME.lock().scopes.get(scope.id).is_none());
    }

    #[test]
    fn memo_recomputes_on_dep_change() {
        let _g = TEST_LOCK.lock();
        let s = create_signal(2i32);
        let m = create_memo(move || s.get() * 10);
        assert_eq!(m.get(), 20);
        s.set(3);
        assert_eq!(m.get(), 30);
    }

    #[test]
    fn effect_queued_on_push_drained_on_flush() {
        let _g = TEST_LOCK.lock();
        let s = create_signal(0i32);
        let seen = Arc::new(AtomicUsize::new(999));
        let _e = {
            let seen = seen.clone();
            create_effect(move || seen.store(s.get() as usize, SeqCst))
        };
        // first run is synchronous on creation
        assert_eq!(seen.load(SeqCst), 0);
        s.set(7);
        // queued, NOT yet run — the pull happens at flush (§3.1 / §7.5)
        assert_eq!(seen.load(SeqCst), 0);
        Runtime::flush();
        assert_eq!(seen.load(SeqCst), 7);
    }

    #[test]
    fn diamond_glitch_free_each_recomputed_once() {
        let _g = TEST_LOCK.lock();
        let s = create_signal(1i32);
        let (ca, cb, cd) = (
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        );
        let a = {
            let ca = ca.clone();
            create_memo(move || {
                ca.fetch_add(1, SeqCst);
                s.get() + 1
            })
        };
        let b = {
            let cb = cb.clone();
            create_memo(move || {
                cb.fetch_add(1, SeqCst);
                s.get() + 10
            })
        };
        let d = {
            let cd = cd.clone();
            create_memo(move || {
                cd.fetch_add(1, SeqCst);
                a.get() + b.get()
            })
        };

        assert_eq!(d.get(), (1 + 1) + (1 + 10));
        assert_eq!(
            (ca.load(SeqCst), cb.load(SeqCst), cd.load(SeqCst)),
            (1, 1, 1)
        );

        s.set(2);
        assert_eq!(d.get(), (2 + 1) + (2 + 10));
        // each node recomputed exactly once more — glitch-free diamond (§3.1)
        assert_eq!(
            (ca.load(SeqCst), cb.load(SeqCst), cd.load(SeqCst)),
            (2, 2, 2)
        );
    }

    #[test]
    fn signal_equals_prunes_propagation() {
        let _g = TEST_LOCK.lock();
        let s = create_signal_equals(5i32, eq_i32);
        let runs = Arc::new(AtomicUsize::new(0));
        let _e = {
            let runs = runs.clone();
            create_effect(move || {
                let _ = s.get();
                runs.fetch_add(1, SeqCst);
            })
        };
        assert_eq!(runs.load(SeqCst), 1); // first run on creation
        s.set(5); // equal → gate suppresses the whole push
        Runtime::flush();
        assert_eq!(runs.load(SeqCst), 1);
        s.set(6); // real change fires
        Runtime::flush();
        assert_eq!(runs.load(SeqCst), 2);
    }

    #[test]
    fn memo_equals_stops_propagation() {
        let _g = TEST_LOCK.lock();
        let s = create_signal(0i32);
        let parity = create_memo_equals(move || s.get() % 2, eq_i32);
        let runs = Arc::new(AtomicUsize::new(0));
        let _e = {
            let runs = runs.clone();
            create_effect(move || {
                let _ = parity.get();
                runs.fetch_add(1, SeqCst);
            })
        };
        assert_eq!(runs.load(SeqCst), 1);

        s.set(2); // 0 → still even: memo recomputes but value is unchanged
        Runtime::flush();
        // the memo boundary prunes: the effect must NOT re-run (§3.1)
        assert_eq!(runs.load(SeqCst), 1);

        s.set(3); // parity flips → propagation resumes
        Runtime::flush();
        assert_eq!(runs.load(SeqCst), 2);
    }

    #[test]
    fn dynamic_dependencies_resubscribe() {
        let _g = TEST_LOCK.lock();
        let cond = create_signal(true);
        let x = create_signal(10i32);
        let y = create_signal(20i32);
        let count = Arc::new(AtomicUsize::new(0));
        let m = {
            let count = count.clone();
            create_memo(move || {
                count.fetch_add(1, SeqCst);
                if cond.get() {
                    x.get()
                } else {
                    y.get()
                }
            })
        };

        assert_eq!(m.get(), 10);
        assert_eq!(count.load(SeqCst), 1);

        // y is not a dependency yet → changing it must not recompute m.
        y.set(21);
        assert_eq!(m.get(), 10);
        assert_eq!(count.load(SeqCst), 1);

        // x IS a dependency → recompute.
        x.set(11);
        assert_eq!(m.get(), 11);
        assert_eq!(count.load(SeqCst), 2);

        // flip the branch: m now reads y and unsubscribes x.
        cond.set(false);
        assert_eq!(m.get(), 21);
        assert_eq!(count.load(SeqCst), 3);

        // x is no longer a dependency → no recompute.
        x.set(100);
        assert_eq!(m.get(), 21);
        assert_eq!(count.load(SeqCst), 3);

        // y is a dependency now.
        y.set(22);
        assert_eq!(m.get(), 22);
        assert_eq!(count.load(SeqCst), 4);
    }

    #[test]
    fn disposal_safety() {
        let _g = TEST_LOCK.lock();

        // A disposed signal reads back as None, never panics.
        let scope = create_scope();
        let s = run_in_scope(scope, || create_signal(5i32));
        assert_eq!(s.try_get(), Some(5));
        scope.dispose();
        assert_eq!(s.try_get(), None);

        // Disposing a queued effect, then flushing, is safe and does not re-run it.
        let s2 = create_signal(0i32);
        let scope2 = create_scope();
        let runs = Arc::new(AtomicUsize::new(0));
        let _e = {
            let runs = runs.clone();
            run_in_scope(scope2, || {
                create_effect(move || {
                    let _ = s2.try_get();
                    runs.fetch_add(1, SeqCst);
                })
            })
        };
        assert_eq!(runs.load(SeqCst), 1);
        s2.set(1); // queues the effect
        scope2.dispose(); // removes it from the arena and the queue
        Runtime::flush(); // must not panic on the dangling id
        assert_eq!(runs.load(SeqCst), 1); // and must not re-run the disposed effect
    }

    #[test]
    fn subscription_direct_dependencies_are_isolated() {
        let _g = TEST_LOCK.lock();
        let left = create_signal(0i32);
        let right = create_signal(0i32);
        let group = SubscriptionGroup::new();
        let left_sub = group.subscribe(10);
        let right_sub = group.subscribe(20);
        left_sub.track(|| left.get());
        right_sub.track(|| right.get());

        left.set(1);
        Runtime::flush();
        let mut ready = Vec::new();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![10]);

        right.set(1);
        Runtime::flush();
        ready.clear();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![20]);
    }

    #[test]
    fn subscriptions_track_independently_on_parallel_threads() {
        let _g = TEST_LOCK.lock();
        let left = create_signal(0i32);
        let right = create_signal(0i32);
        let overlap = Arc::new(std::sync::Barrier::new(2));

        let run = |source: Signal<i32>, key: u64, overlap: Arc<std::sync::Barrier>| {
            std::thread::spawn(move || {
                let group = SubscriptionGroup::new();
                let subscription = group.subscribe(key);
                subscription.track(|| {
                    // Both observer contexts are installed before either read.
                    overlap.wait();
                    source.get()
                });
                source.set(1);
                Runtime::flush();
                let mut ready = Vec::new();
                group.drain_ready_into(&mut ready);
                ready
            })
        };

        let left_thread = run(left, 10, overlap.clone());
        let right_thread = run(right, 20, overlap);
        assert_eq!(left_thread.join().unwrap(), vec![10]);
        assert_eq!(right_thread.join().unwrap(), vec![20]);
    }

    #[test]
    fn concurrent_flush_waits_for_already_drained_work() {
        let _g = TEST_LOCK.lock();
        let source = create_signal(0i32);
        let blocker = create_signal(0i32);
        let group = SubscriptionGroup::new();
        let subscription = group.subscribe(77);
        subscription.track(|| source.get());

        let entered = Arc::new(std::sync::Barrier::new(2));
        let release = Arc::new(std::sync::Barrier::new(2));
        let runs = Arc::new(AtomicUsize::new(0));
        let scope = create_scope();
        let _effect = run_in_scope(scope, || {
            let entered = entered.clone();
            let release = release.clone();
            let runs = runs.clone();
            create_effect(move || {
                blocker.get();
                if runs.fetch_add(1, SeqCst) > 0 {
                    entered.wait();
                    release.wait();
                }
            })
        });

        blocker.set(1);
        source.set(1);
        let first = std::thread::spawn(Runtime::flush);
        entered.wait();

        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            Runtime::flush();
            finished_tx.send(()).unwrap();
        });
        assert!(finished_rx
            .recv_timeout(std::time::Duration::from_millis(20))
            .is_err());

        release.wait();
        first.join().unwrap();
        second.join().unwrap();
        finished_rx.recv().unwrap();

        let mut ready = Vec::new();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![77]);
        scope.dispose();
    }

    #[test]
    fn subscription_memo_equality_prunes_ready_key() {
        let _g = TEST_LOCK.lock();
        let source = create_signal(0i32);
        let parity = create_memo_equals(move || source.get() % 2, eq_i32);
        let group = SubscriptionGroup::new();
        let sub = group.subscribe(7);
        sub.track(|| parity.get());

        source.set(2); // memo is rechecked, but retains the equal value
        Runtime::flush();
        let mut ready = Vec::new();
        group.drain_ready_into(&mut ready);
        assert!(ready.is_empty());

        source.set(3);
        Runtime::flush();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![7]);
    }

    #[test]
    fn subscription_retracks_dynamic_dependencies() {
        let _g = TEST_LOCK.lock();
        let condition = create_signal(true);
        let left = create_signal(1i32);
        let right = create_signal(2i32);
        let group = SubscriptionGroup::new();
        let sub = group.subscribe(42);
        let track = || {
            sub.track(|| {
                if condition.get() {
                    left.get()
                } else {
                    right.get()
                }
            });
        };
        track();

        right.set(3);
        Runtime::flush();
        let mut ready = Vec::new();
        group.drain_ready_into(&mut ready);
        assert!(ready.is_empty());

        left.set(4);
        Runtime::flush();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![42]);

        // Consume and rebuild the producer: its source switches from left to right.
        track();
        condition.set(false);
        Runtime::flush();
        ready.clear();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![42]);
        track();

        left.set(5);
        Runtime::flush();
        ready.clear();
        group.drain_ready_into(&mut ready);
        assert!(ready.is_empty());

        right.set(6);
        Runtime::flush();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![42]);
    }

    #[test]
    fn subscription_coalesces_until_retracked() {
        let _g = TEST_LOCK.lock();
        let source = create_signal(0i32);
        let group = SubscriptionGroup::new();
        let sub = group.subscribe(9);
        sub.track(|| source.get());
        let mut ready = Vec::new();

        source.set(1);
        Runtime::flush();
        source.set(2);
        Runtime::flush();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![9]);

        // Even after draining, this producer remains coalesced until its owner
        // has read it again.
        source.set(3);
        Runtime::flush();
        ready.clear();
        group.drain_ready_into(&mut ready);
        assert!(ready.is_empty());

        sub.track(|| source.get());
        source.set(4);
        Runtime::flush();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![9]);
    }

    #[test]
    fn subscription_disposal_removes_edges_and_ready_work() {
        let _g = TEST_LOCK.lock();
        let source = create_signal(0i32);
        let group = SubscriptionGroup::new();
        let sub = group.subscribe(11);
        sub.track(|| source.get());
        source.set(1);
        Runtime::flush();
        drop(sub); // removes both the source edge and the undrained key

        let mut ready = Vec::new();
        group.drain_ready_into(&mut ready);
        assert!(ready.is_empty());
        source.set(2);
        Runtime::flush();
        group.drain_ready_into(&mut ready);
        assert!(ready.is_empty());
    }

    #[test]
    fn subscription_group_unsubscribe_invalidates_clones_and_clear_reuses_group() {
        let _g = TEST_LOCK.lock();
        let source = create_signal(0i32);
        let group = SubscriptionGroup::new();
        let sub = group.subscribe(11);
        let clone = sub.clone();
        sub.track(|| source.get());
        group.unsubscribe(sub);

        source.set(1);
        Runtime::flush();
        let mut ready = Vec::new();
        group.drain_ready_into(&mut ready);
        assert!(ready.is_empty());
        // A stale clone is harmless and cannot reinstall graph edges.
        assert_eq!(clone.track(|| source.get()), 1);

        let replacement = group.subscribe(12);
        replacement.track(|| source.get());
        group.clear();
        let reused = group.subscribe(13);
        reused.track(|| source.get());
        source.set(2);
        Runtime::flush();
        group.drain_ready_into(&mut ready);
        assert_eq!(ready, vec![13]);
    }

    #[cfg(feature = "count-allocations")]
    #[test]
    fn second_update_allocates_nothing() {
        let _g = TEST_LOCK.lock();
        let s = create_signal(0i32);
        let out = Arc::new(AtomicI32::new(-1));
        let _e = {
            let out = out.clone();
            create_effect(move || out.store(s.get(), SeqCst))
        };

        // Warm: the first set + flush is allowed to grow pools (§4.1).
        s.set(1);
        Runtime::flush();
        assert_eq!(out.load(SeqCst), 1);

        // Measure the SECOND set + flush: the steady-state re-render path.
        let info = allocation_counter::measure(|| {
            s.set(2);
            Runtime::flush();
        });
        assert_eq!(info.count_total, 0, "allocations on steady-state update");
        assert_eq!(info.bytes_total, 0, "bytes on steady-state update");
        assert_eq!(out.load(SeqCst), 2);
    }

    // Silence unused import when the alloc feature is off.
    #[cfg(not(feature = "count-allocations"))]
    #[allow(dead_code)]
    fn _uses_atomic_i32(_: AtomicI32) {}
}
