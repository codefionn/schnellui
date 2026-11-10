//! Links in rich text, driven through the windowed host's own pointer, key and
//! AccessKit paths. A targeted span is a Link node: it must act like the
//! `Link` widget from every input, show the same feedback, and keep that
//! feedback across repaints and theme switches.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::windowed::headless::{HeadlessHost, HostKey, PointerButton};
use schnellui::a11y::{to_access_id, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::{Point, Primitive, Rect, WidgetId};
use schnellui::widgets::{CursorIcon, RichDoc, RichSpan, RichText, Theme, View, DEFAULT_THEME};

const TARGET: &str = "https://example.org/a";
type Log = Rc<RefCell<Vec<String>>>;

fn doc() -> RichDoc {
    RichDoc::new().heading(1, ["Links"]).paragraph([
        RichSpan::plain("Visit the "),
        RichSpan::link_to("hyperlink", TARGET),
        RichSpan::plain(" now, or read the "),
        RichSpan::link("styled only"),
        RichSpan::plain(" text that has no target."),
    ])
}

fn view(log: &Log, doc: RichDoc) -> RichText {
    let log = log.clone();
    RichText::new(doc).on_link(move |target| log.borrow_mut().push(target.to_string()))
}

fn themed_host(
    theme: Theme,
    width: u32,
    mut build: impl FnMut() -> RichText + 'static,
) -> HeadlessHost {
    struct Root(RichText);
    impl View for Root {
        fn build(
            self: Box<Self>,
            ctx: &mut schnellui::widgets::BuildCtx,
            parent: Option<WidgetId>,
        ) -> WidgetId {
            Box::new(self.0).build(ctx, parent)
        }
    }
    let mut app = App::mount_themed_with_size_scaled(
        ThemeMode::Fixed(theme),
        move || Root(build()),
        width,
        200,
        1.0,
    );
    app.apply_reduced_motion(true);
    app.frame();
    HeadlessHost::new(app)
}

fn host(width: u32) -> (HeadlessHost, Log) {
    let log = Log::default();
    let shared = log.clone();
    (
        themed_host(DEFAULT_THEME, width, move || view(&shared, doc())),
        log,
    )
}

fn link(host: &HeadlessHost) -> WidgetId {
    host.app()
        .find_widget(Role::Link, Some("hyperlink"))
        .expect("the targeted span is a link node")
}

fn rect(host: &HeadlessHost, id: WidgetId) -> Rect {
    host.app().scene().layout(id).expect("laid out").rect
}

fn centre(r: Rect) -> Point {
    Point {
        x: r.x + r.width / 2.0,
        y: r.y + r.height / 2.0,
    }
}

fn prims(host: &HeadlessHost, id: WidgetId) -> Vec<Primitive> {
    host.app().scene().paint(id).unwrap().primitives.clone()
}

/// Nothing a hover, press or focus left behind: every record still finds its
/// primitives, and re-laying each decoration changes nothing.
fn assert_decorations_sound(host: &mut HeadlessHost) {
    let widgets = host.app().widgets.clone();
    assert_eq!(
        schnellui::widgets::probes::decoration_faults(&widgets, host.app().scene()),
        None
    );
    assert_eq!(
        schnellui::widgets::probes::decoration_relay_diff(&widgets, host.app_mut().scene_mut()),
        None
    );
}

const AWAY: Point = Point { x: 390.0, y: 190.0 };

#[test]
fn hover_press_release_on_the_link_fires_once_with_its_target() {
    let (mut host, log) = host(400);
    let id = link(&host);
    let base = prims(&host, id);
    assert!(!base.is_empty(), "the link paints its own glyphs");
    let at = centre(rect(&host, id));

    host.pointer_move(at);
    assert_eq!(host.app().cursor_at(at), CursorIcon::Pointer);
    assert_ne!(prims(&host, id), base, "hover is shown on the link");
    assert_decorations_sound(&mut host);
    assert!(log.borrow().is_empty());

    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    assert_eq!(*log.borrow(), [TARGET]);
    assert_decorations_sound(&mut host);

    host.pointer_move(AWAY);
    assert_eq!(*log.borrow(), [TARGET], "leaving does not fire again");
}

#[test]
fn hover_then_leave_fires_nothing_and_takes_the_hover_off() {
    let (mut host, log) = host(400);
    let id = link(&host);
    let base = prims(&host, id);
    host.pointer_move(centre(rect(&host, id)));
    assert_ne!(prims(&host, id), base);
    host.pointer_move(AWAY);
    assert_eq!(prims(&host, id), base, "no stale hover decoration");
    host.pointer_leave();
    assert!(log.borrow().is_empty());
    assert_decorations_sound(&mut host);
}

#[test]
fn press_on_the_link_drag_off_release_does_not_fire() {
    let (mut host, log) = host(400);
    let id = link(&host);
    let base = prims(&host, id);
    host.pointer_move(centre(rect(&host, id)));
    host.press(PointerButton::Left);
    host.pointer_move(AWAY);
    host.release(PointerButton::Left);
    assert!(log.borrow().is_empty(), "released elsewhere is no click");
    assert_eq!(prims(&host, id), base, "press feedback is gone");
    assert_decorations_sound(&mut host);
}

#[test]
fn only_the_link_glyphs_are_hit_not_the_paragraph() {
    let (mut host, log) = host(400);
    let id = link(&host);
    let r = rect(&host, id);
    // plain text just left of the link, on the same line
    let plain = Point {
        x: r.x - 8.0,
        y: r.y + r.height / 2.0,
    };
    assert_ne!(host.app().hit_test(plain), Some(id));
    assert_eq!(host.app().cursor_at(plain), CursorIcon::Default);
    host.pointer_move(plain);
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    assert!(log.borrow().is_empty());
}

#[test]
fn tab_reaches_the_link_with_a_visible_ring_and_enter_fires_it() {
    let (mut host, log) = host(400);
    let id = link(&host);
    let base = prims(&host, id);
    host.key(&HostKey::Tab, false, false);
    assert_eq!(host.app().focused_widget(), Some(id));
    assert!(schnellui::widgets::focus_ring_visible(
        &host.app().widgets,
        id
    ));
    assert!(
        prims(&host, id)
            .iter()
            .any(|p| matches!(p, Primitive::StrokeRect { .. })),
        "the ring is painted around the link"
    );
    assert_decorations_sound(&mut host);
    host.key(&HostKey::Space, false, false);
    assert!(log.borrow().is_empty(), "a link takes Enter, not Space");
    host.key(&HostKey::Enter, false, false);
    assert_eq!(*log.borrow(), [TARGET]);
    assert_decorations_sound(&mut host);
    host.blur();
    assert_eq!(
        prims(&host, id),
        base,
        "the ring comes off when focus leaves"
    );
}

#[test]
fn an_accesskit_click_on_the_link_fires_it() {
    let (mut host, log) = host(400);
    let id = link(&host);
    let handled = host.app_mut().dispatch_action(&ActionRequest {
        action: Action::Click,
        target_tree: TreeId::ROOT,
        target_node: to_access_id(id),
        data: None,
    });
    assert!(handled);
    assert_eq!(*log.borrow(), [TARGET]);
}

#[test]
fn the_link_is_an_a11y_child_of_the_document_with_bounds_over_its_glyphs() {
    let (host, _log) = host(400);
    let id = link(&host);
    let app = host.app();
    let tree = schnellui::a11y::dump_tree(app.scene());
    let document = tree.root.expect("a root");
    assert_eq!(document.role, "document");
    let nodes: Vec<_> = document
        .children
        .iter()
        .filter(|c| c.role == "link")
        .collect();
    assert_eq!(
        nodes.len(),
        1,
        "one node per targeted span, none for the styled-only link"
    );
    assert_eq!(nodes[0].name.as_deref(), Some("hyperlink"));
    assert!(nodes[0].actions.iter().any(|a| a == "click"));
    assert!(nodes[0].actions.iter().any(|a| a == "focus"));

    let update = schnellui::a11y::build_full_tree_update(app.scene());
    let bounds_of = |id: WidgetId| {
        update
            .nodes
            .iter()
            .find(|(n, _)| *n == to_access_id(id))
            .and_then(|(_, node)| node.bounds())
            .expect("bounds published")
    };
    let link_bounds = bounds_of(id);
    let paragraph = bounds_of(app.scene().root().unwrap());
    assert!(link_bounds.x1 > link_bounds.x0 && link_bounds.y1 > link_bounds.y0);
    assert!(
        link_bounds.x0 >= paragraph.x0
            && link_bounds.x1 <= paragraph.x1
            && link_bounds.y0 >= paragraph.y0
            && link_bounds.y1 <= paragraph.y1,
        "{link_bounds:?} is not inside {paragraph:?}"
    );
}

#[test]
fn a_styled_link_without_a_target_stays_plain_text() {
    let (mut host, log) = host(400);
    assert!(host
        .app()
        .find_widget(Role::Link, Some("styled only"))
        .is_none());
    let document = host.app().scene().root().unwrap();
    let before = prims(&host, document);
    // somewhere over the untargeted link's glyphs
    let words = prims(&host, document)
        .iter()
        .filter_map(|p| match p {
            Primitive::GlyphQuad { rect, .. } => Some(*rect),
            _ => None,
        })
        .count();
    assert!(words > 0);
    for dx in (0..380).step_by(10) {
        let p = Point {
            x: 10.0 + dx as f32,
            y: 50.0,
        };
        host.pointer_move(p);
        host.press(PointerButton::Left);
        host.release(PointerButton::Left);
    }
    // nothing but the one targeted link ever fires
    assert!(log.borrow().iter().all(|t| t == TARGET));
    assert_eq!(
        prims(&host, document),
        before,
        "the document paint never changes"
    );
}

#[test]
fn links_without_a_handler_are_exposed_but_inert() {
    let mut app = App::mount_with_size(RichText::new(doc()), 400, 200);
    app.frame();
    let id = app.find_widget(Role::Link, Some("hyperlink")).unwrap();
    let node = app.scene().a11y(id).unwrap();
    assert_eq!(node.actions, 0, "no click or focus without an on_link");
    assert!(!app.dispatch_click(id));
}

#[test]
fn a_wrapped_link_is_hit_on_its_lines_only() {
    let log = Log::default();
    let shared = log.clone();
    let long = RichDoc::new().paragraph([
        RichSpan::plain("see "),
        RichSpan::link_to(
            "a very long hyperlink that wraps over several lines",
            TARGET,
        ),
        RichSpan::plain(" end"),
    ]);
    let mut host = themed_host(DEFAULT_THEME, 160, move || view(&shared, long.clone()));
    let id = host
        .app()
        .find_widget(
            Role::Link,
            Some("a very long hyperlink that wraps over several lines"),
        )
        .unwrap();
    let union = rect(&host, id);
    let lines = schnellui::widgets::link_fragments(&host.app().widgets, id);
    assert!(lines.len() >= 2, "wrapped onto several lines: {lines:?}");
    assert_eq!(lines.iter().fold(Rect::default(), |a, r| a.union(r)), union);
    // the union's bottom-right corner is past the end of the last line
    let corner = Point {
        x: union.right() - 1.0,
        y: union.bottom() - 2.0,
    };
    assert_ne!(
        host.app().hit_test(corner),
        Some(id),
        "the gap in the union box is not the link"
    );
    // the first and the last line both are
    for line in [lines[0], lines[lines.len() - 1]] {
        let p = centre(line);
        assert_eq!(host.app().hit_test(p), Some(id), "{p:?}");
    }
    host.pointer_move(centre(lines[lines.len() - 1]));
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    assert_eq!(*log.borrow(), [TARGET]);
}

#[test]
fn a_theme_switch_under_a_hovered_link_leaves_no_stale_decoration() {
    let log = Log::default();
    let shared = log.clone();
    let mut host = themed_host(DEFAULT_THEME, 400, move || view(&shared, doc()));
    let at = centre(rect(&host, link(&host)));
    host.pointer_move(at);
    host.set_theme(schnellui::theme::DARK);
    assert_decorations_sound(&mut host);
    let id = link(&host);
    host.pointer_move(AWAY);
    assert_decorations_sound(&mut host);

    // the rebuilt link paints what a fresh mount of that theme paints
    let fresh = {
        let other = Log::default();
        themed_host(schnellui::theme::DARK, 400, move || view(&other, doc()))
    };
    assert_eq!(prims(&host, id), prims(&fresh, link(&fresh)));

    host.pointer_move(centre(rect(&host, id)));
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    assert_eq!(
        *log.borrow(),
        [TARGET],
        "the link still fires after the switch"
    );
}

#[test]
fn a_theme_switch_under_a_focused_link_keeps_focus_and_the_ring() {
    let log = Log::default();
    let shared = log.clone();
    let mut host = themed_host(DEFAULT_THEME, 400, move || view(&shared, doc()));
    host.key(&HostKey::Tab, false, false);
    assert!(schnellui::widgets::focus_ring_visible(
        &host.app().widgets,
        link(&host)
    ));
    host.set_theme(schnellui::theme::DARK);
    let id = link(&host);
    assert_eq!(host.app().focused_widget(), Some(id));
    assert!(schnellui::widgets::focus_ring_visible(
        &host.app().widgets,
        id
    ));
    assert!(prims(&host, id)
        .iter()
        .any(|p| matches!(p, Primitive::StrokeRect { .. })));
    assert_decorations_sound(&mut host);
    host.key(&HostKey::Enter, false, false);
    assert_eq!(*log.borrow(), [TARGET]);
}

#[test]
fn a_dynamic_document_keeps_the_link_nodes_in_step_with_the_text() {
    let log = Log::default();
    let shared = log.clone();
    let version = schnellui::signal::create_signal(0_u8);
    let mut host = themed_host(DEFAULT_THEME, 400, move || {
        let shared = shared.clone();
        RichText::dynamic(move || match version.get() {
            0 => RichDoc::new().paragraph([
                RichSpan::plain("one "),
                RichSpan::link_to("alpha", "x://alpha"),
            ]),
            1 => RichDoc::new().paragraph([
                RichSpan::link_to("beta", "x://beta"),
                RichSpan::plain(" and "),
                RichSpan::link_to("gamma", "x://gamma"),
            ]),
            _ => RichDoc::new().paragraph(["no links any more"]),
        })
        .on_link(move |t| shared.borrow_mut().push(t.to_string()))
    });
    let names = |host: &HeadlessHost| -> Vec<String> {
        let tree = schnellui::a11y::dump_tree(host.app().scene());
        tree.root
            .unwrap()
            .children
            .iter()
            .filter(|c| c.role == "link")
            .filter_map(|c| c.name.clone())
            .collect()
    };
    assert_eq!(names(&host), ["alpha"]);
    let first = host.app().find_widget(Role::Link, Some("alpha")).unwrap();

    version.set(1);
    host.frame();
    assert_eq!(names(&host), ["beta", "gamma"]);
    let beta = host.app().find_widget(Role::Link, Some("beta")).unwrap();
    assert_eq!(beta, first, "a link keeps its node across a retarget");
    assert!(
        !rect(&host, beta).is_empty(),
        "and it is re-boxed over its new text"
    );
    host.pointer_move(centre(rect(&host, beta)));
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    let gamma = host.app().find_widget(Role::Link, Some("gamma")).unwrap();
    host.pointer_move(centre(rect(&host, gamma)));
    host.press(PointerButton::Left);
    host.release(PointerButton::Left);
    assert_eq!(*log.borrow(), ["x://beta", "x://gamma"]);

    version.set(2);
    host.frame();
    assert!(names(&host).is_empty(), "no link text, no link nodes");
    assert!(host.app().find_widget(Role::Link, None).is_none());
    assert_decorations_sound(&mut host);
    host.key(&HostKey::Tab, false, false);
    assert_eq!(host.app().focused_widget(), None, "nothing left to tab to");
}

#[test]
fn adjacent_links_with_the_same_look_keep_their_own_targets() {
    let log = Log::default();
    let shared = log.clone();
    let doc = RichDoc::new()
        .heading(2, [RichSpan::link_to("head", "x://head")])
        .bullet([
            RichSpan::plain("item "),
            RichSpan::link_to("one", "x://one"),
            RichSpan::link_to("two", "x://two"),
        ])
        .quote([RichSpan::link_to("quoted", "x://quoted")]);
    let mut host = themed_host(DEFAULT_THEME, 400, move || view(&shared, doc.clone()));
    for name in ["head", "one", "two", "quoted"] {
        let id = host.app().find_widget(Role::Link, Some(name)).unwrap();
        host.pointer_move(centre(rect(&host, id)));
        host.press(PointerButton::Left);
        host.release(PointerButton::Left);
    }
    assert_eq!(
        *log.borrow(),
        ["x://head", "x://one", "x://two", "x://quoted"],
        "every link, in every block kind, fires its own target"
    );
    // Tab walks them in reading order
    host.blur();
    let mut order = Vec::new();
    for _ in 0..4 {
        host.key(&HostKey::Tab, false, false);
        let id = host.app().focused_widget().unwrap();
        order.push(host.app().scene().a11y(id).unwrap().name.clone().unwrap());
    }
    assert_eq!(order, ["head", "one", "two", "quoted"]);
}

fn every_theme() -> Theme {
    use schnellui::scene::Color;
    use schnellui::widgets::{ComponentInteractions, InteractionStates, InteractionStyle};
    let style = |r: u8| InteractionStyle {
        background: Some(Color::rgba(r, 40, 90, 0x50)),
        foreground: Some(Color::rgb(r, 10, 200)),
        border: Some(Color::rgb(r, 120, 30)),
    };
    let states = |r: u8| InteractionStates {
        hover: style(r),
        focus: style(r + 20),
        active: style(r + 40),
    };
    Theme {
        interactions: states(10),
        component_interactions: ComponentInteractions {
            button: Some(states(70)),
            navigation: Some(states(130)),
            toggle: None,
            editable: Some(states(190)),
            raw_surface: Some(states(200)),
        },
        ..DEFAULT_THEME
    }
}

fn scroll_doc_host(theme: Theme) -> HeadlessHost {
    use schnellui::widgets::{Button, Column, Pad, Scroll};
    let doc = RichDoc::new()
        .heading(2, ["Links"])
        .paragraph([
            RichSpan::plain("Read the "),
            RichSpan::link_to("manual", "x://manual"),
            RichSpan::plain(", the "),
            RichSpan::link_to("changelog and the long migration guide", "x://changes"),
            RichSpan::plain(" or a "),
            RichSpan::link("styled-only link"),
            RichSpan::plain(" in a paragraph that wraps in a narrow window."),
        ])
        .bullet([
            RichSpan::plain("item "),
            RichSpan::link_to("one", "x://one"),
            RichSpan::link_to("two", "x://two"),
        ])
        .quote([RichSpan::link_to("quoted", "x://quoted")]);
    let mut app = App::mount_themed_with_size_scaled(
        ThemeMode::Fixed(theme),
        move || {
            let doc = doc.clone();
            // the button leaves the document taller than its viewport, so the
            // scroll view is a focusable, hoverable surface of its own
            Column::new()
                .fill()
                .child(
                    Scroll::new()
                        .fill()
                        .child(Pad::all(12.0).child(RichText::new(doc).on_link(|_| {}))),
                )
                .child(Pad::all(8.0).child(Button::new("Done")))
        },
        420,
        300,
        1.0,
    );
    app.apply_reduced_motion(true);
    app.frame();
    HeadlessHost::new(app)
}

fn all_link_paints(host: &HeadlessHost) -> Vec<(String, Vec<Primitive>)> {
    let mut out = Vec::new();
    let mut stack: Vec<WidgetId> = host.app().scene().root().into_iter().collect();
    while let Some(id) = stack.pop() {
        let scene = host.app().scene();
        if scene
            .a11y(id)
            .is_some_and(|a| a.role == Role::Link.as_u16())
        {
            out.push((
                scene.a11y(id).unwrap().name.clone().unwrap_or_default(),
                scene.paint(id).unwrap().primitives.clone(),
            ));
        }
        stack.extend(scene.node(id).unwrap().children.iter().copied());
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A press on the scroll view that holds a document ends with the pointer gone
/// and the button up: every link must paint what a fresh mount paints. The
/// scroll view's hover and press washes land on the last painted descendant,
/// which is a link, and a link's rect used to be taken for its underline when
/// the next state recoloured it.
#[test]
fn washes_of_an_enclosing_scroll_view_never_stick_to_a_link() {
    let mut host = scroll_doc_host(every_theme());
    let fresh = scroll_doc_host(every_theme());
    host.pointer_move(Point {
        x: 420.0 * 0.514,
        y: 300.0 * 0.355,
    });
    host.press(PointerButton::Left);
    host.key(&HostKey::Tab, false, false);
    host.release(PointerButton::Left);
    host.pointer_leave();
    host.key(&HostKey::Escape, false, false);
    host.blur();
    assert_decorations_sound(&mut host);
    let (now, base) = (all_link_paints(&host), all_link_paints(&fresh));
    assert_eq!(now.len(), 5, "every targeted span is a link");
    for ((name, shown), (_, expected)) in now.iter().zip(&base) {
        assert_eq!(shown, expected, "link {name:?} kept a decoration");
    }
}

/// Press on the scroll view that holds a document, focus its last link (the node
/// the press stroke sits on), then move the pointer onto another link. The press
/// comes off and goes back on for the new hover, and it must land below the
/// focus ring, as it did before.
#[test]
fn a_press_on_the_scroll_view_stays_below_the_ring_of_a_link_inside() {
    let mut host = scroll_doc_host(every_theme());
    // over the heading: no link, so the press lands on the enclosing scroll view
    host.pointer_move(Point { x: 200.0, y: 30.0 });
    host.press(PointerButton::Left);
    let quoted = host.app().find_widget(Role::Link, Some("quoted")).unwrap();
    host.app_mut().focus(Some(quoted));
    host.key(&HostKey::Tab, true, false);
    host.key(&HostKey::Tab, false, false);
    assert_eq!(host.app().focused_widget(), Some(quoted));
    assert!(schnellui::widgets::focus_ring_visible(
        &host.app().widgets,
        quoted
    ));
    assert_decorations_sound(&mut host);
    let manual = host.app().find_widget(Role::Link, Some("manual")).unwrap();
    host.pointer_move(centre(rect(&host, manual)));
    assert_decorations_sound(&mut host);
    host.release(PointerButton::Left);
    host.pointer_leave();
    assert_decorations_sound(&mut host);
}
