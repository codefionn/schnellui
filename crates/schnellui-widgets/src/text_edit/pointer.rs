use super::*;

#[derive(Debug, Clone, Copy, PartialEq)]

pub enum EditKey<'a> {
    Insert(&'a str),
    Backspace,
    Delete,
    Left {
        select: bool,
        word: bool,
    },
    Right {
        select: bool,
        word: bool,
    },
    /// Line-scoped on a multi-line [`TextArea`](crate::TextArea); whole-value
    /// on a single-line input.
    Home {
        select: bool,
    },
    End {
        select: bool,
    },
    /// Caret up/down one visual line — meaningful only on a multi-line
    /// [`TextArea`](crate::TextArea); a no-op on a single-line input.
    Up {
        select: bool,
    },
    Down {
        select: bool,
    },
    /// Newline insertion — multi-line only; a single-line input ignores it.
    Enter,
    SelectAll,
}

/// The semantic phase of a pointer-selection gesture in an editable text
/// control. Window hosts should send one of the press variants followed by
/// [`TextPointerAction::Drag`] while the primary button remains down.
///
/// [`dispatch_text_pointer`] remains available as the compact caret/extend API;
/// this richer form adds native double-click word selection, triple-click line
/// selection, and unit-preserving drag extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextPointerAction {
    /// A normal press, or a Shift-press when `extend` is true.
    Place { extend: bool },
    /// Select the Unicode word-boundary segment under the pointer.
    SelectWord,
    /// Select the pointer's logical line; selects all in a single-line input.
    SelectLine,
    /// Add or remove a collapsed caret without disturbing existing selections.
    /// Native hosts map this to VS Code-style Alt-click.
    AddCaret,
    /// Extend the selection from the press that started the current gesture.
    Drag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerGranularity {
    Character,
    Word,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PointerOrigin {
    start: usize,
    end: usize,
    granularity: PointerGranularity,
}

// ---------------------------------------------------------------------------
// caret / boundary helpers (byte-index arithmetic, char-boundary safe)
// ---------------------------------------------------------------------------

pub(crate) fn prev_boundary(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j > 0 {
        j -= 1;
        if s.is_char_boundary(j) {
            return j;
        }
    }
    0
}

pub(crate) fn next_boundary(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j < s.len() {
        j += 1;
        if s.is_char_boundary(j) {
            return j;
        }
    }
    s.len()
}

/// Ctrl+Left target: skip whitespace, then the word, leftwards.
pub(crate) fn prev_word(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j > 0
        && s[..j]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_whitespace())
    {
        j = prev_boundary(s, j);
    }
    while j > 0
        && s[..j]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_whitespace())
    {
        j = prev_boundary(s, j);
    }
    j
}

/// Ctrl+Right target: skip the word, then whitespace, rightwards.
pub(crate) fn next_word(s: &str, i: usize) -> usize {
    let mut j = i.min(s.len());
    while j < s.len() && s[j..].chars().next().is_some_and(|c| !c.is_whitespace()) {
        j = next_boundary(s, j);
    }
    while j < s.len() && s[j..].chars().next().is_some_and(|c| c.is_whitespace()) {
        j = next_boundary(s, j);
    }
    j
}

/// The Unicode word-boundary segment containing `i`. Word-like segments,
/// punctuation runs, and whitespace runs remain distinct, matching native
/// editors more closely than whitespace-only splitting. A pointer beyond the
/// final glyph produces a collapsed range at the end.
pub(crate) fn word_range_at(s: &str, i: usize) -> (usize, usize) {
    let i = i.min(s.len());
    if i == s.len() {
        return (i, i);
    }
    s.split_word_bound_indices()
        .find_map(|(start, segment)| {
            let end = start + segment.len();
            (i >= start && i < end).then_some((start, end))
        })
        .unwrap_or((i, i))
}

/// The line containing `i`, including its trailing newline when one exists.
/// Including the newline makes a line-granularity drag compose cleanly with
/// adjacent selected lines and lets the existing TextArea paint mark it.
pub(crate) fn line_range_at(s: &str, i: usize) -> (usize, usize) {
    let i = i.min(s.len());
    let start = s[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let end = s[i..].find('\n').map(|p| i + p + 1).unwrap_or(s.len());
    (start, end)
}

/// Applies a pointer action without depending on a particular retained editor
/// state. Returns `(caret, anchor)` and updates the gesture origin used by
/// subsequent drags.
pub(crate) fn apply_pointer_selection(
    value: &str,
    anchor: usize,
    origin: &mut Option<PointerOrigin>,
    idx: usize,
    action: TextPointerAction,
    multiline: bool,
) -> (usize, usize) {
    let idx = idx.min(value.len());
    match action {
        TextPointerAction::Place { extend: false } => {
            *origin = Some(PointerOrigin {
                start: idx,
                end: idx,
                granularity: PointerGranularity::Character,
            });
            (idx, idx)
        }
        TextPointerAction::Place { extend: true } => {
            *origin = Some(PointerOrigin {
                start: anchor,
                end: anchor,
                granularity: PointerGranularity::Character,
            });
            (idx, anchor)
        }
        TextPointerAction::SelectWord => {
            let (start, end) = word_range_at(value, idx);
            *origin = Some(PointerOrigin {
                start,
                end,
                granularity: PointerGranularity::Word,
            });
            (end, start)
        }
        TextPointerAction::SelectLine => {
            let (start, end) = if multiline {
                line_range_at(value, idx)
            } else {
                (0, value.len())
            };
            *origin = Some(PointerOrigin {
                start,
                end,
                granularity: PointerGranularity::Line,
            });
            (end, start)
        }
        TextPointerAction::AddCaret => {
            unreachable!("AddCaret is handled by the retained editor state")
        }
        TextPointerAction::Drag => {
            let base = origin.unwrap_or(PointerOrigin {
                start: anchor,
                end: anchor,
                granularity: PointerGranularity::Character,
            });
            match base.granularity {
                PointerGranularity::Character => (idx, base.start),
                PointerGranularity::Word | PointerGranularity::Line => {
                    let (target_start, target_end) = match base.granularity {
                        PointerGranularity::Word => word_range_at(value, idx),
                        PointerGranularity::Line => line_range_at(value, idx),
                        PointerGranularity::Character => unreachable!(),
                    };
                    if target_end <= base.start {
                        (target_start, base.end)
                    } else if target_start >= base.end {
                        (target_end, base.start)
                    } else {
                        (base.end, base.start)
                    }
                }
            }
        }
    }
}

pub(crate) fn pointer_selection_index(
    action: TextPointerAction,
    origin: Option<PointerOrigin>,
    caret_idx: usize,
    hit_idx: usize,
) -> usize {
    match action {
        TextPointerAction::SelectWord | TextPointerAction::SelectLine => hit_idx,
        TextPointerAction::Drag
            if origin.is_some_and(|base| base.granularity != PointerGranularity::Character) =>
        {
            hit_idx
        }
        TextPointerAction::Place { .. } | TextPointerAction::AddCaret | TextPointerAction::Drag => {
            caret_idx
        }
    }
}

/// The physical pen advance in front of byte index `byte` (LTR: the sum of the
/// advances of every glyph whose cluster starts before it).
pub(crate) fn advance_before(shaped: &ShapedText, byte: usize) -> f32 {
    shaped
        .glyphs
        .iter()
        .filter(|g| (g.cluster as usize) < byte)
        .map(|g| g.x_advance)
        .sum()
}
