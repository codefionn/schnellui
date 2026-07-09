//! Explicit, immutable dependency scopes for view construction.

use std::any::{type_name, Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// An immutable, cloneable collection of typed values passed explicitly through
/// view construction.
///
/// Derived contexts retain their parent and store only locally provided values.
/// Looking up a type walks from the child toward the root, so a child can shadow
/// one value without mutating its parent or siblings.
#[derive(Clone, Default)]
pub struct Context(Rc<ContextScope>);

#[derive(Default)]
struct ContextScope {
    parent: Option<Context>,
    values: HashMap<TypeId, Box<dyn Any>>,
}

impl Context {
    /// Creates an empty root context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns an empty child scope linked to this context.
    pub fn derive(&self) -> Self {
        Self(Rc::new(ContextScope {
            parent: Some(self.clone()),
            values: HashMap::new(),
        }))
    }

    /// Returns a child context containing `value`.
    ///
    /// Providing the same type shadows its nearest ancestor for this child only.
    #[must_use]
    pub fn with<T: 'static>(&self, value: T) -> Self {
        let mut values = HashMap::new();
        values.insert(TypeId::of::<T>(), Box::new(value) as Box<dyn Any>);
        Self(Rc::new(ContextScope {
            parent: Some(self.clone()),
            values,
        }))
    }

    /// Builder-style synonym for [`Context::with`].
    #[must_use]
    pub fn provide<T: 'static>(&self, value: T) -> Self {
        self.with(value)
    }

    /// Clones the nearest value of type `T` from this scope or an ancestor.
    pub fn get<T: Clone + 'static>(&self) -> Option<T> {
        let mut scope = Some(self);
        while let Some(context) = scope {
            if let Some(value) = context.0.values.get(&TypeId::of::<T>()) {
                return value.downcast_ref::<T>().cloned();
            }
            scope = context.0.parent.as_ref();
        }
        None
    }

    /// Returns the nearest value of type `T`, panicking with its type name when
    /// the dependency was not explicitly provided.
    pub fn require<T: Clone + 'static>(&self) -> T {
        self.get::<T>()
            .unwrap_or_else(|| panic!("missing context value `{}`", type_name::<T>()))
    }

    /// Returns whether this scope or an ancestor provides `T`.
    pub fn contains<T: 'static>(&self) -> bool {
        let mut scope = Some(self);
        while let Some(context) = scope {
            if context.0.values.contains_key(&TypeId::of::<T>()) {
                return true;
            }
            scope = context.0.parent.as_ref();
        }
        false
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut depth = 0;
        let mut values = 0;
        let mut scope = Some(self);
        while let Some(context) = scope {
            depth += 1;
            values += context.0.values.len();
            scope = context.0.parent.as_ref();
        }
        formatter
            .debug_struct("Context")
            .field("depth", &depth)
            .field("values", &values)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::Context;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Pane(&'static str);

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Locale(&'static str);

    #[test]
    fn child_values_are_explicit_and_inherited() {
        let root = Context::new().provide(Locale("en"));
        let child = root.with(Pane("left"));

        assert_eq!(child.require::<Locale>(), Locale("en"));
        assert_eq!(child.require::<Pane>(), Pane("left"));
        assert!(!root.contains::<Pane>());
    }

    #[test]
    fn inline_context_shadows_without_mutating_parent_or_sibling() {
        let root = Context::new().with(Pane("root"));
        let left = root.with(Pane("left"));
        let right = root.with(Pane("right"));

        assert_eq!(root.require::<Pane>(), Pane("root"));
        assert_eq!(left.require::<Pane>(), Pane("left"));
        assert_eq!(right.require::<Pane>(), Pane("right"));
    }

    #[test]
    #[should_panic(expected = "missing context value")]
    fn require_names_a_missing_dependency() {
        Context::new().require::<Pane>();
    }
}
