//! User Interface representing the the NES Control Deck

use crate::{
    apu::SAMPLE_RATE,
    common::{Clocked, Powered},
    control_deck::ControlDeck,
    nes_err,
    ppu::{RENDER_HEIGHT, RENDER_WIDTH},
    NesResult,
};
use pix_engine::prelude::*;
use std::path::PathBuf;
use window::{Window, WindowBuilder};

mod config;
mod event;
mod filesystem;
mod window;

pub use config::NesConfig;

const APP_NAME: &str = "TetaNES";
const ICON_PATH: &str = "static/tetanes_icon.png";
const _STATIC_ICON: &[u8] = include_bytes!("../static/tetanes_icon.png");
const WINDOW_WIDTH: u32 = (RENDER_WIDTH as f32 * 8.0 / 7.0 + 0.5) as u32; // for 8:7 Aspect Ratio
const WINDOW_HEIGHT: u32 = RENDER_HEIGHT;

/* TODO:
 * turbo/clock
 * zapper decay
 * Rewind
 * ppu view / scanline
 * nt view / scanline
 * debug view / active
 * cpu break points
 * emulation speed
 * recording / playback (based on cli path)
 * messages
 * clear_savestate
 * savable
 * config file:
 *   - speed 0.1 - 4.0
 *   - savestates_off
 *   - rewind_off
 *   - concurrent_dpad
 *   - vsync_off
 *   - sound_off
 *   - no_pause_in_bg
 *   - debug
 *   - genie_codes
 *   - save_slot
 *   - record
 */

#[derive(Debug, Clone)]
pub struct NesBuilder {
    path: PathBuf,
    fullscreen: bool,
    scale: f32,
}

impl NesBuilder {
    /// Creates a new NesBuilder instance.
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
            fullscreen: false,
            scale: 3.0,
        }
    }

    /// The initial ROM or path to search ROMs for.
    pub fn path<'a>(&'a mut self, path: PathBuf) -> &'a mut Self {
        self.path = path;
        self
    }

    /// Enables fullscreen mode.
    pub fn fullscreen<'a>(&'a mut self, val: bool) -> &'a mut Self {
        self.fullscreen = val;
        self
    }

    /// Set the window scale.
    pub fn scale<'a>(&'a mut self, val: f32) -> &'a mut Self {
        self.scale = val;
        self
    }

    /// TODO: build documentation
    pub fn build(&self) -> Nes {
        let control_deck = ControlDeck::new();
        let mut config = NesConfig::new();
        config.path = self.path.to_owned();
        config.scale = self.scale;
        config.fullscreen = self.fullscreen;
        Nes {
            roms: Vec::new(),
            control_deck,
            windows: Vec::new(),
            config,
        }
    }
}

/// Represents an NES Emulation.
#[derive(Debug, Clone)]
pub struct Nes {
    roms: Vec<PathBuf>,
    control_deck: ControlDeck,
    windows: Vec<Window>,
    config: NesConfig,
}

impl Nes {
    /// Begins emulation by starting the game engine loop
    pub fn run(&mut self) -> NesResult<()> {
        let filename = self.config.path.file_name().and_then(|f| f.to_str());
        let title = if let Some(filename) = filename {
            format!("{} - {}", APP_NAME, filename.replace(".nes", ""))
        } else {
            APP_NAME.to_owned()
        };

        let width = (self.config.scale * WINDOW_WIDTH as f32) as u32;
        let height = (self.config.scale * WINDOW_HEIGHT as f32) as u32;
        let mut engine = PixEngine::create(width, height);
        engine
            .with_title(title)
            .with_frame_rate()
            .audio_sample_rate(SAMPLE_RATE.floor() as i32)
            .icon(ICON_PATH)
            .resizable();

        if self.config.fullscreen {
            engine.fullscreen();
        }
        if self.config.vsync {
            engine.vsync_enabled();
        }

        Ok(engine.build()?.run(self)?)
    }

    /// Update rendering textures with emulation state
    fn render_frame(&mut self, s: &mut PixState) -> PixResult<()> {
        self.windows[0].update_texture(s, self.control_deck.get_frame())?;
        Ok(())
    }
}

impl AppState for Nes {
    fn on_start(&mut self, s: &mut PixState) -> PixResult<()> {
        let main_window = WindowBuilder::new(s.width(), s.height())
            .with_id(s.window_id())
            .create_texture(PixelFormat::Rgb, RENDER_WIDTH, RENDER_HEIGHT)
            .clip((0, 8, RENDER_WIDTH, RENDER_HEIGHT - 8))
            .build(s)?;
        self.windows.push(main_window);
        self.find_roms()?;
        if self.roms.len() == 1 {
            self.load_rom(0)?;
            self.control_deck.power_on();
        }
        Ok(())
    }

    fn on_update(&mut self, s: &mut PixState) -> PixResult<()> {
        self.control_deck.clock();
        self.render_frame(s)?;
        if self.config.sound_enabled {
            s.enqueue_audio(&self.control_deck.get_audio_samples());
        }
        self.control_deck.clear_audio_samples();
        Ok(())
    }

    fn on_stop(&mut self, _s: &mut PixState) -> PixResult<()> {
        self.control_deck.power_off();
        Ok(())
    }

    fn on_key_pressed(&mut self, s: &mut PixState, key: Key, repeat: bool) -> PixResult<()> {
        self.handle_key_pressed(s, key, repeat)
    }

    fn on_key_released(&mut self, s: &mut PixState, key: Key, _repeat: bool) -> PixResult<()> {
        self.handle_key_released(s, key)
    }

    // TODO: controller
    // fn on_controller_down() {}
    // fn on_controller_release() {}
    // fn on_controller_axis_motion() {}
}

impl Default for Nes {
    fn default() -> Self {
        Self {
            roms: Vec::new(),
            windows: Vec::new(),
            ..Default::default()
        }
    }
}
