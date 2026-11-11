use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use schnellui::scene::ComponentRef;
use schnellui::widgets::View;

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
    pub(crate) escape_return: bool,
    pub(crate) escape_fallback: Option<ComponentRef>,
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
            escape_return: false,
            escape_fallback: None,
        }
    }

    /// Restores focus to a retained sibling after this replacement removes a
    /// transient control (for example, an autocomplete choice).
    pub fn with_focus_after(mut self, target: ComponentRef) -> Self {
        self.focus_after = Some(target);
        self
    }

    /// Marks the replaced branch as a docked region: a panel beside the content that is no
    /// dialog and takes no focus trap. The control that had focus when the region appeared is
    /// remembered, and Escape with focus inside the region moves focus back to it. A later
    /// replacement of the same target without this mark forgets the control, as when the region
    /// closes.
    pub fn with_escape_return(mut self) -> Self {
        self.escape_return = true;
        self
    }

    /// Where Escape goes when the remembered opener is gone, or when none was remembered (the
    /// control that opened the region vanished with it, as a card's button does). `target` is
    /// resolved when Escape is pressed: focus goes to it or to its first focusable descendant.
    /// Only meaningful together with [`Self::with_escape_return`].
    pub fn with_escape_fallback(mut self, target: ComponentRef) -> Self {
        self.escape_fallback = Some(target);
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
