//! wh6006-gui — desktop front end for the Ecowitt WH6006 weather logger.

mod app;
mod worker;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Desktop app for the Ecowitt WH6006 weather logger")]
struct Args {
    /// hidraw node to use (default: auto-detect USB 04d8:003c)
    #[arg(long)]
    device: Option<PathBuf>,
    /// Hex-dump every packet to stderr
    #[arg(long)]
    debug: bool,
}

fn main() -> eframe::Result {
    let args = Args::parse();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("WH6006 Weather Logger")
            .with_app_id("wh6006-gui")
            .with_inner_size([980.0, 680.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "wh6006-gui",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, args.device, args.debug)))),
    )
}
