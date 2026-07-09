//! # schnellui-store
//!
//! A small, selector-oriented facade over [`schnellui_signal`]. [`Store`] splits
//! its concerns deliberately: a tiny revision [`Signal`] owns reactive dependency
//! tracking and disposal, while its payload lives behind a shared [`RwLock`]. This
//! lets nested closure reads borrow the same store safely without lifting the
//! complete application state out of the reactive arena. Each [`Selector`] is one
//! equality-gated [`Memo`], so changing unrelated store fields recomputes its
//! selector but only a changed projection reaches consumers.
//!
//! ## Lifetime and threading
//!
//! `Store` and `Selector` are cheap `Clone` handles into schnellui-signal's
//! process-global arena. Their shared internal scope keeps the root signal and
//! all selectors alive until the final handle is dropped. A store made inside an
//! outer [`schnellui_signal::Scope`] is also a child of that outer scope; after
//! the outer scope is disposed, [`Store::try_get`] and [`Selector::try_get`]
//! return `None`. Panicking reads retain the signal crate's semantics and panic
//! after disposal.
//!
//! The shared signal runtime requires store data and selector values to be
//! `Send + Sync + 'static`. Selectors must also be `Clone` because memo reads
//! clone their cached result. The default [`Store::select`] equality boundary uses
//! [`PartialEq`], so it suppresses downstream propagation when the projection is
//! unchanged.
//!
//! ```
//! use schnellui_store::Store;
//!
//! #[derive(Clone)]
//! struct AppState {
//!     title: String,
//!     clicks: usize,
//! }
//!
//! let store = Store::new(AppState {
//!     title: "Draft".into(),
//!     clicks: 0,
//! });
//! let clicks = store.select(|state| state.clicks);
//!
//! store.update(|state| state.clicks += 1);
//! assert_eq!(clicks.get(), 1);
//! ```

use std::any::Any;
use std::{
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::Arc,
};

use parking_lot::RwLock;
use schnellui_signal::{
    create_memo_equals, create_scope, create_signal, run_in_scope, Memo, Scope, Signal,
};

/// A shared application-state handle backed by one [`Signal`].
///
/// Cloning the handle refers to the same reactive value. Use
/// [`Store::select`] to subscribe consumers to a projection rather than the
/// complete state value.
pub struct Store<T: 'static> {
    /// Reactive invalidation and disposal identity. It intentionally stores no
    /// application payload, so a tracked read never prevents a nested payload
    /// read from taking another shared lock.
    revision: Signal<()>,
    data: Arc<RwLock<T>>,
    owner: Arc<ScopeOwner>,
}

impl<T: 'static> Clone for Store<T> {
    fn clone(&self) -> Self {
        Self {
            revision: self.revision,
            data: Arc::clone(&self.data),
            owner: Arc::clone(&self.owner),
        }
    }
}

impl<T: Send + Sync + 'static> Store<T> {
    /// Creates a store containing `value`.
    ///
    /// Like [`Signal::set`], [`Store::set`] always notifies direct consumers;
    /// selectors still prune consumers whose projections did not change.
    #[must_use]
    pub fn new(value: T) -> Self {
        let owner = Arc::new(ScopeOwner {
            scope: create_scope(),
        });
        let revision = run_in_scope(owner.scope, || create_signal(()));
        Self {
            revision,
            data: Arc::new(RwLock::new(value)),
            owner,
        }
    }

    /// Returns whether two handles refer to the same store value.
    #[inline]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.owner, &other.owner)
    }

    /// Reads the current state by reference for the duration of `reader`.
    ///
    /// This is the closure-oriented spelling used by application models. It is
    /// equivalent to [`Store::with`], and records a reactive dependency when it
    /// runs inside a memo or effect.
    pub fn read<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.with(reader)
    }

    /// Clones the current state, recording a reactive dependency when applicable.
    ///
    /// Panics if the signal's owning scope has been disposed; use
    /// [`Store::try_get`] when disposal is expected.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.try_get().expect("Store::get on a disposed store")
    }

    /// Clones the current state, or returns `None` after its owning scope is
    /// disposed. Records a reactive dependency when applicable.
    pub fn try_get(&self) -> Option<T>
    where
        T: Clone,
    {
        self.revision.try_get()?;
        Some(self.data.read().clone())
    }

    /// Reads the current state by reference without cloning it.
    ///
    /// The signal runtime releases its lock before running `reader`, so the
    /// closure may safely access other stores or signals. Panics after disposal.
    pub fn with<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.revision.get();
        reader(&self.data.read())
    }

    /// Reads the current state by reference without subscribing the current
    /// memo, effect, or manual producer to the whole store.
    ///
    /// Use this to fetch a payload after depending on a narrower [`Selector`].
    /// The runtime lock is released before `reader` runs. Panics after disposal;
    /// use [`Store::try_peek`] when disposal is expected.
    pub fn peek<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.try_peek(reader)
            .expect("Store::peek on a disposed store")
    }

    /// Untracked closure read, returning `None` after disposal.
    pub fn try_peek<R>(&self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        self.revision.try_peek(|_| ())?;
        Some(reader(&self.data.read()))
    }

    /// Alias for [`Store::peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn read_untracked<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.peek(reader)
    }

    /// Alias for [`Store::try_peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn try_read_untracked<R>(&self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        self.try_peek(reader)
    }

    /// Replaces the state and marks direct reactive consumers dirty.
    ///
    /// Setting a value equal to the previous value still notifies because the
    /// underlying store signal intentionally has no equality gate. Prefer
    /// selectors for fine-grained propagation pruning.
    pub fn set(&self, value: T) {
        if self.revision.try_peek(|_| ()).is_none() {
            return;
        }
        *self.data.write() = value;
        self.revision.set(());
    }

    /// Mutates the state in place and marks direct reactive consumers dirty.
    ///
    /// The closure runs outside the reactive runtime lock. As with
    /// [`Signal::update`], updating a disposed store is a no-op.
    pub fn update(&self, update: impl FnOnce(&mut T)) {
        let _ = self.try_transaction(update);
    }

    /// Mutates the state in place and returns the closure's result without
    /// cloning the store value.
    ///
    /// The write guard is released before a panic resumes, so the store remains
    /// usable after a panicking mutation. Panics after disposal; use
    /// [`Store::try_transaction`] when disposal is expected.
    pub fn transaction<R>(&self, transaction: impl FnOnce(&mut T) -> R) -> R {
        self.try_transaction(transaction)
            .expect("Store::transaction on a disposed store")
    }

    /// Mutates the state in place and returns the closure's result, or `None`
    /// without calling `transaction` after the store has been disposed.
    pub fn try_transaction<R>(&self, transaction: impl FnOnce(&mut T) -> R) -> Option<R> {
        if self.revision.try_peek(|_| ()).is_none() {
            return None;
        }
        let result = {
            let mut data = self.data.write();
            catch_unwind(AssertUnwindSafe(|| transaction(&mut data)))
        };
        // A partially completed mutation remains observable after an unwind,
        // exactly like an ordinary mutable borrow. Notify before resuming the
        // panic so consumers cannot retain a stale projection.
        let alive = self.revision.try_peek(|_| ()).is_some();
        if alive {
            self.revision.set(());
        }
        match result {
            Ok(value) if alive => Some(value),
            Ok(_) => None,
            Err(payload) => resume_unwind(payload),
        }
    }

    /// Creates a lazily evaluated projection of this store.
    ///
    /// `selector` runs whenever this projection must be checked. Its cached
    /// result is compared with [`PartialEq`]; when it is unchanged, the memo
    /// prevents propagation to effects and other consumers that read this
    /// selector. The selector shares the store's signal scope semantics.
    #[must_use]
    pub fn select<U>(&self, selector: impl Fn(&T) -> U + Send + 'static) -> Selector<U>
    where
        U: Send + Sync + Clone + PartialEq + 'static,
    {
        let revision = self.revision;
        let data = Arc::clone(&self.data);
        Selector {
            memo: run_in_scope(self.owner.scope, || {
                create_memo_equals(
                    move || {
                        revision.get();
                        selector(&data.read())
                    },
                    partial_eq::<U>,
                )
            }),
            owner: Arc::clone(&self.owner),
        }
    }
}

/// A cached, equality-gated projection created by [`Store::select`].
///
/// It is a cloneable handle over a signal [`Memo`]. Reading it subscribes the
/// current memo or effect, if any, to only this projected value.
pub struct Selector<T: 'static> {
    memo: Memo<T>,
    owner: Arc<ScopeOwner>,
}

impl<T: 'static> Clone for Selector<T> {
    fn clone(&self) -> Self {
        Self {
            memo: self.memo,
            owner: Arc::clone(&self.owner),
        }
    }
}

impl<T: 'static> Selector<T> {
    /// Clones the projected value, pulling the memo cache first.
    ///
    /// Panics if its owning scope has been disposed; use [`Selector::try_get`]
    /// when disposal is expected.
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        self.memo.get()
    }

    /// Clones the projected value, or returns `None` after its owning scope is
    /// disposed.
    pub fn try_get(&self) -> Option<T>
    where
        T: Clone,
    {
        self.memo.try_get()
    }

    /// Reads the projected value by reference without cloning it.
    ///
    /// Pulls the memo cache before calling `reader`; the runtime lock is released
    /// while user code runs. Panics after disposal.
    pub fn with<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.memo.with(reader)
    }

    /// Reads the settled projection without subscribing the current reactive
    /// observer to it. Panics after disposal; use [`Selector::try_peek`] when
    /// disposal is expected.
    pub fn peek<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.memo.peek(reader)
    }

    /// Untracked projection read, returning `None` after disposal.
    pub fn try_peek<R>(&self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        self.memo.try_peek(reader)
    }

    /// Alias for [`Selector::peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn read_untracked<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.peek(reader)
    }

    /// Alias for [`Selector::try_peek`] that makes the dependency behavior explicit.
    #[inline]
    pub fn try_read_untracked<R>(&self, reader: impl FnOnce(&T) -> R) -> Option<R> {
        self.try_peek(reader)
    }
}

/// Shared RAII owner for a store's signal scope.
///
/// The owner intentionally sits outside the reactive arena. A memo compute only
/// captures its raw [`Signal`] handle, never this `Arc`, so dropping every public
/// store/selector handle deterministically disposes the scope instead of forming
/// an arena-retained reference cycle.
struct ScopeOwner {
    scope: Scope,
}

impl Drop for ScopeOwner {
    fn drop(&mut self) {
        self.scope.dispose();
    }
}

fn partial_eq<T: PartialEq + 'static>(left: &dyn Any, right: &dyn Any) -> bool {
    left.downcast_ref::<T>() == right.downcast_ref::<T>()
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use schnellui_signal::{create_effect, create_memo, create_scope, run_in_scope, Runtime};

    use super::Store;

    // The reactive arena is process-global. Serializing the tests prevents one
    // test's queued effect from being flushed by another test.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct AppState {
        title: String,
        clicks: usize,
    }

    #[test]
    fn store_supports_cloned_and_closure_reads() {
        let _serial = TEST_LOCK.lock().unwrap();
        let store = Store::new(AppState {
            title: "Draft".into(),
            clicks: 0,
        });

        assert_eq!(store.get().title, "Draft");
        assert_eq!(store.try_get().map(|state| state.clicks), Some(0));
        assert_eq!(store.read(|state| state.title.len()), 5);
        assert_eq!(store.with(|state| state.clicks), 0);

        store.set(AppState {
            title: "Published".into(),
            clicks: 3,
        });
        store.update(|state| state.clicks += 1);

        assert_eq!(
            store.read(|state| (state.title.clone(), state.clicks)),
            (String::from("Published"), 4)
        );
    }

    #[test]
    fn nested_store_reads_share_the_payload_without_lifting_it_from_the_store() {
        let _serial = TEST_LOCK.lock().unwrap();
        let store = Store::new(AppState {
            title: "Nested".into(),
            clicks: 3,
        });
        let nested = store.read(|outer| {
            assert_eq!(outer.title, "Nested");
            store.read(|inner| (inner.title.clone(), inner.clicks))
        });

        assert_eq!(nested, (String::from("Nested"), 3));
    }

    #[test]
    fn selectors_can_nest_payload_reads_without_capturing_the_store_owner() {
        let _serial = TEST_LOCK.lock().unwrap();
        let store = Store::new(AppState {
            title: "Selector".into(),
            clicks: 4,
        });
        // A selector must capture only the payload handle, never `Store`'s
        // scope owner. Reading the same RwLock under its outer selector read
        // also proves nested shared payload reads remain valid.
        let nested_data = Arc::clone(&store.data);
        let combined = store.select(move |state| state.clicks + nested_data.read().clicks);

        assert_eq!(combined.get(), 8);
        store.transaction(|state| state.clicks = 5);
        assert_eq!(combined.get(), 10);
    }

    #[test]
    fn transaction_returns_result_without_cloning_the_state() {
        let _serial = TEST_LOCK.lock().unwrap();
        let store = Store::new(AppState {
            title: "Draft".into(),
            clicks: 0,
        });
        let clone = store.clone();

        let previous_title = store.transaction(|state| {
            state.clicks += 1;
            std::mem::replace(&mut state.title, "Published".into())
        });

        assert_eq!(previous_title, "Draft");
        assert_eq!(
            store.read(|state| (state.title.clone(), state.clicks)),
            (String::from("Published"), 1)
        );
        assert!(store.ptr_eq(&clone));
        assert!(!store.ptr_eq(&Store::new(AppState {
            title: "Published".into(),
            clicks: 1,
        })));
    }

    #[test]
    fn transaction_restores_state_after_panic_and_try_variant_handles_disposal() {
        let _serial = TEST_LOCK.lock().unwrap();
        let store = Store::new(AppState {
            title: "Draft".into(),
            clicks: 0,
        });

        assert!(catch_unwind(AssertUnwindSafe(|| {
            store.transaction(|state| {
                state.clicks = 7;
                panic!("expected transaction panic");
            });
        }))
        .is_err());
        // A panic does not strand the lifted cell outside the arena. Like an
        // ordinary mutable borrow, work completed before the panic remains.
        assert_eq!(store.read(|state| state.clicks), 7);
        assert_eq!(
            store.transaction(|state| {
                state.clicks += 1;
                state.clicks
            }),
            8
        );

        let scope = create_scope();
        let disposed = run_in_scope(scope, || {
            Store::new(AppState {
                title: "Scoped".into(),
                clicks: 0,
            })
        });
        scope.dispose();
        assert_eq!(disposed.try_transaction(|state| state.clicks += 1), None);
        assert_eq!(disposed.try_peek(|state| state.clicks), None);
        assert!(catch_unwind(AssertUnwindSafe(|| {
            disposed.transaction(|state| state.clicks += 1);
        }))
        .is_err());
    }

    #[test]
    fn peek_keeps_dynamic_producers_off_the_root_store() {
        let _serial = TEST_LOCK.lock().unwrap();
        let store = Store::new(AppState {
            title: "Draft".into(),
            clicks: 0,
        });
        let clicks = store.select(|state| state.clicks);
        let runs = Arc::new(AtomicUsize::new(0));
        let payload = {
            let store = store.clone();
            let runs = Arc::clone(&runs);
            create_memo(move || {
                runs.fetch_add(1, Ordering::SeqCst);
                let clicks = clicks.get();
                store.read_untracked(|state| format!("{}:{clicks}", state.title))
            })
        };

        assert_eq!(payload.get(), "Draft:0");
        store.transaction(|state| state.title = "Published".into());
        // The root-store write checks `clicks`, whose equal projection prunes
        // this memo. The untracked payload read has not added a root edge.
        assert_eq!(payload.get(), "Draft:0");
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        store.transaction(|state| state.clicks += 1);
        assert_eq!(payload.get(), "Published:1");
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn selector_prunes_unrelated_and_equal_projections() {
        let _serial = TEST_LOCK.lock().unwrap();
        let store = Store::new(AppState {
            title: "Draft".into(),
            clicks: 0,
        });
        let title = store.select(|state| state.title.clone());
        let runs = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(String::new()));
        let effect_scope = create_scope();
        let _effect = run_in_scope(effect_scope, || {
            let runs = Arc::clone(&runs);
            let observed = Arc::clone(&observed);
            create_effect(move || {
                *observed.lock().unwrap() = title.get();
                runs.fetch_add(1, Ordering::SeqCst);
            })
        });

        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(&*observed.lock().unwrap(), "Draft");

        store.update(|state| state.clicks += 1);
        Runtime::flush();
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        // The whole store changed, but this projection is still equal.
        store.set(AppState {
            title: "Draft".into(),
            clicks: 99,
        });
        Runtime::flush();
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        store.update(|state| state.title = "Published".into());
        Runtime::flush();
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert_eq!(&*observed.lock().unwrap(), "Published");

        // Effects retain what their closures capture; scope the test consumer so
        // it releases its Selector and the Store's RAII owner deterministically.
        effect_scope.dispose();
    }

    #[test]
    fn selector_outlives_its_store_and_final_handle_disposes_scope() {
        let _serial = TEST_LOCK.lock().unwrap();
        let before = Runtime::node_count();
        let title = {
            let store = Store::new(AppState {
                title: "Owned".into(),
                clicks: 0,
            });
            let title = store.select(|state| state.title.clone());

            // The store creates one scope, one source signal and one memo.
            assert_eq!(Runtime::node_count(), before + 3);
            title
        };

        // The selector keeps the shared owner alive after its Store is dropped.
        assert_eq!(title.get(), "Owned");
        assert_eq!(Runtime::node_count(), before + 3);

        drop(title);
        assert_eq!(Runtime::node_count(), before);
    }

    #[test]
    fn outer_scope_disposal_invalidates_store_and_selector() {
        let _serial = TEST_LOCK.lock().unwrap();
        let scope = create_scope();
        let store = run_in_scope(scope, || {
            Store::new(AppState {
                title: "Scoped".into(),
                clicks: 0,
            })
        });
        // This is deliberately outside `run_in_scope`: Store selects into its
        // retained internal scope, not whichever scope is currently active.
        let title = store.select(|state| state.title.clone());
        assert_eq!(title.get(), "Scoped");

        scope.dispose();

        assert_eq!(store.try_get(), None);
        assert_eq!(title.try_get(), None);
    }

    #[test]
    fn parallel_outer_scopes_cannot_cross_own_stores() {
        let _serial = TEST_LOCK.lock().unwrap();
        let (first_installed_tx, first_installed_rx) = std::sync::mpsc::channel();
        let (second_installed_tx, second_installed_rx) = std::sync::mpsc::channel();
        let (first_created_tx, first_created_rx) = std::sync::mpsc::channel();

        let first = std::thread::spawn(move || {
            let scope = create_scope();
            let store = run_in_scope(scope, || {
                first_installed_tx.send(()).unwrap();
                second_installed_rx.recv().unwrap();
                let store = Store::new(AppState {
                    title: "First".into(),
                    clicks: 1,
                });
                first_created_tx.send(()).unwrap();
                store
            });
            (scope, store)
        });
        let second = std::thread::spawn(move || {
            first_installed_rx.recv().unwrap();
            let scope = create_scope();
            let store = run_in_scope(scope, || {
                second_installed_tx.send(()).unwrap();
                first_created_rx.recv().unwrap();
                Store::new(AppState {
                    title: "Second".into(),
                    clicks: 2,
                })
            });
            (scope, store)
        });

        let (first_scope, first_store) = first.join().unwrap();
        let (second_scope, second_store) = second.join().unwrap();
        first_scope.dispose();
        assert_eq!(first_store.try_get(), None);
        assert_eq!(second_store.try_get().map(|state| state.clicks), Some(2));
        second_scope.dispose();
        assert_eq!(second_store.try_get(), None);
    }
}
