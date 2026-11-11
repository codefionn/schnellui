//! A compact four-function calculator built entirely from schnellui widgets.
//!
//! Run it interactively with:
//! `cargo run -p calculator -- --scenario ready --windowed`

use std::cell::RefCell;
use std::process::ExitCode;
use std::rc::Rc;

use clap::{Parser, ValueEnum};
use schnellui::a11y::{self, to_access_id, A11yNodeDump, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::scene::Color;
use schnellui::signal::{create_signal, Signal};
use schnellui::testing::find_by_role_name;
use schnellui::widgets::{
    Align, Button, Column, Divider, Justify, Row, Scroll, Shape, Text, Theme, ThemeProvider, View,
    WrapMode,
};
use schnellui::{App, Shortcut};
use strum::IntoEnumIterator;

/// The most characters the readout shows before it would overflow.
const MAX_DIGITS: usize = 12;
const KEY_WIDTH: f32 = 72.0;
const KEY_HEIGHT: f32 = 52.0;
const KEY_GAP: f32 = 10.0;
/// Four keys and three gaps.
const PANEL_WIDTH: f32 = 4.0 * KEY_WIDTH + 3.0 * KEY_GAP;

/// Keyboard focus: 6.1:1 on the page, apart from every key colour.
const FOCUS: Color = Color::rgb(0x1f, 0x4f, 0xb0);
/// The error notice under the readout: 5.9:1 on the page.
const ERROR: Color = Color::rgb(0x9e, 0x2a, 0x14);

/// The dark function keys answer hover by lightening: a darker step would not
/// show on near-black.
const LIGHTEN_ON_HOVER: schnellui::widgets::ComponentInteractions =
    schnellui::widgets::ComponentInteractions {
        button: Some(schnellui::widgets::InteractionStates {
            hover: schnellui::widgets::InteractionStyle::background(Color::rgba(
                0xff, 0xff, 0xff, 0x38,
            )),
            focus: schnellui::widgets::InteractionStyle::border(FOCUS),
            active: schnellui::widgets::InteractionStyle::background(Color::rgba(
                0x20, 0x1d, 0x18, 0x4c,
            )),
        }),
        ..schnellui::widgets::ComponentInteractions::NONE
    };

const CALCULATOR_THEME: Theme = Theme {
    text: Color::rgb(0x20, 0x1d, 0x18),
    text_muted: Color::rgb(0x70, 0x69, 0x5d),
    surface: Color::rgb(0xff, 0xfc, 0xf2),
    surface_muted: Color::rgb(0xe8, 0xe0, 0xcf),
    separator: Color::rgb(0x8c, 0x82, 0x72),
    outline: Color::rgb(0x20, 0x1d, 0x18),
    // Deep enough that the paper-coloured key labels reach 4.5:1.
    accent: Color::rgb(0xc2, 0x46, 0x1f),
    on_accent: Color::rgb(0xff, 0xfc, 0xf2),
    selection: Color::rgb(0xf5, 0xc7, 0xa9),
    interactions: schnellui::widgets::InteractionStates {
        // A key under the pointer darkens a step and a pressed key a step
        // more; neither changes its label, so hover never reads as pressed.
        hover: schnellui::widgets::InteractionStyle::background(Color::rgba(
            0x20, 0x1d, 0x18, 0x26,
        )),
        // The focus ring is blue, a colour no key family wears.
        focus: schnellui::widgets::InteractionStyle::border(FOCUS),
        active: schnellui::widgets::InteractionStyle::background(Color::rgba(
            0x20, 0x1d, 0x18, 0x4c,
        )),
    },
    component_interactions: schnellui::widgets::ComponentInteractions::NONE,
    text_selection: Color::rgb(0xf0, 0xa9, 0x7c),
    disabled: Color::rgb(0xa9, 0xa1, 0x94),
    positive: Color::rgb(0x32, 0x75, 0x51),
    attention: Color::rgb(0x20, 0x1d, 0x18),
    media: Color::rgb(0xd6, 0xce, 0xbf),
    page: Color::rgb(0xd9, 0xd0, 0xbf),
    shape: Shape {
        roundness: 0.35,
        density: 1.45,
        frame: 1.5,
        shadow: 3.0,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, strum::EnumIter)]
#[clap(rename_all = "snake_case")]
enum Scenario {
    Ready,
    Calculated,
    Percentage,
    DivideByZero,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Calculated => "calculated",
            Self::Percentage => "percentage",
            Self::DivideByZero => "divide_by_zero",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "calculator", about = "A schnellui calculator app")]
struct Cli {
    #[arg(long, value_enum)]
    scenario: Option<Scenario>,
    #[arg(long)]
    out: Option<String>,
    #[arg(long, default_value_t = 430)]
    width: u32,
    #[arg(long, default_value_t = 610)]
    height: u32,
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    #[arg(long)]
    list: bool,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    out_dir: Option<String>,
    #[arg(long)]
    dump_a11y: Option<String>,
    #[arg(long)]
    assert: bool,
    #[arg(long)]
    windowed: bool,
    /// Run the live event loop offscreen without opening a desktop window.
    #[arg(long, conflicts_with = "windowed")]
    headless_windowed: bool,
}

#[derive(Clone, Copy)]
enum Operator {
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl Operator {
    fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "−",
            Self::Multiply => "×",
            Self::Divide => "÷",
        }
    }

    fn apply(self, lhs: f64, rhs: f64) -> Option<f64> {
        match self {
            Self::Add => Some(lhs + rhs),
            Self::Subtract => Some(lhs - rhs),
            Self::Multiply => Some(lhs * rhs),
            Self::Divide if rhs == 0.0 => None,
            Self::Divide => Some(lhs / rhs),
        }
        .filter(|value| value.is_finite())
    }
}

#[derive(Clone, Copy)]
enum Key {
    Digit(char),
    Decimal,
    Operator(Operator),
    Equals,
    Clear,
    Backspace,
    ToggleSign,
    Percent,
}

struct CalculatorState {
    current: String,
    history: String,
    lhs: Option<f64>,
    pending: Option<Operator>,
    replace_current: bool,
}

impl Default for CalculatorState {
    fn default() -> Self {
        Self {
            current: "0".into(),
            history: String::new(),
            lhs: None,
            pending: None,
            replace_current: false,
        }
    }
}

impl CalculatorState {
    fn press(&mut self, key: Key) {
        match key {
            Key::Digit(digit) => self.push_digit(digit),
            Key::Decimal => self.push_decimal(),
            Key::Operator(op) => self.choose_operator(op),
            Key::Equals => self.equals(),
            Key::Clear => *self = Self::default(),
            Key::Backspace => self.backspace(),
            Key::ToggleSign => self.toggle_sign(),
            Key::Percent => self.percent(),
        }
    }

    fn push_digit(&mut self, digit: char) {
        if self.current == "Error" {
            *self = Self::default();
        }
        if self.replace_current {
            self.current.clear();
            self.replace_current = false;
        }
        if self.current == "0" {
            self.current.clear();
        }
        if self.current.len() < MAX_DIGITS {
            self.current.push(digit);
        }
    }

    fn push_decimal(&mut self) {
        if self.current == "Error" {
            *self = Self::default();
        }
        if self.replace_current {
            self.current = "0".into();
            self.replace_current = false;
        }
        if !self.current.contains('.') {
            self.current.push('.');
        }
    }

    fn choose_operator(&mut self, op: Operator) {
        let Some(value) = self.value() else {
            return;
        };
        let value = if let (Some(lhs), Some(pending)) = (self.lhs, self.pending) {
            if self.replace_current {
                lhs
            } else if let Some(result) = pending.apply(lhs, value) {
                self.current = format_number(result);
                result
            } else {
                self.show_error();
                return;
            }
        } else {
            value
        };
        self.lhs = Some(value);
        self.pending = Some(op);
        self.history = format!("{} {}", format_number(value), op.symbol());
        self.replace_current = true;
    }

    fn equals(&mut self) {
        let (Some(lhs), Some(op), Some(rhs)) = (self.lhs, self.pending, self.value()) else {
            return;
        };
        if self.history.ends_with('%') {
            self.history.push_str(" =");
        } else {
            self.history = format!(
                "{} {} {} =",
                format_number(lhs),
                op.symbol(),
                format_number(rhs)
            );
        }
        match op.apply(lhs, rhs) {
            Some(result) => self.current = format_number(result),
            None => self.show_error(),
        }
        self.lhs = None;
        self.pending = None;
        self.replace_current = true;
    }

    fn backspace(&mut self) {
        if self.replace_current || self.current == "Error" {
            return;
        }
        self.current.pop();
        if self.current.is_empty() || self.current == "-" {
            self.current = "0".into();
        }
    }

    fn toggle_sign(&mut self) {
        let Some(value) = self.value() else {
            return;
        };
        self.current = format_number(-value);
    }

    fn percent(&mut self) {
        let Some(value) = self.value() else {
            return;
        };
        let percentage = match (self.lhs, self.pending) {
            // On desktop calculators, +/- percentages are relative to the first
            // operand: `200 + 10 %` means `200 + (10% of 200)`.
            (Some(lhs), Some(Operator::Add | Operator::Subtract)) if !self.replace_current => {
                lhs * value / 100.0
            }
            // For multiplication/division and standalone input, `%` is the
            // conventional conversion from a percentage to its decimal value.
            _ => value / 100.0,
        };
        if let (Some(lhs), Some(op)) = (self.lhs, self.pending) {
            self.history = format!(
                "{} {} {}%",
                format_number(lhs),
                op.symbol(),
                format_number(value)
            );
        } else {
            self.history = format!("{}%", format_number(value));
        }
        self.current = format_number(percentage);
        self.replace_current = true;
    }

    fn value(&self) -> Option<f64> {
        self.current.parse().ok()
    }

    fn show_error(&mut self) {
        self.current = "Error".into();
    }
}

fn format_number(value: f64) -> String {
    if value.abs() < 1e-12 {
        return "0".into();
    }
    if value.fract().abs() < 1e-10 && value.abs() < 1e12 {
        return format!("{value:.0}");
    }
    let mut text = format!("{value:.10}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text.len() > MAX_DIGITS {
        // Too long for the readout: fall back to scientific notation.
        return format!("{value:.5e}");
    }
    text
}

fn calc_action(
    key: Key,
    state: Rc<RefCell<CalculatorState>>,
    display: Signal<String>,
    history: Signal<String>,
) -> impl FnMut() {
    move || {
        let mut state = state.borrow_mut();
        state.press(key);
        display.set(state.current.clone());
        history.set(state.history.clone());
    }
}

/// The visual family of a key: digits are light, the functions that edit the
/// entry are dark, and the operators carry the accent.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyKind {
    Digit,
    Function,
    Operator,
}

/// One keypad key: its accessible name, the glyph painted on it, its family,
/// the keyboard characters that press it, and what it does.
struct KeySpec {
    name: &'static str,
    glyph: &'static str,
    kind: KeyKind,
    typed: &'static [char],
    key: Key,
}

const fn spec(
    name: &'static str,
    glyph: &'static str,
    kind: KeyKind,
    typed: &'static [char],
    key: Key,
) -> KeySpec {
    KeySpec {
        name,
        glyph,
        kind,
        typed,
        key,
    }
}

const KEYPAD: [[KeySpec; 4]; 5] = [
    [
        spec("clear", "AC", KeyKind::Function, &['c', 'C'], Key::Clear),
        spec(
            "toggle sign",
            "+/−",
            KeyKind::Function,
            &['n'],
            Key::ToggleSign,
        ),
        spec("percent", "%", KeyKind::Function, &['%'], Key::Percent),
        spec(
            "divide",
            "÷",
            KeyKind::Operator,
            &['/', '÷'],
            Key::Operator(Operator::Divide),
        ),
    ],
    [
        spec("7", "7", KeyKind::Digit, &['7'], Key::Digit('7')),
        spec("8", "8", KeyKind::Digit, &['8'], Key::Digit('8')),
        spec("9", "9", KeyKind::Digit, &['9'], Key::Digit('9')),
        spec(
            "multiply",
            "×",
            KeyKind::Operator,
            &['*', 'x', 'X', '×'],
            Key::Operator(Operator::Multiply),
        ),
    ],
    [
        spec("4", "4", KeyKind::Digit, &['4'], Key::Digit('4')),
        spec("5", "5", KeyKind::Digit, &['5'], Key::Digit('5')),
        spec("6", "6", KeyKind::Digit, &['6'], Key::Digit('6')),
        spec(
            "subtract",
            "−",
            KeyKind::Operator,
            &['-', '−'],
            Key::Operator(Operator::Subtract),
        ),
    ],
    [
        spec("1", "1", KeyKind::Digit, &['1'], Key::Digit('1')),
        spec("2", "2", KeyKind::Digit, &['2'], Key::Digit('2')),
        spec("3", "3", KeyKind::Digit, &['3'], Key::Digit('3')),
        spec(
            "add",
            "+",
            KeyKind::Operator,
            &['+'],
            Key::Operator(Operator::Add),
        ),
    ],
    [
        spec("0", "0", KeyKind::Digit, &['0'], Key::Digit('0')),
        spec(
            "decimal point",
            ".",
            KeyKind::Digit,
            &['.', ','],
            Key::Decimal,
        ),
        spec(
            "delete",
            "DEL",
            KeyKind::Function,
            &['d', 'D', '\u{8}', '\u{7f}'],
            Key::Backspace,
        ),
        spec("equals", "=", KeyKind::Operator, &['=', '\r'], Key::Equals),
    ],
];

/// The dark function keys: the calculator theme with ink as the key fill.
const FUNCTION_KEY_THEME: Theme = Theme {
    accent: Color::rgb(0x33, 0x2e, 0x27),
    on_accent: Color::rgb(0xff, 0xfc, 0xf2),
    component_interactions: LIGHTEN_ON_HOVER,
    ..CALCULATOR_THEME
};

/// The accent operator keys darken a full step under the pointer.
const OPERATOR_KEY_THEME: Theme = Theme {
    component_interactions: schnellui::widgets::ComponentInteractions {
        button: Some(schnellui::widgets::InteractionStates {
            hover: schnellui::widgets::InteractionStyle::background(Color::rgba(
                0x20, 0x1d, 0x18, 0x58,
            )),
            focus: schnellui::widgets::InteractionStyle::border(FOCUS),
            active: schnellui::widgets::InteractionStyle::background(Color::rgba(
                0x20, 0x1d, 0x18, 0x80,
            )),
        }),
        ..schnellui::widgets::ComponentInteractions::NONE
    },
    ..CALCULATOR_THEME
};

/// The digit keys: paper-coloured, with the same block shadow as the rest.
const DIGIT_KEY_THEME: Theme = Theme {
    accent: Color::rgb(0xff, 0xfc, 0xf2),
    on_accent: Color::rgb(0x20, 0x1d, 0x18),
    ..CALCULATOR_THEME
};

/// The pending operator in the readout, in the operator keys' colour.
const PENDING_THEME: Theme = Theme {
    text: Color::rgb(0xa8, 0x3a, 0x17),
    ..CALCULATOR_THEME
};

/// The error notice.
const ERROR_THEME: Theme = Theme {
    text: ERROR,
    ..CALCULATOR_THEME
};

/// Secondary text: the expression line and the keyboard hint.
const MUTED_THEME: Theme = Theme {
    text: Color::rgb(0x5c, 0x55, 0x4a),
    ..CALCULATOR_THEME
};

/// One keypad key, coloured by its family.
fn key_view(spec: &KeySpec, on_click: impl FnMut() + 'static) -> Box<dyn View> {
    let button = Button::new(spec.name)
        .display(spec.glyph)
        .width(KEY_WIDTH)
        .height(KEY_HEIGHT)
        .text_size(if spec.kind == KeyKind::Function {
            18.0
        } else {
            22.0
        })
        .on_click(on_click);
    let theme = match spec.kind {
        KeyKind::Digit => DIGIT_KEY_THEME,
        KeyKind::Function => FUNCTION_KEY_THEME,
        KeyKind::Operator => OPERATOR_KEY_THEME,
    };
    Box::new(ThemeProvider::new(theme, button))
}

/// A full-width row that pushes its text to the right edge, the way a
/// calculator readout reads.
fn readout(text: impl View + 'static) -> Row {
    Row::new()
        .width(PANEL_WIDTH)
        .justify(Justify::End)
        .child(text)
}

fn calculator_view(
    state: Rc<RefCell<CalculatorState>>,
    display: Signal<String>,
    history: Signal<String>,
) -> impl View {
    let mut keypad = Column::new().gap(KEY_GAP);
    for row in &KEYPAD {
        let mut keys = Row::new().gap(KEY_GAP);
        for spec in row {
            keys = keys.child(key_view(
                spec,
                calc_action(spec.key, Rc::clone(&state), display, history),
            ));
        }
        keypad = keypad.child(keys);
    }

    // The operator waiting for its second operand, shown at the readout's
    // left edge until the next digit or `=`.
    let pending = move || {
        let history = history.get();
        match history.chars().last() {
            Some(c @ ('+' | '−' | '×' | '÷')) => c.to_string(),
            _ => String::new(),
        }
    };
    let screen = Column::new()
        .width(PANEL_WIDTH)
        .gap(4.0)
        .child(readout(ThemeProvider::new(
            MUTED_THEME,
            Text::dynamic(move || history.get())
                .size(16.0)
                .role(Role::Status),
        )))
        .child(
            Row::new()
                .width(PANEL_WIDTH)
                .justify(Justify::SpaceBetween)
                .align(Align::Center)
                .child(ThemeProvider::new(
                    PENDING_THEME,
                    Text::dynamic(pending).size(30.0).bold(),
                ))
                .child(
                    Text::dynamic(move || display.get())
                        .size(46.0)
                        .role(Role::Status),
                ),
        )
        .child(readout(ThemeProvider::new(
            ERROR_THEME,
            Text::dynamic(move || {
                if display.get() == "Error" {
                    "Can't divide by zero. Press AC to start over.".to_string()
                } else {
                    String::new()
                }
            })
            .size(13.0)
            .bold()
            .role(Role::Status),
        )));

    // A window shorter than the panel scrolls it, so every key can be shown.
    // The Scroll sits inside the centering column and shrinks to the window,
    // rather than filling it: a viewport sizes its scroll range from the
    // content, so a full-height child would never overflow.
    Column::new()
        .fill()
        .align(Align::Center)
        .justify(Justify::Center)
        .child(
            Scroll::new().scrollbar(true).child(
                Column::new()
                    .width(PANEL_WIDTH)
                    .gap(14.0)
                    .child(Text::new("Calculator").size(20.0).bold())
                    .child(Divider::new())
                    .child(screen)
                    .child(Divider::new())
                    .child(keypad)
                    .child(ThemeProvider::new(
                        MUTED_THEME,
                        Text::new("Keyboard: digits, + − * / and = or Enter. C clears, D or Backspace deletes.")
                            .size(12.0)
                            .wrap(WrapMode::Word),
                    )),
            ),
        )
}

/// Lets the physical keyboard press the keypad: every character in a key's
/// `typed` list presses that key.
fn register_typing(
    app: &mut App,
    state: &Rc<RefCell<CalculatorState>>,
    display: Signal<String>,
    history: Signal<String>,
) {
    for spec in KEYPAD.iter().flatten() {
        for &c in spec.typed {
            app.register_shortcut(
                Shortcut::new(c, false, false, false),
                calc_action(spec.key, Rc::clone(state), display, history),
            );
            // Shifted symbols (+ * % on most layouts) arrive with Shift held.
            app.register_shortcut(
                Shortcut::new(c, false, true, false),
                calc_action(spec.key, Rc::clone(state), display, history),
            );
        }
    }
}

fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    let state = Rc::new(RefCell::new(CalculatorState::default()));
    let display = create_signal("0".to_string());
    let history = create_signal(String::new());
    let mut app = App::mount_with_theme_size_scaled(
        CALCULATOR_THEME,
        calculator_view(Rc::clone(&state), display, history),
        width,
        height,
        scale,
    );
    app.set_clear_color(CALCULATOR_THEME.page);
    register_typing(&mut app, &state, display, history);
    match scenario {
        Scenario::Ready => {}
        Scenario::Calculated => drive(
            &mut app,
            &["1", "2", "decimal point", "5", "multiply", "4", "equals"],
        ),
        Scenario::Percentage => drive(
            &mut app,
            &["2", "0", "0", "add", "1", "0", "percent", "equals"],
        ),
        Scenario::DivideByZero => drive(&mut app, &["8", "divide", "0", "equals"]),
    }
    app
}

fn drive(app: &mut App, labels: &[&str]) {
    for label in labels {
        let Some(id) = app.find_widget(Role::Button, Some(label)) else {
            eprintln!("drive: missing calculator key {label:?}");
            return;
        };
        app.dispatch_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: to_access_id(id),
            data: None,
        });
    }
}

fn find_status_value<'a>(node: &'a A11yNodeDump, needle: &str) -> Option<&'a str> {
    if node.role == "status" && node.value.as_deref().is_some_and(|v| v.contains(needle)) {
        return node.value.as_deref();
    }
    node.children
        .iter()
        .find_map(|child| find_status_value(child, needle))
}

fn run_assertions(scenario: Scenario, app: &App) -> Result<(), String> {
    let tree = a11y::dump_tree(app.scene());
    let root = tree.root.as_ref().ok_or("empty a11y tree")?;
    find_by_role_name(&tree, "button", Some("equals")).ok_or("missing equals key")?;
    match scenario {
        Scenario::Ready => find_status_value(root, "0")
            .map(|_| ())
            .ok_or_else(|| "display is not zero".into()),
        Scenario::Calculated => {
            find_status_value(root, "50").ok_or("display is not 50")?;
            find_status_value(root, "12.5 × 4 =").ok_or("calculation history is missing")?;
            Ok(())
        }
        Scenario::Percentage => {
            find_status_value(root, "220").ok_or("200 + 10% did not produce 220")?;
            find_status_value(root, "200 + 10% =").ok_or("percentage history is missing")?;
            Ok(())
        }
        Scenario::DivideByZero => {
            find_status_value(root, "Error").ok_or("error display is missing")?;
            find_status_value(root, "Can't divide by zero").ok_or("error context is missing")?;
            find_status_value(root, "8 ÷ 0 =").ok_or("error history is missing")?;
            Ok(())
        }
    }
}

fn render_one(scenario: Scenario, cli: &Cli, out: &str) -> ExitCode {
    let mut app = scenario_app(scenario, cli.width, cli.height, cli.scale);
    app.frame();
    if let Some(path) = &cli.dump_a11y {
        if let Err(error) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    if cli.assert {
        if let Err(error) = run_assertions(scenario, &app) {
            eprintln!("assertion failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    if let Err(error) = app.render_to_png(out) {
        eprintln!("render failed: {error}");
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
        for scenario in Scenario::iter() {
            println!("{}", scenario.name());
        }
        return ExitCode::SUCCESS;
    }
    if cli.all {
        let dir = cli.out_dir.clone().unwrap_or_else(|| ".".into());
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!("could not create out-dir: {error}");
            return ExitCode::FAILURE;
        }
        for scenario in Scenario::iter() {
            let out = format!("{dir}/{}.png", scenario.name());
            if render_one(scenario, &cli, &out) != ExitCode::SUCCESS {
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
        return match app.run_windowed("calculator") {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("windowed run failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let out = cli
        .out
        .clone()
        .unwrap_or_else(|| format!("{}.png", scenario.name()));
    render_one(scenario, &cli, &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::testing::fuzz::tab_stops_reachable;

    /// Resize the live window: every scenario lays out as a fresh mount at the
    /// new size does, so no size from the launch is left in the page.
    #[test]
    fn a_live_resize_matches_a_fresh_mount() {
        schnellui::testing::fuzz::assert_resize_matches_fresh(
            Scenario::iter(),
            (430, 610),
            (500, 700),
            &[1.0, 1.5],
            scenario_app,
        );
    }

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

    /// At 360x240 the panel overflows and its scrollbar shows. The bar sits in a
    /// strip of its own: no key or display text reaches under it.
    #[test]
    fn the_scrollbar_covers_no_key_at_360x240() {
        for scenario in Scenario::iter() {
            let mut app = scenario_app(scenario, 360, 240, 1.0);
            app.frame();
            assert!(
                !schnellui::testing::fuzz::scrollbars(&app).is_empty(),
                "{}: the panel should overflow here",
                scenario.name()
            );
            let covered = schnellui::testing::fuzz::scrollbar_overlaps(&app);
            assert!(covered.is_empty(), "{}: {covered:#?}", scenario.name());
        }
    }

    /// Every Tab stop of every scenario can be shown, at the default size and
    /// in a narrow, short window: something must scroll it in.
    #[test]
    fn every_tab_stop_reaches_the_window() {
        let mut stops = 0;
        for scenario in Scenario::iter() {
            for (w, h) in [(430, 610), (360, 240), (360, 120)] {
                let mut app = scenario_app(scenario, w, h, 1.0);
                stops += tab_stops_reachable(&mut app)
                    .unwrap_or_else(|e| panic!("{} at {w}x{h}: {e}", scenario.name()));
            }
        }
        assert!(stops > 0, "no scenario has a Tab stop");
    }

    /// At the default size the panel fits, at any scale: float layout may leave
    /// it a hair over the viewport, which must not draw a bar or scroll.
    #[test]
    fn a_fitting_panel_has_no_scrollbar_at_any_scale() {
        for scale in [1.0, 1.5, 2.0] {
            let mut app = scenario_app(Scenario::Ready, 430, 610, scale);
            app.frame();
            assert!(
                schnellui::testing::fuzz::scrollbars(&app).is_empty(),
                "scrollbar at scale {scale}"
            );
        }
    }
}
