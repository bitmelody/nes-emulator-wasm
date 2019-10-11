//! Usage: rustynes [rom_file | rom_directory]
//!
//! 1. If a rom file is provided, that rom is loaded
//! 2. If a directory is provided, `.nes` files are searched for in that directory
//! 3. If no arguments are provided, the current directory is searched for rom files ending in
//!    `.nes`
//!
//! In the case of 2 and 3, if valid NES rom files are found, a menu screen is displayed to select
//! which rom to run. If there are any errors related to invalid files, directories, or
//! permissions, the program will print an error and exit.

use rustynes::ui::{Ui, UiSettings};
use std::{env, path::PathBuf};
use structopt::StructOpt;

fn main() {
    let opt = Opt::from_args();
    let settings = UiSettings {
        path: opt
            .path
            .unwrap_or_else(|| env::current_dir().unwrap_or_default()),
        debug: opt.debug,
        fullscreen: opt.fullscreen,
        vsync: !opt.vsync_off,
        sound_enabled: !opt.sound_off,
        save_enabled: !opt.no_save,
        concurrent_dpad: opt.concurrent_dpad,
        randomize_ram: opt.randomize_ram,
        save_slot: opt.save_slot,
        scale: opt.scale,
        speed: opt.speed,
        genie_codes: opt.genie_codes,
    };
    let ui = Ui::with_settings(settings);
    if let Err(e) = ui.run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

/// Command-Line Options
#[derive(StructOpt, Debug)]
#[structopt(
    name = "rustynes",
    about = "An NES emulator written in Rust.",
    version = "0.1.0",
    author = "Luke Petherbridge <me@lukeworks.tech>"
)]
struct Opt {
    #[structopt(
        parse(from_os_str),
        help = "The NES ROM to load or a directory containing `.nes` ROM files. [default: current directory]"
    )]
    path: Option<PathBuf>,
    #[structopt(
        short = "d",
        long = "debug",
        help = "Start with debugger enabled and emulation paused at first CPU instruction."
    )]
    debug: bool,
    #[structopt(short = "f", long = "fullscreen", help = "Fullscreen")]
    fullscreen: bool,
    #[structopt(short = "v", long = "vsync_off", help = "Disable Vsync")]
    vsync_off: bool,
    #[structopt(long = "sound_off", help = "Disable Sound")]
    sound_off: bool,
    #[structopt(
        long = "concurrent_dpad",
        help = "Enables the ability to simulate concurrent L+R and U+D on the D-Pad"
    )]
    concurrent_dpad: bool,
    #[structopt(
        long = "randomize_ram",
        help = "By default RAM initializes to 0x00 on power up. This affects some games RNG seed generators."
    )]
    randomize_ram: bool,
    #[structopt(long = "no_save", help = "Disable savestates")]
    no_save: bool,
    #[structopt(
        long = "save_slot",
        default_value = "1",
        help = "Use Save Slot # (Options: 1-4)"
    )]
    save_slot: u8,
    #[structopt(
        short = "s",
        long = "scale",
        default_value = "3",
        help = "Window scale"
    )]
    scale: u32,
    #[structopt(long = "speed", default_value = "1.0", help = "Emulation speed")]
    speed: f64,
    #[structopt(long = "genie_codes", help = "List of Game Genie Codes")]
    genie_codes: Vec<String>,
}
