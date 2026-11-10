//! # rich_text — a one-shot screenshotter example (SOUL §7.1)
//!
//! Exercises the rich text surface: the [`RichText`] viewer rendering a
//! hand-built [`RichDoc`] (headings, emphasis, lists, quotes, code, rules),
//! and the [`TextArea`] multi-line editor with an **example-side** syntax
//! highlighter — format parsing and token coloring are the implementor's
//! responsibility by design, so this example *is* the implementor: it builds
//! its documents in code and plugs a tiny Rust-ish keyword highlighter into
//! the editor. The `preview` scenario wires editor → signal → dynamic viewer
//! (edit-and-preview, SOUL §3.3), driven headlessly through the inbound
//! AccessKit `SetValue` path (SOUL §6.3, §7.5). The showcase's hyperlink
//! carries a target; `RichText::on_link` reports it in a status line, and the
//! `link` scenario clicks it through the AccessKit `Click` path.

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, Role};
use schnellui::accesskit_action::{Action, ActionData, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::Color;
use schnellui::signal::create_signal;
use schnellui::testing::{assert_value_contains, find_by_role_name, SnapshotConfig};
use schnellui::widgets::{
    Align, Column, EditKey, Flex, Pad, RichDoc, RichSpan, RichText, Row, Scroll, SpanStyle, Stack,
    Text, TextArea, Theme, ThemeProvider, View, WrapMode, DEFAULT_THEME,
};
use schnellui::App;
use strum::IntoEnumIterator;

// ---------------------------------------------------------------------------
// the implementor-side styling (SOUL: parsing is application code)
// ---------------------------------------------------------------------------

/// The example's syntax palette (any colors work; the framework just draws).
const KEYWORD: Color = Color::rgb(0x88, 0x33, 0xaa);
const STRING: Color = Color::rgb(0x22, 0x77, 0x33);
const NUMBER: Color = Color::rgb(0x0a, 0x66, 0xaa);
/// 5.6:1 on the code surface, so comments stay readable under a selection.
const COMMENT: Color = Color::rgb(0x5a, 0x61, 0x6c);
/// Markup markers (`#`, `-`, `>`) in the source editor: quieter than the text
/// they introduce but still 5.6:1 on white.
const MARKER: Color = Color::rgb(0x5a, 0x61, 0x6c);
/// Pane captions above the editor and the preview.
const CAPTION: Color = Color::rgb(0x55, 0x5b, 0x66);

const RUST_KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "pub", "if", "else", "match", "return", "struct", "impl", "use", "for",
    "in", "while", "loop", "true", "false",
];

/// A deliberately tiny Rust-ish highlighter: keywords (bold), `//` comments,
/// `"…"` strings, numbers. One span list per source line — the shape
/// [`TextArea::highlight`] and [`RichDoc::code_block`] both consume.
fn highlight_rust(src: &str) -> Vec<Vec<RichSpan>> {
    src.lines().map(highlight_line).collect()
}

fn highlight_line(line: &str) -> Vec<RichSpan> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let flush = |plain: &mut String, spans: &mut Vec<RichSpan>| {
        if !plain.is_empty() {
            spans.push(RichSpan::code(std::mem::take(plain)));
        }
    };
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &line[i..];
        if rest.starts_with("//") {
            flush(&mut plain, &mut spans);
            spans.push(RichSpan::token(rest, COMMENT));
            return spans;
        }
        if let Some(stripped) = rest.strip_prefix('"') {
            let end = stripped.find('"').map(|e| e + 2).unwrap_or(rest.len());
            flush(&mut plain, &mut spans);
            spans.push(RichSpan::token(&rest[..end], STRING));
            i += end;
            continue;
        }
        let c = rest.chars().next().unwrap();
        if c.is_ascii_digit() && !plain.chars().next_back().is_some_and(char::is_alphanumeric) {
            let end = rest
                .find(|ch: char| !ch.is_ascii_alphanumeric() && ch != '.' && ch != '_')
                .unwrap_or(rest.len());
            flush(&mut plain, &mut spans);
            spans.push(RichSpan::token(&rest[..end], NUMBER));
            i += end;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let end = rest
                .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
                .unwrap_or(rest.len());
            let word = &rest[..end];
            if RUST_KEYWORDS.contains(&word) {
                flush(&mut plain, &mut spans);
                spans.push(RichSpan::new(
                    word,
                    SpanStyle {
                        bold: true,
                        ..SpanStyle::token(KEYWORD)
                    },
                ));
            } else {
                plain.push_str(word);
            }
            i += end;
            continue;
        }
        plain.push(c);
        i += c.len_utf8();
    }
    flush(&mut plain, &mut spans);
    spans
}

/// Colors the miniature markup in the source editor: block markers muted,
/// heading text bold, everything else plain.
fn highlight_markup(src: &str) -> Vec<Vec<RichSpan>> {
    src.lines()
        .map(|line| {
            for marker in ["## ", "# ", "- ", "> "] {
                if let Some(rest) = line.strip_prefix(marker) {
                    let rest_style = if marker.starts_with('#') {
                        SpanStyle {
                            bold: true,
                            ..SpanStyle::PLAIN
                        }
                    } else {
                        SpanStyle::PLAIN
                    };
                    return vec![
                        RichSpan::token(marker, MARKER),
                        RichSpan::new(rest, rest_style),
                    ];
                }
            }
            vec![RichSpan::plain(line)]
        })
        .collect()
}

/// The example's miniature markup (the implementor-side "format"): `# ` / `## `
/// headings, `- ` bullets, `> ` quotes, blank-line paragraphs. Used by the
/// `preview` scenario to turn the edited source into a document.
fn parse_mini_markup(src: &str) -> RichDoc {
    let mut doc = RichDoc::new();
    let mut para = String::new();
    for line in src.lines().map(str::trim_end) {
        if let Some(t) = line.strip_prefix("## ") {
            doc = flush_para(doc, &mut para).heading(2, [t]);
        } else if let Some(t) = line.strip_prefix("# ") {
            doc = flush_para(doc, &mut para).heading(1, [t]);
        } else if let Some(t) = line.strip_prefix("- ") {
            doc = flush_para(doc, &mut para).bullet([t]);
        } else if let Some(t) = line.strip_prefix("> ") {
            doc = flush_para(doc, &mut para).quote([t]);
        } else if line.is_empty() {
            doc = flush_para(doc, &mut para);
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(line);
        }
    }
    flush_para(doc, &mut para)
}

fn flush_para(doc: RichDoc, para: &mut String) -> RichDoc {
    if para.is_empty() {
        doc
    } else {
        doc.paragraph([std::mem::take(para)])
    }
}

// ---------------------------------------------------------------------------
// the scenario documents
// ---------------------------------------------------------------------------

const CODE_SAMPLE: &str = r#"// the soul made executable (SOUL §1)
fn rerender(count: u32) -> Frame {
    let budget = 0; // allocs + reallocs + frees
    if count > 9000 {
        return Frame::skip("over budget");
    }
    Frame::draw(count)
}"#;

/// The showcase document: every block kind + every span axis, built by hand
/// through the chainable model builders.
fn showcase_doc() -> RichDoc {
    RichDoc::new()
        .heading(1, ["schnellui rich text"])
        .paragraph([
            RichSpan::plain("A retained document with "),
            RichSpan::bold("bold"),
            RichSpan::plain(", "),
            RichSpan::italic("italic"),
            RichSpan::plain(", "),
            RichSpan::new(
                "bold italic",
                SpanStyle {
                    bold: true,
                    italic: true,
                    ..SpanStyle::PLAIN
                },
            ),
            RichSpan::plain(", inline "),
            RichSpan::code("code"),
            RichSpan::plain(", a "),
            RichSpan::link_to("hyperlink", LINK_TARGET),
            RichSpan::plain(", and "),
            RichSpan::new(
                "strikethrough",
                SpanStyle {
                    strike: true,
                    ..SpanStyle::PLAIN
                },
            ),
            RichSpan::plain(" — with zero allocations on a clean re-render."),
        ])
        .heading(2, ["Blocks"])
        .bullet(["bullet lists"])
        .bullet(["nested and ordered items"])
        .list_item(
            1,
            schnellui::widgets::ListMarker::Bullet,
            ["one level deeper"],
        )
        .list_item(
            0,
            schnellui::widgets::ListMarker::Number(1),
            ["first ordered entry"],
        )
        .list_item(
            0,
            schnellui::widgets::ListMarker::Number(2),
            ["second ordered entry"],
        )
        .quote(["Accessibility is a first-class output, not an overlay."])
        .code_block("rust", highlight_rust(CODE_SAMPLE))
        .rule()
        .paragraph(["Importers are application code: this document was built by hand."])
}

/// Where the showcase's hyperlink points.
const LINK_TARGET: &str = "https://schnellui.dev/docs/rich-text";
/// What the status line shows until a link is activated.
const NO_LINK: &str = "No link activated yet. Click or Tab to the hyperlink.";

const EDITOR_SOURCE: &str = r#"fn main() {
    let greeting = "hello, schnellui";
    println!("{greeting}: {}", 42);
}"#;

const PREVIEW_SOURCE: &str = "# Trip notes\n\nType on the left; the page on the right follows as you write.\n\n## Packing\n\n- tide tables\n- spare batteries\n- a second map\n\n> Leave every site as you found it.";

/// The source the `preview` scenario drives in through AccessKit `SetValue`.
const DRIVEN_SOURCE: &str = "# Driven\n\nThis body arrived via SetValue and the preview re-flowed.\n\n## Checklist\n\n- headings\n- bullet lists\n\n> Quotes render as quotes.";

// ---------------------------------------------------------------------------
// scenarios (SOUL §7.1 enumerable table)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// the viewer over the hand-built showcase document — constructed.
    Document,
    /// the showcase with its hyperlink *driven*: an AccessKit `Click` on the
    /// link node reaches `RichText::on_link`, and the status line names the
    /// target (the same path a pointer click or Enter takes, SOUL §6.3).
    Link,
    /// the multi-line editor with the example highlighter, *driven* to a
    /// mid-document caret + an edit through the real key path (SOUL §7.5).
    Editor,
    /// editor + dynamic viewer side by side; the source is *driven* through an
    /// inbound AccessKit `SetValue`, and the preview re-flows (SOUL §6.3).
    Preview,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::Document => "document",
            Scenario::Link => "link",
            Scenario::Editor => "editor",
            Scenario::Preview => "preview",
        }
    }
}

/// The screenshotter CLI contract (SOUL §7.1).
#[derive(Parser, Debug)]
#[command(
    name = "rich_text",
    about = "schnellui one-shot rich text viewer/editor screenshotter (SOUL §7.1)"
)]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 880)]
    width: u32,
    #[arg(long, default_value_t = 640)]
    height: u32,
    /// logical→physical scale (SOUL §7.1).
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long)]
    theme: Option<String>,
    /// print scenario names one per line and exit (SOUL §7.1).
    #[arg(long)]
    list: bool,
    /// render every scenario into `--out-dir` (SOUL §7.1).
    #[arg(long)]
    all: bool,
    #[arg(long)]
    out_dir: Option<String>,
    /// emit `[{scenario, path, width, height}]` mapping (SOUL §7.1).
    #[arg(long)]
    manifest: Option<String>,
    /// write the AccessKit tree JSON alongside the PNG (SOUL §7.1).
    #[arg(long)]
    dump_a11y: Option<String>,
    /// run the scenario's embedded a11y assertions, nonzero on failure (SOUL §7.1).
    #[arg(long)]
    assert: bool,
    /// opt-in windowed mode (SOUL §8): live editing with real keyboard input.
    #[arg(long)]
    windowed: bool,
    /// Run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
}

/// Captions in a muted ink (6.6:1 on white).
const CAPTION_THEME: Theme = Theme {
    text: CAPTION,
    ..DEFAULT_THEME
};

/// A small, quiet label above a pane.
fn pane_caption(text: &'static str) -> impl View {
    ThemeProvider::new(
        CAPTION_THEME,
        Text::new(text).size(12.0).bold().wrap(WrapMode::Word),
    )
}

/// The padded work area under a viewport-filling stack: a definite box, so a
/// pane can take the height its captions leave and scroll its content inside.
fn stage(content: impl View) -> impl View {
    Stack::new().fill().child(Pad::all(24.0).child(content))
}

fn document_view(width: f32) -> impl View {
    let status = create_signal(NO_LINK.to_string());
    stage(
        Scroll::new().fill().scrollbar(true).child(
            Column::new()
                .width(width - 48.0)
                .gap(16.0)
                .child(
                    RichText::new(showcase_doc())
                        .on_link(move |target| status.set(format!("Opened link: {target}"))),
                )
                .child(Text::dynamic(move || status.get()).size(13.0)),
        ),
    )
}

fn editor_view(width: f32) -> impl View {
    stage(
        Column::new()
            .fill()
            .width(width - 48.0)
            .gap(8.0)
            .child(RichText::new(RichDoc::new().heading(2, ["Source editor"])))
            .child(pane_caption(
                "main.rs · keywords, strings, numbers and comments are colored by the example",
            ))
            // The area grows with its text, so it scrolls inside the room the
            // captions leave; the caret reveal then keeps typing in view.
            .child(
                Flex::new().grow(1.0).shrink(1.0).child(
                    Scroll::new().scrollbar(true).child(
                        TextArea::new(EDITOR_SOURCE)
                            .placeholder("type some rust")
                            .size(15.0)
                            .rows(23)
                            .wrap(WrapMode::Word)
                            .line_numbers(1, EDITOR_SOURCE.lines().count(), CAPTION)
                            .highlight(highlight_rust),
                    ),
                ),
            ),
    )
}

fn preview_view(width: f32) -> impl View {
    let source = schnellui::signal::create_signal(PREVIEW_SOURCE.to_string());
    let pane = ((width - 48.0 - 24.0) / 2.0).floor();
    stage(
        Row::new()
            .fill()
            .gap(24.0)
            .align(Align::Start)
            .child(
                Column::new()
                    .fill()
                    .width(pane)
                    .gap(8.0)
                    .child(pane_caption("MARKUP SOURCE"))
                    .child(
                        Flex::new().grow(1.0).shrink(1.0).child(
                            Scroll::new().scrollbar(true).child(
                                TextArea::new(PREVIEW_SOURCE)
                                    .placeholder("mini markup source")
                                    .size(15.0)
                                    .rows(25)
                                    .wrap(WrapMode::Word)
                                    .highlight(highlight_markup)
                                    .on_input(move |v| source.set(v.to_string())),
                            ),
                        ),
                    ),
            )
            .child(
                Column::new()
                    .fill()
                    .width(pane)
                    .gap(8.0)
                    .child(pane_caption("PREVIEW"))
                    .child(
                        Flex::new().grow(1.0).shrink(1.0).child(
                            Scroll::new()
                                .scrollbar(true)
                                .child(RichText::dynamic(move || parse_mini_markup(&source.get()))),
                        ),
                    ),
            ),
    )
}

/// Builds the app for a scenario in its target state (SOUL §7.5): `Document`
/// is constructed; `Editor`/`Preview` are *driven* there through the real
/// inbound paths (focus + edit keys / `SetValue`), proving the state is
/// reachable, not merely constructible.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::Document => {
            App::mount_with_size_scaled(document_view(width as f32), width, height, scale)
        }
        Scenario::Link => {
            let mut app =
                App::mount_with_size_scaled(document_view(width as f32), width, height, scale);
            app.frame();
            // click the link by role + name, through the inbound AccessKit path
            if let Some(id) = app.find_widget(Role::Link, Some("hyperlink")) {
                app.dispatch_action(&ActionRequest {
                    action: Action::Click,
                    target_tree: TreeId::ROOT,
                    target_node: to_access_id(id),
                    data: None,
                });
            }
            app
        }
        Scenario::Editor => {
            let mut app =
                App::mount_with_size_scaled(editor_view(width as f32), width, height, scale);
            app.frame();
            // focus the editor by role, then edit through the real key path:
            // append a comment line at the end of the source (SOUL §6.3).
            if let Some(id) = app.find_widget(Role::MultilineTextInput, None) {
                app.focus(Some(id));
                app.dispatch_edit_key(EditKey::End { select: false });
                app.dispatch_edit_key(EditKey::Enter);
                app.dispatch_edit_key(EditKey::Insert("// edited headlessly"));
                // select the word we just typed so the selection wash shows
                app.dispatch_edit_key(EditKey::Left {
                    select: true,
                    word: true,
                });
            }
            app
        }
        Scenario::Preview => {
            let mut app =
                App::mount_with_size_scaled(preview_view(width as f32), width, height, scale);
            app.frame();
            // drive a new source through the inbound AccessKit SetValue path;
            // on_input feeds the signal, the dynamic viewer re-flows (§6.3).
            if let Some(id) = app.find_widget(Role::MultilineTextInput, None) {
                let req = ActionRequest {
                    action: Action::SetValue,
                    target_tree: TreeId::ROOT,
                    target_node: to_access_id(id),
                    data: Some(ActionData::Value(DRIVEN_SOURCE.into())),
                };
                app.dispatch_action(&req);
            }
            app
        }
    }
}

/// The a11y oracle per scenario (SOUL §7.5): assert against roles, names, and
/// values — the semantic ground truth — before the pixels are even encoded.
fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    match scenario {
        Scenario::Document => {
            let doc = find_by_role_name(&tree, "document", None)
                .ok_or_else(|| "missing document".to_string())?;
            if doc.name.as_deref() != Some("schnellui rich text") {
                return Err(format!("document name: {:?}", doc.name));
            }
            assert_value_contains(&tree, "document", None, "bold")?;
            assert_value_contains(&tree, "document", None, "fn rerender")?;
            // the targeted span is a real link node: named by its text, a tab
            // stop that clicks; nothing has been opened yet
            let link = find_by_role_name(&tree, "link", Some("hyperlink"))
                .ok_or_else(|| "missing link node for the hyperlink".to_string())?;
            for action in ["click", "focus"] {
                if !link.actions.iter().any(|a| a == action) {
                    return Err(format!(
                        "link lacks the {action} action: {:?}",
                        link.actions
                    ));
                }
            }
            if find_by_role_name(&tree, "label", Some(NO_LINK)).is_none() {
                return Err("the status line should start at its idle text".to_string());
            }
            Ok(())
        }
        Scenario::Link => {
            let opened = format!("Opened link: {LINK_TARGET}");
            find_by_role_name(&tree, "label", Some(&opened))
                .ok_or_else(|| format!("status line never said {opened:?}"))?;
            if find_by_role_name(&tree, "label", Some(NO_LINK)).is_some() {
                return Err("the idle status text is still showing".to_string());
            }
            Ok(())
        }
        Scenario::Editor => {
            find_by_role_name(&tree, "multiline_text_input", None)
                .ok_or_else(|| "missing text area".to_string())?;
            assert_value_contains(&tree, "multiline_text_input", None, "// edited headlessly")?;
            // the edit was typed at the keyboard focus (SOUL §6.3)
            let focused = app
                .focused_widget()
                .ok_or_else(|| "nothing focused".to_string())?;
            if app.find_widget(Role::MultilineTextInput, None) != Some(focused) {
                return Err("focus is not on the editor".to_string());
            }
            Ok(())
        }
        Scenario::Preview => {
            assert_value_contains(&tree, "multiline_text_input", None, "Driven")?;
            // the dynamic viewer re-parsed the driven source: its accessible
            // name is the new first heading, its value the new body.
            let doc = find_by_role_name(&tree, "document", None)
                .ok_or_else(|| "missing preview document".to_string())?;
            if doc.name.as_deref() != Some("Driven") {
                return Err(format!("preview name not updated: {:?}", doc.name));
            }
            assert_value_contains(&tree, "document", None, "arrived via SetValue")?;
            Ok(())
        }
    }
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame(); // one synchronous frame (SOUL §7.1)

    if let Some(path) = &cli.dump_a11y {
        if let Err(e) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    if cli.assert {
        if let Err(e) = run_assertions(scenario, &app) {
            eprintln!("assertion failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    if let Err(e) = app.render_to_png(out) {
        eprintln!("render failed: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.headless_windowed {
        std::env::set_var("SCHNELLUI_HEADLESS_WINDOWED", "1");
    }

    if cli.list {
        for s in Scenario::iter() {
            println!("{}", s.name());
        }
        return ExitCode::SUCCESS;
    }

    if cli.all {
        let dir = cli.out_dir.clone().unwrap_or_else(|| ".".to_string());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("could not create out-dir {dir:?}: {e}");
            return ExitCode::FAILURE;
        }
        let pw = (cli.width as f32 * cli.scale).round().max(1.0) as u32;
        let ph = (cli.height as f32 * cli.scale).round().max(1.0) as u32;
        let mut manifest = Vec::new();
        for s in Scenario::iter() {
            let out = format!("{dir}/{}.png", s.name());
            let code = render_one(s, &cli, &out);
            if code != ExitCode::SUCCESS {
                return code;
            }
            manifest.push(manifest_entry(s.name(), &out, pw, ph));
        }
        if let Some(path) = &cli.manifest {
            let json = format!("[{}]", manifest.join(","));
            if let Err(e) = std::fs::write(path, json) {
                eprintln!("manifest write failed: {e}");
                return ExitCode::FAILURE;
            }
        }
        return ExitCode::SUCCESS;
    }

    let Some(scenario) = cli.scenario else {
        eprintln!("one of --scenario, --list, or --all is required");
        return ExitCode::FAILURE;
    };
    if cli.windowed || cli.headless_windowed {
        let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
        app.set_escape_exits(true);
        return match app.run_windowed("rich_text") {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("windowed run failed: {e}");
                ExitCode::FAILURE
            }
        };
    }

    let out = cli
        .out
        .clone()
        .unwrap_or_else(|| format!("{}.png", scenario.name()));
    let _cfg = SnapshotConfig::from_env("snapshots");
    render_one(scenario, &cli, &out)
}

/// One `--manifest` entry (SOUL §7.1).
fn manifest_entry(name: &str, path: &str, width: u32, height: u32) -> String {
    format!("{{\"scenario\":\"{name}\",\"path\":\"{path}\",\"width\":{width},\"height\":{height}}}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::testing::fuzz::tab_stops_reachable;

    /// At a phone width no text or control is cut at a side with nothing to
    /// scroll it in: prose wraps and fixed-width content reflows or scrolls.
    #[test]
    fn nothing_is_clipped_sideways_at_360() {
        use schnellui::testing::fuzz::sideways_clips;
        let mut faults = Vec::new();
        for scenario in Scenario::iter() {
            let mut app = scenario_app(scenario, 360, 600, 1.0);
            for fault in sideways_clips(&mut app) {
                faults.push(format!("{}: {fault}", scenario.name()));
            }
        }
        assert!(faults.is_empty(), "{faults:#?}");
    }

    /// Every Tab stop of every scenario can be shown, at the default size and
    /// in a narrow, short window: something must scroll it in. Below about
    /// 200px of height the pane captions alone fill the window, so that is not
    /// tried.
    #[test]
    fn every_tab_stop_reaches_the_window() {
        let mut stops = 0;
        for scenario in Scenario::iter() {
            for (w, h) in [(880, 640), (360, 240), (420, 300)] {
                let mut app = scenario_app(scenario, w, h, 1.0);
                stops += tab_stops_reachable(&mut app)
                    .unwrap_or_else(|e| panic!("{} at {w}x{h}: {e}", scenario.name()));
            }
        }
        assert!(stops > 0, "no scenario has a Tab stop");
    }

    /// Typing past the bottom of a pane keeps the caret in the window: the
    /// area scrolls inside its pane instead of growing off the page.
    #[test]
    fn caret_stays_in_the_window_while_typing_and_arrowing() {
        use schnellui::testing::fuzz::visible_in_window;
        use schnellui::UiKey;
        for scenario in [Scenario::Editor, Scenario::Preview] {
            for (w, h) in [(880, 640), (360, 240)] {
                let mut app = scenario_app(scenario, w, h, 1.0);
                app.frame();
                let id = app
                    .find_widget(Role::MultilineTextInput, None)
                    .expect("the text area");
                app.focus(Some(id));
                app.dispatch_key(UiKey::End {
                    shift: false,
                    ctrl: true,
                });
                for n in 0..40 {
                    app.dispatch_key(UiKey::Enter);
                    app.dispatch_key(UiKey::Char("x"));
                    app.frame();
                    assert!(
                        visible_in_window(&mut app, id),
                        "{scenario:?} at {w}x{h}: caret left the window after {n} lines"
                    );
                }
                for n in 0..60 {
                    app.dispatch_key(UiKey::Up { shift: false });
                    app.frame();
                    assert!(
                        visible_in_window(&mut app, id),
                        "{scenario:?} at {w}x{h}: caret left the window after {n} Up presses"
                    );
                }
                for n in 0..80 {
                    app.dispatch_key(UiKey::Down { shift: false });
                    app.frame();
                    assert!(
                        visible_in_window(&mut app, id),
                        "{scenario:?} at {w}x{h}: caret left the window after {n} Down presses"
                    );
                }
            }
        }
    }

    /// The editor and the markup source keep their top border on the viewport
    /// while the text scrolls: the frame is the field's, not part of the text.
    #[test]
    fn the_top_border_stays_while_the_text_scrolls() {
        use schnellui::UiKey;
        for scenario in [Scenario::Editor, Scenario::Preview] {
            for scale in [1.0_f32, 2.0] {
                let (w, h) = (880u32, 640u32);
                let mut app = scenario_app(scenario, w, h, scale);
                app.frame();
                let id = app
                    .find_widget(Role::MultilineTextInput, None)
                    .expect("the text area");
                let scroll = app
                    .scene()
                    .node(id)
                    .and_then(|n| n.parent)
                    .expect("the area sits in a scroll viewport");
                let view = app.scene().layout(scroll).expect("laid out").rect;
                // the row of the border that is on the viewport's top edge
                let (px, py) = (
                    ((view.x + view.width / 2.0) * scale) as usize,
                    (view.y * scale) as usize + 1,
                );
                let stride = (w as f32 * scale) as usize;
                let pixel = |app: &mut App| {
                    let rgba = app.render_rgba8().expect("a headless renderer");
                    let at = (py * stride + px) * 4;
                    [rgba[at], rgba[at + 1], rgba[at + 2]]
                };
                app.focus(Some(id));
                let before = pixel(&mut app);
                assert_ne!(before, [255; 3], "{scenario:?}: no border to start with");
                app.dispatch_key(UiKey::End {
                    shift: false,
                    ctrl: true,
                });
                for _ in 0..40 {
                    app.dispatch_key(UiKey::Enter);
                }
                app.frame();
                assert!(
                    app.scene().scroll_offset(scroll).y > 100.0,
                    "{scenario:?}: the text did not scroll"
                );
                assert_eq!(
                    pixel(&mut app),
                    before,
                    "{scenario:?} at scale {scale}: the top border scrolled away"
                );
            }
        }
    }
}
