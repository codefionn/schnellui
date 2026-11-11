//! Editing one retained line must not allocate a copy of the whole document.
#![cfg(feature = "count-allocations")]

use schnellui::a11y::Role;
use schnellui::widgets::{Column, EditKey, TextArea};
use schnellui::{App, RawImeEvent};

fn measured_edit(lines: usize) -> [(u64, u64); 2] {
    let mut source = "=".repeat(160);
    for _ in 1..lines {
        source.push_str("\nA retained source line with ordinary text.");
    }
    let mut app = App::mount_with_size(Column::new().child(TextArea::new(source)), 800, 600);
    app.frame();
    let id = app.find_widget(Role::MultilineTextInput, None).unwrap();
    app.focus(Some(id));
    for _ in 0..lines / 2 {
        app.dispatch_edit_key(EditKey::Up { select: false });
    }
    app.dispatch_edit_key(EditKey::Home { select: false });
    app.dispatch_edit_key(EditKey::Right {
        select: false,
        word: false,
    });
    app.frame();
    for _ in 0..3 {
        app.dispatch_edit_key(EditKey::Insert("x"));
        app.frame();
        app.dispatch_edit_key(EditKey::Backspace);
        app.frame();
    }
    let original_len = app.scene().a11y(id).unwrap().value.as_ref().unwrap().len();
    let insert = allocation_counter::measure(|| {
        assert!(app.dispatch_edit_key(EditKey::Insert("x")));
        app.frame();
    });
    assert_eq!(
        app.scene().a11y(id).unwrap().value.as_ref().unwrap().len(),
        original_len + 1
    );
    let erase = allocation_counter::measure(|| {
        assert!(app.dispatch_edit_key(EditKey::Backspace));
        app.frame();
    });
    assert_eq!(
        app.scene().a11y(id).unwrap().value.as_ref().unwrap().len(),
        original_len
    );
    [
        (insert.count_total, insert.bytes_total),
        (erase.count_total, erase.bytes_total),
    ]
}

#[test]
fn text_area_edit_bytes_are_stable_across_document_sizes_and_edit_states() {
    let smallest = measured_edit(10);
    assert_eq!(
        smallest[0], smallest[1],
        "insertion and backspace must retain their buffers"
    );
    for lines in [100, 1000] {
        assert_eq!(
            measured_edit(lines),
            smallest,
            "allocation bytes grew with {lines} lines"
        );
    }
}

/// What a composition costs: the preedit update itself, and the frames around
/// it. A frame without a composition must cost what it did before one ran.
fn measured_composition(lines: usize) -> [(u64, u64); 3] {
    let mut source = "=".repeat(160);
    for _ in 1..lines {
        source.push_str("\nA retained source line with ordinary text.");
    }
    let mut app = App::mount_with_size(Column::new().child(TextArea::new(source)), 800, 600);
    app.frame();
    let id = app.find_widget(Role::MultilineTextInput, None).unwrap();
    app.focus(Some(id));
    for _ in 0..lines / 2 {
        app.dispatch_edit_key(EditKey::Up { select: false });
    }
    app.dispatch_edit_key(EditKey::Home { select: false });
    app.frame();
    let caret_step = |app: &mut App| {
        allocation_counter::measure(|| {
            app.dispatch_edit_key(EditKey::Right {
                select: false,
                word: false,
            });
            app.frame();
        })
    };
    let compose = |app: &mut App, text: &str| {
        allocation_counter::measure(|| {
            app.dispatch_ime(RawImeEvent::Preedit {
                text: text.into(),
                cursor: Some((text.len(), text.len())),
            });
            app.frame();
        })
    };
    for _ in 0..3 {
        caret_step(&mut app);
        compose(&mut app, "にほ");
        compose(&mut app, "");
    }
    let before = caret_step(&mut app);
    compose(&mut app, "に");
    let update = compose(&mut app, "にほ");
    compose(&mut app, "");
    let after = caret_step(&mut app);
    [
        (before.count_total, before.bytes_total),
        (update.count_total, update.bytes_total),
        (after.count_total, after.bytes_total),
    ]
}

#[test]
fn composing_costs_the_composed_line_and_leaves_plain_frames_alone() {
    let smallest = measured_composition(10);
    // A caret step after a composition allocates no more than one before it.
    assert!(
        smallest[2].1 <= smallest[0].1 && smallest[2].0 <= smallest[0].0,
        "plain frames cost more after a composition: {smallest:?}"
    );
    for lines in [100, 1000] {
        let sized = measured_composition(lines);
        assert_eq!(
            sized[1], smallest[1],
            "a preedit update grew with {lines} lines"
        );
        assert!(sized[2].1 <= sized[0].1, "{sized:?}");
    }
}
