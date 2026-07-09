use schnellui::signal::create_signal;
use schnellui::widgets::{ReloadError, TemplateBlueprint};
use schnellui::App;
use std::io::Write;
use std::net::TcpStream;

#[test]
fn macro_template_reloads_through_app_socket_and_preserves_signal_node() {
    let count = create_signal(7u32);
    let view = schnellui::view! {
        column {
            text { "Heading" }
            text { (count.get()) }
        }
    };
    let mut next = TemplateBlueprint::from(view.template());
    let mut app = App::mount_with_size(view, 240, 120);
    app.frame();
    let root = app.scene().root().unwrap();
    let heading = app.scene().node(root).unwrap().children[0];
    let dynamic = app.scene().node(root).unwrap().children[1];
    let address = app.enable_hot_reload("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(address).unwrap();
    client.set_nodelay(true).unwrap();
    next.nodes[1].text = Some("Edited heading".into());
    let encoded = serde_json::to_vec(&next).unwrap();
    // Fragmented requests must stay buffered until their newline arrives.
    let middle = encoded.len() / 2;
    client.write_all(&encoded[..middle]).unwrap();
    app.frame();
    assert!(app.take_hot_reload_results().is_empty());
    client.write_all(&encoded[middle..]).unwrap();
    client.write_all(b"\n").unwrap();
    let mut results = Vec::new();
    for _ in 0..500 {
        app.frame();
        results.extend(app.take_hot_reload_results());
        if !results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok(), "{results:?}");
    assert_eq!(
        app.scene().a11y(heading).unwrap().name.as_deref(),
        Some("Edited heading")
    );
    assert_eq!(app.scene().node(root).unwrap().children[1], dynamic);
    count.set(23);
    app.frame();
    assert_eq!(
        app.scene().a11y(dynamic).unwrap().name.as_deref(),
        Some("23")
    );

    // A data-only child edit must also reconnect the resident layout graph.
    next.nodes[2].path = vec![0, 2];
    let mut inserted = next.nodes[1].clone();
    inserted.path = vec![0, 1];
    inserted.text = Some("New child".into());
    next.nodes.insert(2, inserted);
    serde_json::to_writer(&mut client, &next).unwrap();
    client.write_all(b"\n").unwrap();
    let mut results = Vec::new();
    for _ in 0..500 {
        app.frame();
        results.extend(app.take_hot_reload_results());
        if !results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(results[0].is_ok(), "{results:?}");
    let inserted = app.scene().node(root).unwrap().children[1];
    assert_eq!(app.scene().node(root).unwrap().children[2], dynamic);
    let inserted_rect = app.scene().layout(inserted).unwrap().rect;
    assert!(inserted_rect.width > 0.0 && inserted_rect.height > 0.0);
    assert!(app.scene().layout(dynamic).unwrap().rect.y >= inserted_rect.y + inserted_rect.height);

    next.nodes.remove(2);
    next.nodes[2].path = vec![0, 1];
    serde_json::to_writer(&mut client, &next).unwrap();
    client.write_all(b"\n").unwrap();
    let mut results = Vec::new();
    for _ in 0..500 {
        app.frame();
        results.extend(app.take_hot_reload_results());
        if !results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(results[0].is_ok(), "{results:?}");
    assert!(app.scene().node(inserted).is_none());
    assert_eq!(app.scene().node(root).unwrap().children[1], dynamic);

    next.nodes[1].text = Some("Must not land".into());
    next.nodes[2].dynamic = "different Rust code".into();
    serde_json::to_writer(&mut client, &next).unwrap();
    client.write_all(b"\n").unwrap();
    let mut results = Vec::new();
    for _ in 0..500 {
        app.frame();
        results.extend(app.take_hot_reload_results());
        if !results.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        matches!(&results[..], [Err(ReloadError::RebuildRequired(_))]),
        "{results:?}"
    );
    assert_eq!(
        app.scene().a11y(heading).unwrap().name.as_deref(),
        Some("Edited heading")
    );
    app.disable_hot_reload();
}
