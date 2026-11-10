//! The retained-state probes the properties rely on, checked on hand-built
//! layouts where the right answer is known.

use schnellui::a11y::Role;
use schnellui::testing::fuzz::placement_faults;
use schnellui::widgets::{Column, Dialog, Pad, Scroll, Stack, Text};
use schnellui::App;

/// A dialog scoped to a panel inside scrolled content sits below the fold
/// of a short window. Scrolling reaches it, so it has not left the window.
#[test]
fn a_scoped_dialog_below_the_fold_of_a_scroll_view_is_placed() {
    let view = Scroll::new().fill().child(
        Column::new()
            .gap(12.0)
            .child(Pad::all(200.0).child(Text::new("Above the fold")))
            .child(
                Stack::new().size(260.0, 220.0).child(
                    Dialog::new("Scoped")
                        .modeless()
                        .non_fixed()
                        .size(240.0, 200.0)
                        .child(Text::new("Inside")),
                ),
            ),
    );
    let mut app = App::mount_with_size(view, 320, 300);
    app.frame();
    let panel = app.find_widget(Role::Dialog, Some("Scoped")).unwrap();
    assert!(
        app.scene().layout(panel).unwrap().rect.bottom() > 300.0,
        "the dialog starts below the fold"
    );
    assert_eq!(placement_faults(&app), None);
}
