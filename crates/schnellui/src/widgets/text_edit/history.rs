//! Undo/redo history of one editable text widget (SOUL §3.3: it lives in the
//! widget runtime beside the edit state, shared by `TextInput` and `TextArea`).
//!
//! A step is a **delta**, not a snapshot: the byte range an edit rewrote, the
//! text that range held before, and the selection on either side. Recording
//! and undoing therefore cost what the edit changed, never the document size,
//! and a run of typed characters extends one step in place without allocating.
//! The replaced text of every step lives in one grow-only arena per stack, so
//! a steady-state keystroke allocates nothing.
//!
//! Runs: typed inserts that continue at the caret, and Backspace or Delete
//! runs that continue at the caret, extend the open step. A whitespace
//! insert, a caret move or selection change, a different kind of edit, an
//! undo or a redo closes the run.
//!
//! The history trusts that every change of the value goes through
//! [`EditHistory::begin`]/[`EditHistory::finish`], [`EditHistory::undo`] or
//! [`EditHistory::redo`]. A whole-value rewrite calls [`EditHistory::clear`],
//! and the recorded text length is checked as a backstop: a value that is not
//! the length the history left behind drops the history instead of
//! restoring stale text.

use super::*;

/// How many undo steps a field keeps.
pub(crate) const MAX_UNDO_STEPS: usize = 100;

/// The sort of edit a step records. Only the first three extend a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditKind {
    Type,
    Backspace,
    Delete,
    /// Paste, cut, Enter, an edit over several carets: always a step of its own.
    Other,
}

/// A caret, anchor and any additional selections.
#[derive(Clone, Default)]
struct Selection {
    caret: usize,
    anchor: usize,
    secondary: SmallVec<[TextSelection; 1]>,
}

impl Selection {
    fn capture(caret: usize, anchor: usize, secondary: &[TextSelection]) -> Selection {
        Selection {
            caret,
            anchor,
            secondary: SmallVec::from_slice(secondary),
        }
    }

    fn is_caret(&self) -> bool {
        self.caret == self.anchor && self.secondary.is_empty()
    }
}

struct Step {
    kind: EditKind,
    /// Byte offset of the rewritten range.
    at: usize,
    /// Length of the range as it stands in the value now.
    len: usize,
    /// Where the text that range held when this step is applied lies in the
    /// stack's arena.
    text: std::ops::Range<usize>,
    before: Selection,
    after: Selection,
}

#[derive(Default)]
struct Stack {
    steps: Vec<Step>,
    arena: String,
}

impl Stack {
    fn clear(&mut self) {
        self.steps.clear();
        self.arena.clear();
    }

    /// Drops the oldest steps beyond the cap, and the arena bytes in front of
    /// the survivors once they outweigh the live text. That keeps the arena
    /// within twice the live text, so a field at its cap stops allocating.
    fn trim(&mut self) {
        if self.steps.len() > MAX_UNDO_STEPS {
            self.steps.remove(0);
        }
        let base = self
            .steps
            .first()
            .map_or(self.arena.len(), |s| s.text.start);
        if self.steps.is_empty() {
            self.arena.clear();
        } else if base * 2 > self.arena.len() {
            self.arena.drain(..base);
            for step in &mut self.steps {
                step.text = step.text.start - base..step.text.end - base;
            }
        }
    }
}

/// What [`EditHistory::begin`] decided, handed back to [`EditHistory::finish`].
pub(crate) struct Open {
    plan: Plan,
    kind: EditKind,
    /// The span of the value the edit may rewrite: bytes `lo..hi` of the old value.
    lo: usize,
    hi: usize,
    old_len: usize,
    before: Selection,
}

enum Plan {
    /// The edit changes nothing.
    Skip,
    /// A new step; `lo..hi` was copied to the end of the undo arena.
    New,
    /// The open run grows; the bytes of `lo..hi` were added to its text.
    Extend,
}

#[derive(Default)]
pub(crate) struct EditHistory {
    undo: Stack,
    redo: Stack,
    /// The value length the history left behind.
    len: usize,
    /// The newest undo step may still grow.
    run_open: bool,
}

impl EditHistory {
    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.run_open = false;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.undo.steps.is_empty() && self.redo.steps.is_empty()
    }

    /// Drops the history when the value is not what it left behind.
    fn check(&mut self, value: &str) -> bool {
        if !self.is_empty() && value.len() != self.len {
            self.clear();
        }
        !self.is_empty()
    }

    /// Prepares to record an edit of `value` that is about to run. `inserted`
    /// is the text the edit puts in place of each selection, `mode` how an
    /// empty selection widens.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn begin(
        &mut self,
        value: &str,
        caret: usize,
        anchor: usize,
        secondary: &[TextSelection],
        kind: EditKind,
        inserted: &str,
        mode: ReplaceMode,
    ) -> Open {
        self.check(value);
        let mut lo = usize::MAX;
        let mut hi = 0;
        for selection in selection_list(caret, anchor, secondary) {
            let (mut start, mut end) = selection.range();
            if start == end {
                match mode {
                    ReplaceMode::Backspace if start > 0 => start = prev_boundary(value, start),
                    ReplaceMode::Delete if end < value.len() => end = next_boundary(value, end),
                    _ => {}
                }
            }
            lo = lo.min(start);
            hi = hi.max(end);
        }
        let mut open = Open {
            plan: Plan::Skip,
            kind,
            lo,
            hi,
            old_len: value.len(),
            before: Selection::default(),
        };
        if lo == hi && inserted.is_empty() {
            return open;
        }
        let single = secondary.is_empty() && caret == anchor;
        if single && kind != EditKind::Other && self.run_open {
            if let Some(last) = self.undo.steps.last_mut() {
                let continues = last.kind == kind
                    && last.after.is_caret()
                    && last.after.caret == caret
                    && match kind {
                        EditKind::Type => {
                            lo == hi
                                && caret == last.at + last.len
                                && !inserted.chars().any(char::is_whitespace)
                        }
                        EditKind::Backspace => last.len == 0 && hi == last.at && lo < hi,
                        EditKind::Delete => last.len == 0 && lo == last.at && lo < hi,
                        EditKind::Other => false,
                    };
                if continues {
                    match kind {
                        EditKind::Backspace => {
                            self.undo.arena.insert_str(last.text.start, &value[lo..hi]);
                            last.text.end += hi - lo;
                        }
                        EditKind::Delete => {
                            self.undo.arena.push_str(&value[lo..hi]);
                            last.text.end += hi - lo;
                        }
                        _ => {}
                    }
                    open.plan = Plan::Extend;
                    return open;
                }
            }
        }
        self.undo.arena.push_str(&value[lo..hi]);
        open.before = Selection::capture(caret, anchor, secondary);
        open.plan = Plan::New;
        open
    }

    /// Completes what [`begin`](Self::begin) started, once the edit has run.
    pub(crate) fn finish(
        &mut self,
        open: Open,
        value: &str,
        caret: usize,
        anchor: usize,
        secondary: &[TextSelection],
        change: Change,
    ) {
        let Open {
            plan,
            kind,
            lo,
            hi,
            old_len,
            before,
        } = open;
        let applied = change == Change::Text;
        match plan {
            Plan::Skip => {}
            Plan::New if !applied => {
                let keep = self.undo.arena.len() - (hi - lo);
                self.undo.arena.truncate(keep);
            }
            Plan::New => {
                self.redo.clear();
                if self.undo.steps.capacity() == 0 {
                    // Room for a full history, so a field that keeps being
                    // edited at its cap never grows the stack again.
                    self.undo.steps.reserve_exact(MAX_UNDO_STEPS + 1);
                    self.undo.arena.reserve(256);
                }
                let end = self.undo.arena.len();
                self.undo.steps.push(Step {
                    kind,
                    at: lo,
                    len: (hi + value.len()).saturating_sub(old_len + lo),
                    text: end - (hi - lo)..end,
                    before,
                    after: Selection::capture(caret, anchor, secondary),
                });
                self.undo.trim();
                self.run_open = true;
                self.len = value.len();
            }
            Plan::Extend => {
                let Some(last) = self.undo.steps.last_mut() else {
                    return;
                };
                let grown = hi - lo;
                if !applied {
                    // Take back the bytes `begin` added to the run's text.
                    match kind {
                        EditKind::Backspace => {
                            self.undo
                                .arena
                                .replace_range(last.text.start..last.text.start + grown, "");
                            last.text.end -= grown;
                        }
                        EditKind::Delete => {
                            let keep = last.text.end - grown;
                            self.undo.arena.truncate(keep);
                            last.text.end = keep;
                        }
                        _ => {}
                    }
                    return;
                }
                self.redo.clear();
                match kind {
                    EditKind::Type => last.len = last.len + value.len() - old_len,
                    _ => last.at = lo,
                }
                last.after.caret = caret;
                last.after.anchor = anchor;
                last.after.secondary.clear();
                last.after.secondary.extend_from_slice(secondary);
                self.len = value.len();
            }
        }
    }

    /// Restores the text and selection before the newest edit. `None` when
    /// there is nothing to undo (or the history was stale).
    pub(crate) fn undo(
        &mut self,
        value: &mut String,
        caret: &mut usize,
        anchor: &mut usize,
        secondary: &mut SmallVec<[TextSelection; 4]>,
    ) -> Option<Rewrite> {
        self.step(true, value, caret, anchor, secondary)
    }

    /// Reapplies the edit the last undo took back.
    pub(crate) fn redo(
        &mut self,
        value: &mut String,
        caret: &mut usize,
        anchor: &mut usize,
        secondary: &mut SmallVec<[TextSelection; 4]>,
    ) -> Option<Rewrite> {
        self.step(false, value, caret, anchor, secondary)
    }

    fn step(
        &mut self,
        undoing: bool,
        value: &mut String,
        caret: &mut usize,
        anchor: &mut usize,
        secondary: &mut SmallVec<[TextSelection; 4]>,
    ) -> Option<Rewrite> {
        if !self.check(value) {
            return None;
        }
        self.run_open = false;
        let (from, to) = if undoing {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let step = from.steps.last()?;
        let end = step.at + step.len;
        if end > value.len() || !value.is_char_boundary(step.at) || !value.is_char_boundary(end) {
            self.clear();
            return None;
        }
        let step = from.steps.pop()?;
        let old_len = value.len();
        let swapped_start = to.arena.len();
        to.arena.push_str(&value[step.at..end]);
        let text = &from.arena[step.text.clone()];
        value.replace_range(step.at..end, text);
        let restored = text.len();
        from.arena.truncate(step.text.start);
        let selection = if undoing { &step.before } else { &step.after };
        *caret = selection.caret;
        *anchor = selection.anchor;
        secondary.clear();
        secondary.extend_from_slice(&selection.secondary);
        to.steps.push(Step {
            kind: step.kind,
            at: step.at,
            len: restored,
            text: swapped_start..swapped_start + (end - step.at),
            before: step.before,
            after: step.after,
        });
        if !undoing {
            to.trim();
        }
        self.len = value.len();
        Some(Rewrite {
            lo: step.at,
            hi: end,
            old_len,
        })
    }
}

/// The span an undo or redo rewrote, in the value as it was before.
pub(crate) struct Rewrite {
    pub(crate) lo: usize,
    pub(crate) hi: usize,
    pub(crate) old_len: usize,
}

/// Classifies a typed insert; an empty one is a cut.
pub(crate) fn insert_kind(text: &str) -> EditKind {
    if text.is_empty() {
        EditKind::Other
    } else {
        EditKind::Type
    }
}
