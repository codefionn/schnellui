use std::hint::black_box;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use clap::Parser;

use schnellui::a11y::Role;
use schnellui::layout::{Align, Container, ContainerStyle};
use schnellui::scene::{Color, ComponentRef, Point, Primitive, Rect, WidgetKind};
use schnellui::signal::{create_effect, create_memo, create_signal, Runtime};
use schnellui::widgets::{Button, Column, Flex, Scroll, Text, TextInput, View, WrapMode};
use schnellui::{App, TestValue};

mod cli;
mod config;
mod measure;
mod workloads;

pub(crate) use config::*;
pub(crate) use measure::*;

fn main() -> std::process::ExitCode {
    cli::main()
}
