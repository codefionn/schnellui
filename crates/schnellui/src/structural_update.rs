use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use schnellui_scene::ComponentRef;
use schnellui_widgets::View;

use crate::Remount;

/// A replacement view for one mounted component reference.
///
/// The referenced root is replaced in place while its parent, siblings, renderer,
/// window and all runtime state outside that branch remain resident.
pub struct SubtreeReplacement {
    pub(crate) target: ComponentRef,
    pub(crate) view: Box<dyn View>,
    pub(crate) reason: Cow<'static, str>,
    pub(crate) focus_after: Option<ComponentRef>,
}

impl SubtreeReplacement {
    pub fn new(
        target: ComponentRef,
        view: impl View,
        reason: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            target,
            view: Box::new(view),
            reason: reason.into(),
            focus_after: None,
        }
    }

    /// Restores focus to a retained sibling after this replacement removes a
    /// transient control (for example, an autocomplete choice).
    pub fn with_focus_after(mut self, target: ComponentRef) -> Self {
        self.focus_after = Some(target);
        self
    }
}

/// One structural update yielded by a native window host callback.
pub enum WindowUpdate {
    /// Replace the complete app while retaining the native window and GPU surface.
    Remount(Remount),
    /// Replace one or more independent retained branches in the current app.
    Subtrees(Vec<SubtreeReplacement>),
}

impl WindowUpdate {
    pub fn subtree(replacement: SubtreeReplacement) -> Self {
        Self::Subtrees(vec![replacement])
    }

    pub fn subtrees(replacements: Vec<SubtreeReplacement>) -> Self {
        Self::Subtrees(replacements)
    }
}

impl From<Remount> for WindowUpdate {
    fn from(value: Remount) -> Self {
        Self::Remount(value)
    }
}

impl From<SubtreeReplacement> for WindowUpdate {
    fn from(value: SubtreeReplacement) -> Self {
        Self::subtree(value)
    }
}

/// Failure to resolve a subtree target in the current mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissingSubtreeTarget {
    target: ComponentRef,
}

impl MissingSubtreeTarget {
    pub fn target(self) -> ComponentRef {
        self.target
    }
}

impl fmt::Display for MissingSubtreeTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "component ref {} is not mounted in this app",
            self.target.id()
        )
    }
}

impl Error for MissingSubtreeTarget {}

pub(crate) fn missing(target: ComponentRef) -> MissingSubtreeTarget {
    MissingSubtreeTarget { target }
}
