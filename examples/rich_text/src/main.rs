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
//! AccessKit `SetValue` path (SOUL §6.3, §7.5).

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, Role};
use schnellui::accesskit_action::{Action, ActionData, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::Color;
use schnellui::widgets::{
    Column, EditKey, Pad, RichDoc, RichSpan, RichText, Row, SpanStyle, TextArea, View,
};
use schnellui::App;
use schnellui_testing::{assert_value_contains, find_by_role_name, SnapshotConfig};
use strum::IntoEnumIterator;

// ---------------------------------------------------------------------------
// the implementor-side styling (SOUL: parsing is application code)
// ---------------------------------------------------------------------------

/// The example's syntax palette (any colors work; the framework just draws).
const KEYWORD: Color = Color::rgb(0x88, 0x33, 0xaa);
const STRING: Color = Color::rgb(0x22, 0x77, 0x33);
const NUMBER: Color = Color::rgb(0x0a, 0x66, 0xaa);
const COMMENT: Color = Color::rgb(0x88, 0x88, 0x88);

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
            RichSpan::link("hyperlink"),
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

const EDITOR_SOURCE: &str = r#"fn main() {
    let greeting = "hello, schnellui";
    println!("{greeting}: {}", 42);
}"#;

const PREVIEW_SOURCE: &str = "# Live preview\n\nEdit the source, watch the document.\n\n- editor on the left\n- viewer on the right\n\n> driven through AccessKit";

// ---------------------------------------------------------------------------
// scenarios (SOUL §7.1 enumerable table)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    /// the viewer over the hand-built showcase document — constructed.
    Document,
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
    #[arg(long, default_value_t = 560)]
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
}

fn document_view() -> impl View {
    Pad::all(16.0).child(RichText::new(showcase_doc()))
}

fn editor_view() -> impl View {
    Pad::all(16.0).child(
        Column::new()
            .gap(8.0)
            .child(RichText::new(RichDoc::new().heading(2, ["Source editor"])))
            .child(
                TextArea::new(EDITOR_SOURCE)
                    .placeholder("type some rust")
                    .highlight(highlight_rust),
            ),
    )
}

fn preview_view() -> impl View {
    let source = schnellui::signal::create_signal(PREVIEW_SOURCE.to_string());
    Pad::all(16.0).child(
        Row::new()
            .gap(16.0)
            .child(
                TextArea::new(PREVIEW_SOURCE)
                    .placeholder("mini markup source")
                    .on_input(move |v| source.set(v.to_string())),
            )
            .child(RichText::dynamic(move || parse_mini_markup(&source.get()))),
    )
}

/// Builds the app for a scenario in its target state (SOUL §7.5): `Document`
/// is constructed; `Editor`/`Preview` are *driven* there through the real
/// inbound paths (focus + edit keys / `SetValue`), proving the state is
/// reachable, not merely constructible.
fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    match scenario {
        Scenario::Document => App::mount_with_size_scaled(document_view(), width, height, scale),
        Scenario::Editor => {
            let mut app = App::mount_with_size_scaled(editor_view(), width, height, scale);
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
            let mut app = App::mount_with_size_scaled(preview_view(), width, height, scale);
            app.frame();
            // drive a new source through the inbound AccessKit SetValue path;
            // on_input feeds the signal, the dynamic viewer re-flows (§6.3).
            if let Some(id) = app.find_widget(Role::MultilineTextInput, None) {
                let req = ActionRequest {
                    action: Action::SetValue,
                    target_tree: TreeId::ROOT,
                    target_node: to_access_id(id),
                    data: Some(ActionData::Value(
                        "# Driven\n\nThis body arrived via SetValue.\n\n- and re-flowed live"
                            .into(),
                    )),
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
    if cli.windowed {
        let app = scenario_app(scenario, cli.width, cli.height, cli.scale);
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
