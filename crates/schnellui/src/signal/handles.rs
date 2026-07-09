//! The `Copy` handles ([`Signal`], [`Memo`], [`Effect`]) and their constructors.

use super::*;

/// A reactive value cell. `Copy` handle carrying only a [`NodeId`] (SOUL §3.1).
pub struct Signal<T: 'static> {
    pub(crate) id: NodeId,
    pub(crate) _pd: PhantomData<fn() -> T>,
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
            rt.nodes.get_mut(self.id).and_then(|n| n.value.take())
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
            Some(n) => n,
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
        // Reuse the existing allocation; the old value is dropped after the lock
        // is released because its `Drop` may re-enter the runtime.
        let old = std::mem::replace(slot, value);
        node.version = node.version.wrapping_add(1);
        rt.global_version = rt.global_version.wrapping_add(1);
        propagate_change(&mut rt, self.id);
        drop(rt);
        drop(old);
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
                Some(n) => n,
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
    pub(crate) id: NodeId,
    pub(crate) _pd: PhantomData<fn() -> T>,
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
        let mut rt = settle(self.id);
        track_read(&mut rt, self.id);
        let node = rt.nodes.get(self.id)?;
        node.value
            .as_ref()
            .and_then(|v| v.downcast_ref::<Option<T>>())
            .and_then(|opt| opt.clone())
    }

    /// Reads by reference through a closure without cloning `T`. Settles the memo,
    /// lifts the cell out, releases the lock, then runs `f` (§3.1 borrow-safety).
    pub fn with<R>(self, f: impl FnOnce(&T) -> R) -> R {
        let cell = {
            let mut rt = settle(self.id);
            track_read(&mut rt, self.id);
            rt.nodes
                .get_mut(self.id)
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
        let cell = {
            let mut rt = settle(self.id);
            rt.nodes.get_mut(self.id).and_then(|n| n.value.take())
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
    pub(crate) id: NodeId,
}

impl Effect {
    /// The underlying arena key.
    #[inline]
    pub fn id(self) -> NodeId {
        self.id
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

pub(crate) fn make_memo<T: Send + Sync + Clone + 'static>(
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
