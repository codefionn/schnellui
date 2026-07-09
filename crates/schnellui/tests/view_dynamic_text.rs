//! `view!` dynamic text writes into the slot's reused buffer and still updates.

use schnellui::signal::create_signal;
use schnellui::App;

#[test]
fn view_dynamic_text_updates_through_the_buffer_codegen() {
    let n = create_signal(1i32);
    let mut app = App::mount_with_size(schnellui::view! { text { (n.get()) } }, 100, 50);
    app.frame();
    let root = app.scene().root().unwrap();
    assert_eq!(app.scene().a11y(root).unwrap().name.as_deref(), Some("1"));
    n.set(23);
    app.frame();
    assert_eq!(app.scene().a11y(root).unwrap().name.as_deref(), Some("23"));
}
