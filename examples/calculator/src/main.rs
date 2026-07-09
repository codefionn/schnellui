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
use schnellui::view;
use schnellui::widgets::{Shape, Theme, View};
use schnellui::App;
use schnellui_testing::find_by_role_name;
use strum::IntoEnumIterator;

const KEY_WIDTH: f32 = 64.0;

const CALCULATOR_THEME: Theme = Theme {
    text: Color::rgb(0x20, 0x1d, 0x18),
    text_muted: Color::rgb(0x70, 0x69, 0x5d),
    surface: Color::rgb(0xff, 0xfc, 0xf2),
    surface_muted: Color::rgb(0xe8, 0xe0, 0xcf),
    separator: Color::rgb(0x8c, 0x82, 0x72),
    outline: Color::rgb(0x20, 0x1d, 0x18),
    accent: Color::rgb(0xe3, 0x5d, 0x35),
    on_accent: Color::rgb(0xff, 0xfc, 0xf2),
    selection: Color::rgb(0xf5, 0xc7, 0xa9),
    interactions: schnellui::widgets::InteractionStates {
        hover: schnellui::widgets::InteractionStyle::all(
            Color::rgba(0xe3, 0x5d, 0x35, 0x20),
            Color::rgb(0x20, 0x1d, 0x18),
            Color::rgb(0xe3, 0x5d, 0x35),
        ),
        focus: schnellui::widgets::InteractionStyle::border(Color::rgb(0xe3, 0x5d, 0x35)),
        active: schnellui::widgets::InteractionStyle::background(Color::rgb(0xf5, 0xc7, 0xa9)),
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
            Self::Subtract => "-",
            Self::Multiply => "x",
            Self::Divide => "/",
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
            history: "READY".into(),
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
        if self.current.len() < 14 {
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
        self.history = "CANNOT DIVIDE BY ZERO".into();
    }
}

fn format_number(value: f64) -> String {
    if value.abs() < 1e-12 {
        return "0".into();
    }
    if value.fract().abs() < 1e-10 && value.abs() < 1e14 {
        return format!("{value:.0}");
    }
    let mut text = format!("{value:.10}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
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

/// The calculator's static structure is expressed with `view!`; only the button
/// actions are factored into a helper so the example keeps the state transition in
/// one place. The macro lowers this directly to the equivalent typed widget chain.
fn calculator_view() -> impl View {
    let state = Rc::new(RefCell::new(CalculatorState::default()));
    let display = create_signal("0".to_string());
    let history = create_signal("READY".to_string());
    let action = |key| calc_action(key, Rc::clone(&state), display, history);

    view! {
        column(fill, align = center, justify = center) {
            pad(all = 24.0) {
                column(width = 326.0, gap = 14.0) {
                    row(justify = space_between, align = center) {
                        text(size = 12.0) { "S-01 / DESK CALCULATOR" }
                        badge { "LIVE" }
                    }
                    text(size = 28.0) { "MAKE IT COUNT." }
                    divider
                    text(size = 12.0, role = Role::Status, align = end, ellipsis) {
                        (history.get())
                    }
                    text(size = 38.0, role = Role::Status, align = end, ellipsis) {
                        (display.get())
                    }
                    divider
                    row(gap = 10.0) {
                        button(width = KEY_WIDTH, on:click = action(Key::Clear)) { "AC" }
                        button(width = KEY_WIDTH, on:click = action(Key::ToggleSign)) { "+/-" }
                        button(width = KEY_WIDTH, on:click = action(Key::Percent)) { "%" }
                        button(
                            width = KEY_WIDTH,
                            on:click = action(Key::Operator(Operator::Divide))
                        ) { "/" }
                    }
                    row(gap = 10.0) {
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('7'))) { "7" }
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('8'))) { "8" }
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('9'))) { "9" }
                        button(
                            width = KEY_WIDTH,
                            on:click = action(Key::Operator(Operator::Multiply))
                        ) { "x" }
                    }
                    row(gap = 10.0) {
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('4'))) { "4" }
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('5'))) { "5" }
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('6'))) { "6" }
                        button(
                            width = KEY_WIDTH,
                            on:click = action(Key::Operator(Operator::Subtract))
                        ) { "-" }
                    }
                    row(gap = 10.0) {
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('1'))) { "1" }
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('2'))) { "2" }
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('3'))) { "3" }
                        button(
                            width = KEY_WIDTH,
                            on:click = action(Key::Operator(Operator::Add))
                        ) { "+" }
                    }
                    row(gap = 10.0) {
                        button(width = KEY_WIDTH, on:click = action(Key::Digit('0'))) { "0" }
                        button(width = KEY_WIDTH, on:click = action(Key::Decimal)) { "." }
                        button(width = KEY_WIDTH, on:click = action(Key::Backspace)) { "DEL" }
                        button(width = KEY_WIDTH, on:click = action(Key::Equals)) { "=" }
                    }
                    text(size = 11.0) { "TAB TO MOVE / ENTER TO PRESS" }
                }
            }
        }
    }
}

fn scenario_app(scenario: Scenario, width: u32, height: u32, scale: f32) -> App {
    let mut app = App::mount_with_theme_size_scaled(
        CALCULATOR_THEME,
        calculator_view(),
        width,
        height,
        scale,
    );
    app.set_clear_color(CALCULATOR_THEME.page);
    match scenario {
        Scenario::Ready => {}
        Scenario::Calculated => drive(&mut app, &["1", "2", ".", "5", "x", "4", "="]),
        Scenario::Percentage => drive(&mut app, &["2", "0", "0", "+", "1", "0", "%", "="]),
        Scenario::DivideByZero => drive(&mut app, &["8", "/", "0", "="]),
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
    find_by_role_name(&tree, "button", Some("=")).ok_or("missing equals key")?;
    match scenario {
        Scenario::Ready => find_status_value(root, "0")
            .map(|_| ())
            .ok_or_else(|| "display is not zero".into()),
        Scenario::Calculated => {
            find_status_value(root, "50").ok_or("display is not 50")?;
            find_status_value(root, "12.5 x 4 =").ok_or("calculation history is missing")?;
            Ok(())
        }
        Scenario::Percentage => {
            find_status_value(root, "220").ok_or("200 + 10% did not produce 220")?;
            find_status_value(root, "200 + 10% =").ok_or("percentage history is missing")?;
            Ok(())
        }
        Scenario::DivideByZero => {
            find_status_value(root, "Error").ok_or("error display is missing")?;
            find_status_value(root, "CANNOT DIVIDE BY ZERO").ok_or("error context is missing")?;
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
    if cli.windowed {
        return match scenario_app(scenario, cli.width, cli.height, cli.scale)
            .run_windowed("calculator")
        {
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
