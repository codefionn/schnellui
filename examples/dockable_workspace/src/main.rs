use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use schnellui::a11y::Role;
use schnellui::charts::{BarChart, LineChart};
use schnellui::scene::{Color, Point, Primitive, Rect, Size, WidgetId, WidgetKind};
use schnellui::widgets::{
    node_rect, Align, Badge, BuildCtx, Button, ButtonAppearance, Column, Divider, DockArea,
    DragHandle, DragRelease, Justify, Pad, ProgressBar, Row, Scroll, Spacer, Stack, Switch, Tab,
    TabBar, Text, View, WrapMode,
};
use schnellui::{App, Context};
use strum::IntoEnumIterator;

mod runtime;
mod state;
#[cfg(test)]
mod tests;
mod views;

pub(crate) use state::*;
pub(crate) use views::*;

fn main() -> ExitCode {
    runtime::main()
}
