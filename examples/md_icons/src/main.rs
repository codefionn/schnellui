//! Material Design icon gallery.
//!
//! The rows intentionally repeat the same icon at equal physical sizes with
//! different tints. Those instances share one parsed document, one CPU raster,
//! and one scene/GPU atlas allocation; tint remains cheap per-instance draw data.

use std::process::ExitCode;

use clap::Parser;
use schnellui::scene::Color;
use schnellui::widgets::{Column, Pad, Row, Text, View};
use schnellui::App;
use schnellui_icons::cache_stats;
use schnellui_icons_md::{outlined, rounded, sharp, MdIcon};

#[derive(Parser)]
#[command(
    name = "md_icons",
    about = "Material Design icons through schnellui's cached icon layer"
)]
struct Cli {
    #[arg(long, default_value = "md_icons.png")]
    out: String,
    #[arg(long, default_value_t = 900)]
    width: u32,
    #[arg(long, default_value_t = 620)]
    height: u32,
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    #[arg(long)]
    dump_a11y: Option<String>,
    #[arg(long)]
    windowed: bool,
}

fn size_sample(size: f32) -> impl View {
    Column::new()
        .gap(5.0)
        .child(MdIcon::outlined("home", outlined::ICON_HOME).size(size))
        .child(Text::new(format!("{size:.0}px")).size(13.0))
}

fn gallery() -> impl View {
    let blue = Color::rgb(37, 99, 235);
    let violet = Color::rgb(124, 58, 237);
    let rose = Color::rgb(225, 29, 72);
    let emerald = Color::rgb(5, 150, 105);
    let amber = Color::rgb(217, 119, 6);

    Pad::all(28.0).child(
        Column::new()
            .gap(18.0)
            .child(Text::new("Material Design icons").size(30.0))
            .child(
                Text::new(
                    "Library-neutral sources · cached SVG parsing/rasterization · shared GPU atlas",
                )
                .size(15.0),
            )
            .child(Text::new("One vector asset, five physical sizes").size(18.0))
            .child(
                Row::new()
                    .gap(28.0)
                    .child(size_sample(16.0))
                    .child(size_sample(24.0))
                    .child(size_sample(32.0))
                    .child(size_sample(48.0))
                    .child(size_sample(64.0)),
            )
            .child(Text::new("Three md-icons families, unfilled and filled").size(18.0))
            .child(
                Row::new()
                    .gap(24.0)
                    .child(
                        Column::new()
                            .gap(5.0)
                            .child(MdIcon::outlined("favorite", outlined::ICON_FAVORITE).size(40.0))
                            .child(Text::new("Outlined").size(13.0)),
                    )
                    .child(
                        Column::new()
                            .gap(5.0)
                            .child(
                                MdIcon::outlined_filled(
                                    "favorite",
                                    outlined::filled::ICON_FAVORITE,
                                )
                                .size(40.0),
                            )
                            .child(Text::new("Outlined filled").size(13.0)),
                    )
                    .child(
                        Column::new()
                            .gap(5.0)
                            .child(MdIcon::rounded("favorite", rounded::ICON_FAVORITE).size(40.0))
                            .child(Text::new("Rounded").size(13.0)),
                    )
                    .child(
                        Column::new()
                            .gap(5.0)
                            .child(
                                MdIcon::rounded_filled("favorite", rounded::filled::ICON_FAVORITE)
                                    .size(40.0),
                            )
                            .child(Text::new("Rounded filled").size(13.0)),
                    )
                    .child(
                        Column::new()
                            .gap(5.0)
                            .child(MdIcon::sharp("favorite", sharp::ICON_FAVORITE).size(40.0))
                            .child(Text::new("Sharp").size(13.0)),
                    )
                    .child(
                        Column::new()
                            .gap(5.0)
                            .child(
                                MdIcon::sharp_filled("favorite", sharp::filled::ICON_FAVORITE)
                                    .size(40.0),
                            )
                            .child(Text::new("Sharp filled").size(13.0)),
                    ),
            )
            .child(Text::new("One cached 32px raster, five draw-time tints").size(18.0))
            .child(
                Row::new()
                    .gap(30.0)
                    .child(
                        MdIcon::outlined("settings", outlined::ICON_SETTINGS)
                            .size(32.0)
                            .color(blue)
                            .alt("Blue settings"),
                    )
                    .child(
                        MdIcon::outlined("settings", outlined::ICON_SETTINGS)
                            .size(32.0)
                            .color(violet),
                    )
                    .child(
                        MdIcon::outlined("settings", outlined::ICON_SETTINGS)
                            .size(32.0)
                            .color(rose),
                    )
                    .child(
                        MdIcon::outlined("settings", outlined::ICON_SETTINGS)
                            .size(32.0)
                            .color(emerald),
                    )
                    .child(
                        MdIcon::outlined("settings", outlined::ICON_SETTINGS)
                            .size(32.0)
                            .color(amber),
                    ),
            )
            .child(
                Row::new()
                    .gap(24.0)
                    .child(MdIcon::outlined("search", outlined::ICON_SEARCH).size(28.0))
                    .child(MdIcon::outlined("menu", outlined::ICON_MENU).size(28.0))
                    .child(
                        MdIcon::outlined("visibility", outlined::ICON_VISIBILITY)
                            .size(28.0)
                            .alt("Visible"),
                    )
                    .child(MdIcon::outlined("delete", outlined::ICON_DELETE).size(28.0))
                    .child(MdIcon::outlined("check", outlined::ICON_CHECK).size(28.0)),
            ),
    )
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut app = App::mount_with_size_scaled(gallery(), cli.width, cli.height, cli.scale);

    if cli.windowed {
        return match app.run_windowed("schnellui md-icons") {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("windowed run failed: {error}");
                ExitCode::FAILURE
            }
        };
    }

    app.frame();
    if let Some(path) = &cli.dump_a11y {
        if let Err(error) = app.dump_a11y(path) {
            eprintln!("dump-a11y failed: {error}");
            return ExitCode::FAILURE;
        }
    }
    match app.render_to_png(&cli.out) {
        Ok(()) => {
            let stats = cache_stats();
            let gpu_entries = app.scene().images().cached_len();
            println!(
                "wrote {} ({} parsed icons, {} CPU rasters, {} shared GPU atlas entries)",
                cli.out, stats.parsed_documents, stats.rasterized_images, gpu_entries
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("render failed: {error}");
            ExitCode::FAILURE
        }
    }
}
