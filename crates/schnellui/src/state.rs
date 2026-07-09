//! Application-owned state for retained UI callbacks.
//!
//! [`State`] is deliberately local to the UI thread.  It gives `'static` widget
//! callbacks an owned handle without moving application data into a process or
//! thread global.

use std::cell::{BorrowError, BorrowMutError, RefCell};
use std::fmt;
use std::rc::Rc;

/// A cloneable handle to application-owned, UI-thread state.
///
/// Cloning this type clones only the handle. The contained value is dropped when
/// the last handle is dropped. Access is closure-based so a borrow cannot escape
/// into retained widget code.
pub struct State<T>(Rc<RefCell<T>>);

impl<T> State<T> {
    /// Creates a new independently owned state value.
    pub fn new(value: T) -> Self {
        Self(Rc::new(RefCell::new(value)))
    }

    /// Reads the value for the duration of `reader`.
    ///
    /// Panics when the same state is already mutably borrowed. Use
    /// [`try_read`](Self::try_read) when re-entrancy is expected.
    pub fn read<R>(&self, reader: impl FnOnce(&T) -> R) -> R {
        self.try_read(reader)
            .expect("schnellui State read while the value is mutably borrowed")
    }

    /// Mutates the value for the duration of `update`.
    ///
    /// Panics when the same state is already borrowed. Keep updates small and do
    /// not invoke callbacks that access this state from inside `update`.
    pub fn update<R>(&self, update: impl FnOnce(&mut T) -> R) -> R {
        self.try_update(update)
            .expect("schnellui State update while the value is already borrowed")
    }

    /// Attempts to read without panicking on a conflicting borrow.
    pub fn try_read<R>(&self, reader: impl FnOnce(&T) -> R) -> Result<R, BorrowError> {
        self.0.try_borrow().map(|value| reader(&value))
    }

    /// Attempts to mutate without panicking on a conflicting borrow.
    pub fn try_update<R>(&self, update: impl FnOnce(&mut T) -> R) -> Result<R, BorrowMutError> {
        self.0.try_borrow_mut().map(|mut value| update(&mut value))
    }

    /// Returns whether two handles refer to the same state value.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl<T> Clone for State<T> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<T: fmt::Debug> fmt::Debug for State<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.try_borrow() {
            Ok(value) => formatter.debug_tuple("State").field(&*value).finish(),
            Err(_) => formatter.write_str("State(<borrowed>)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::State;

    #[test]
    fn clones_share_one_owned_value() {
        let state = State::new(vec![1]);
        let callback_handle = state.clone();

        callback_handle.update(|values| values.push(2));

        assert_eq!(state.read(Clone::clone), vec![1, 2]);
        assert!(state.ptr_eq(&callback_handle));
    }

    #[test]
    fn separately_created_values_are_isolated() {
        let first = State::new(1);
        let second = State::new(1);

        first.update(|value| *value += 1);

        assert_eq!(first.read(|value| *value), 2);
        assert_eq!(second.read(|value| *value), 1);
        assert!(!first.ptr_eq(&second));
    }

    #[test]
    fn conflicting_access_is_reported() {
        let state = State::new(1);

        state.update(|_| {
            assert!(state.try_read(|value| *value).is_err());
            assert!(state.try_update(|value| *value += 1).is_err());
        });
    }
}
