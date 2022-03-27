//! User Interface representing the the NES Control Deck

use crate::{
    apu::SAMPLE_RATE,
    common::{config_dir, config_path, Powered},
    control_deck::ControlDeck,
    input::GamepadSlot,
    memory::RamState,
    nes::debug::Debugger,
    ppu::{RENDER_HEIGHT, RENDER_PITCH, RENDER_WIDTH},
    NesResult,
};
use anyhow::Context;
use config::Config;
use filesystem::{is_nes_rom, is_playback_file};
use menu::{Menu, Player};
use pix_engine::prelude::*;
use std::{
    collections::{hash_map::Entry, HashMap},
    env,
    ffi::OsStr,
    fmt::Write,
    fs,
    ops::ControlFlow,
    path::PathBuf,
    time::{Duration, Instant},
};

pub(crate) mod config;
pub(crate) mod debug;
pub(crate) mod event;
pub(crate) mod filesystem;
pub(crate) mod menu;
pub(crate) mod state;

pub(crate) const SETTINGS: &str = "settings.json";
const DEFAULT_SETTINGS: &[u8] = include_bytes!("../config/settings.json");

const APP_NAME: &str = "TetaNES";
#[cfg(not(target_arch = "wasm32"))]
const ICON: &[u8] = include_bytes!("../static/tetanes_icon.png");
const WINDOW_WIDTH: f32 = RENDER_WIDTH as f32 * 8.0 / 7.0 + 0.5; // for 8:7 Aspect Ratio
const WINDOW_HEIGHT: f32 = RENDER_HEIGHT as f32;
// Trim top and bottom 8 scanlines
const NES_FRAME_SRC: Rect<i32> = rect![0, 8, RENDER_WIDTH as i32, RENDER_HEIGHT as i32 - 16];

#[derive(Debug, Clone)]
#[must_use]
pub struct NesBuilder {
    path: PathBuf,
    fullscreen: bool,
    power_state: RamState,
    scale: f32,
    speed: f32,
    genie_codes: Vec<String>,
    debug: bool,
}

impl NesBuilder {
    /// Creates a new `NesBuilder` instance.
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
            fullscreen: false,
            power_state: RamState::Random,
            scale: 3.0,
            speed: 1.0,
            genie_codes: vec![],
            debug: false,
        }
    }

    /// The initial ROM or path to search ROMs for.
    pub fn path<P>(&mut self, path: Option<P>) -> &mut Self
    where
        P: Into<PathBuf>,
    {
        self.path = path.map_or_else(|| env::current_dir().unwrap_or_default(), Into::into);
        self
    }

    /// Enables fullscreen mode.
    pub fn fullscreen(&mut self, val: bool) -> &mut Self {
        self.fullscreen = val;
        self
    }

    /// Sets the default power-on state for RAM values.
    pub fn power_state(&mut self, state: RamState) -> &mut Self {
        self.power_state = state;
        self
    }

    /// Set the window scale.
    pub fn scale(&mut self, val: f32) -> &mut Self {
        self.scale = val;
        self
    }

    /// Set the emulation speed.
    pub fn speed(&mut self, val: f32) -> &mut Self {
        self.speed = val;
        self
    }

    /// Set the game genie codes to use on startup.
    pub fn genie_codes(&mut self, codes: Vec<String>) -> &mut Self {
        self.genie_codes = codes;
        self
    }

    pub fn debug(&mut self, debug: bool) -> &mut Self {
        self.debug = debug;
        self
    }

    /// Creates an Nes instance from an `NesBuilder`.
    ///
    /// # Errors
    ///
    /// If the default configuration directories and files can't be created, an error is returned.
    pub fn build(&self) -> NesResult<Nes> {
        let config_dir = config_dir();
        if !config_dir.exists() {
            fs::create_dir_all(config_dir).context("unable to create config directory")?;
        }

        let settings = config_path(SETTINGS);
        if !settings.exists() {
            fs::write(&settings, DEFAULT_SETTINGS)
                .context("unable to create default `settings.json`")?;
        }
        let mut config = Config::from_file(settings)?;
        config.rom_path = self.path.clone().canonicalize()?;
        config.fullscreen = self.fullscreen;
        config.power_state = self.power_state;
        config.scale = self.scale;
        config.speed = self.speed;
        config.genie_codes = self.genie_codes.clone();
        let mut control_deck = ControlDeck::new(config.power_state);
        control_deck.set_speed(config.speed);
        Ok(Nes::new(control_deck, config, self.debug))
    }
}

impl Default for NesBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents which mode the emulator is in.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub(crate) enum Mode {
    Playing,
    Paused,
    PausedBg,
    InMenu(Menu, Player),
    Recording,
    Replaying,
}

impl Default for Mode {
    fn default() -> Self {
        Self::InMenu(Menu::LoadRom, Player::One)
    }
}

/// A NES window view.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct View {
    window_id: WindowId,
    texture_id: Option<TextureId>,
}

impl View {
    pub(crate) const fn new(window_id: WindowId, texture_id: Option<TextureId>) -> Self {
        Self {
            window_id,
            texture_id,
        }
    }
}

/// Represents all the NES Emulation state.
#[derive(Debug)]
pub struct Nes {
    control_deck: ControlDeck,
    players: HashMap<GamepadSlot, ControllerId>,
    emulation: Option<View>,
    debugger: Option<Debugger>,
    ppu_viewer: Option<View>,
    apu_viewer: Option<View>,
    config: Config,
    mode: Mode,
    debug: bool,
    rewinding: bool,
    scanline: u16,
    speed_counter: f32,
    messages: Vec<(String, Instant)>,
    paths: Vec<PathBuf>,
    selected_path: usize,
    error: Option<String>,
}

impl Nes {
    pub(crate) fn new(control_deck: ControlDeck, config: Config, debug: bool) -> Self {
        Self {
            control_deck,
            players: HashMap::new(),
            emulation: None,
            debugger: None,
            ppu_viewer: None,
            apu_viewer: None,
            config,
            mode: if debug { Mode::Paused } else { Mode::default() },
            debug,
            rewinding: false,
            scanline: 0,
            speed_counter: 0.0,
            messages: vec![],
            paths: vec![],
            selected_path: 0,
            error: None,
        }
    }

    /// Begins emulation by starting the game engine loop.
    ///
    /// # Errors
    ///
    /// If engine fails to build or run, then an error is returned.
    pub fn run(&mut self) -> NesResult<()> {
        let mut title = APP_NAME.to_owned();
        if is_nes_rom(&self.config.rom_path) {
            if let Some(filename) = self
                .config
                .rom_path
                .file_name()
                .map(OsStr::to_string_lossy)
                .map(|f| f.replace(".nes", ""))
            {
                write!(title, " - {}", filename)?;
            }
        }

        let width = (self.config.scale * WINDOW_WIDTH) as u32;
        let height = (self.config.scale * WINDOW_HEIGHT) as u32;
        let mut engine = PixEngine::builder();
        engine
            .with_dimensions(width, height)
            .with_title(title)
            .with_frame_rate()
            .audio_sample_rate(SAMPLE_RATE.floor() as i32)
            .audio_channels(1)
            .target_frame_rate(60)
            .resizable();

        #[cfg(not(target_arch = "wasm32"))]
        {
            engine.icon(Image::from_read(ICON)?);
        }

        if self.config.fullscreen {
            engine.fullscreen();
        }
        if self.config.vsync {
            engine.vsync_enabled();
        }

        engine.build()?.run(self)
    }

    /// Update rendering textures with emulation state
    fn render_views(&mut self, s: &mut PixState) -> PixResult<()> {
        if let Some(view) = self.emulation {
            if let Some(texture_id) = view.texture_id {
                s.update_texture(texture_id, None, self.control_deck.frame(), RENDER_PITCH)?;

                let zapper = self.control_deck.zapper();
                if zapper.connected {
                    s.with_texture(texture_id, |s: &mut PixState| {
                        let [x, y] = zapper.pos.as_array();
                        s.stroke(Color::GRAY);
                        s.line([x - 8, y, x + 8, y])?;
                        s.line([x, y - 8, x, y + 8])?;
                        Ok(())
                    })?;
                }
                s.texture(texture_id, NES_FRAME_SRC, None)?;
            }
        }
        self.render_debugger(s)?;
        self.render_ppu_viewer(s)?;
        Ok(())
    }
}

impl AppState for Nes {
    fn on_start(&mut self, s: &mut PixState) -> PixResult<()> {
        self.emulation = Some(View::new(
            s.window_id(),
            Some(s.create_texture(RENDER_WIDTH, RENDER_HEIGHT, PixelFormat::Rgba)?),
        ));
        if is_nes_rom(&self.config.rom_path) {
            self.load_rom(s)?;
        } else if is_playback_file(&self.config.rom_path) {
            self.mode = Mode::Replaying;
            unimplemented!("Replay not implemented");
        }
        if self.debug {
            self.toggle_debugger(s)?;
        }
        Ok(())
    }

    fn on_update(&mut self, s: &mut PixState) -> PixResult<()> {
        if let Mode::Playing | Mode::Recording | Mode::Replaying = self.mode {
            self.speed_counter += self.config.speed;
            'run: while self.speed_counter > 0.0 {
                self.speed_counter -= 1.0;
                if let Some(ref mut debugger) = self.debugger {
                    if let ControlFlow::Break(_) =
                        self.control_deck.debug_clock_frame(&debugger.breakpoints)
                    {
                        debugger.on_breakpoint = true;
                        self.mode = Mode::Paused;
                        break 'run;
                    }
                } else {
                    self.control_deck.clock_frame();
                }
                if self.control_deck.cpu_corrupted() {
                    self.open_menu(s, Menu::LoadRom)?;
                    self.error = Some("CPU encountered invalid opcode.".into());
                    return Ok(());
                }
            }

            if self.config.sound && self.mode != Mode::Paused {
                if s.audio_size() < 2048 {
                    self.control_deck.clock_frame();
                } else if s.audio_size() > 8192 {
                    std::thread::sleep(Duration::from_millis(10));
                }
                s.enqueue_audio(self.control_deck.audio_samples())?;
            }
            self.control_deck.clear_audio_samples();
        }

        self.render_views(s)?;
        if self.debugger.is_some() && !matches!(self.mode, Mode::InMenu(..)) {
            self.render_status(s, "Debugging")?;
        }
        match self.mode {
            Mode::Paused | Mode::PausedBg => self.render_status(s, "Paused")?,
            Mode::Recording => self.render_status(s, "Recording")?,
            Mode::Replaying => self.render_status(s, "Replay")?,
            Mode::InMenu(menu, player) => self.render_menu(s, menu, player)?,
            Mode::Playing => (),
        }
        self.render_messages(s)?;
        Ok(())
    }

    fn on_stop(&mut self, _s: &mut PixState) -> PixResult<()> {
        if let Err(e) = self.save_sram() {
            log::error!("{}", e);
        }
        self.control_deck.power_off();
        Ok(())
    }

    fn on_key_pressed(&mut self, s: &mut PixState, event: KeyEvent) -> PixResult<bool> {
        // FIXME: Convert to ApuViewer window
        if event.key == Key::A && event.keymod.intersects(KeyMod::SHIFT) {
            self.control_deck.apu_info();
        }
        self.handle_key_event(s, event, true)
    }

    fn on_key_released(&mut self, s: &mut PixState, event: KeyEvent) -> PixResult<bool> {
        self.handle_key_event(s, event, false)
    }

    fn on_mouse_pressed(
        &mut self,
        s: &mut PixState,
        btn: Mouse,
        pos: Point<i32>,
    ) -> PixResult<bool> {
        self.handle_mouse_event(s, btn, pos, true)
    }

    fn on_mouse_motion(
        &mut self,
        s: &mut PixState,
        pos: Point<i32>,
        _rel_pos: Point<i32>,
    ) -> PixResult<bool> {
        self.handle_mouse_motion(s, pos)
    }

    fn on_controller_update(
        &mut self,
        _s: &mut PixState,
        controller_id: ControllerId,
        update: ControllerUpdate,
    ) -> PixResult<bool> {
        match update {
            ControllerUpdate::Added => {
                match self.players.entry(GamepadSlot::One) {
                    Entry::Vacant(v) => {
                        v.insert(controller_id);
                    }
                    Entry::Occupied(_) => {
                        self.players
                            .entry(GamepadSlot::Two)
                            .or_insert(controller_id);
                    }
                }
                Ok(true)
            }
            ControllerUpdate::Removed => {
                self.players.retain(|_, &mut id| id != controller_id);
                Ok(true)
            }
            ControllerUpdate::Remapped => Ok(false),
        }
    }

    fn on_controller_pressed(
        &mut self,
        s: &mut PixState,
        event: ControllerEvent,
    ) -> PixResult<bool> {
        self.handle_controller_event(s, event, true)
    }

    fn on_controller_released(
        &mut self,
        s: &mut PixState,
        event: ControllerEvent,
    ) -> PixResult<bool> {
        self.handle_controller_event(s, event, false)
    }

    fn on_controller_axis_motion(
        &mut self,
        s: &mut PixState,
        controller_id: ControllerId,
        axis: Axis,
        value: i32,
    ) -> PixResult<bool> {
        self.handle_controller_axis(s, controller_id, axis, value)
    }

    fn on_window_event(
        &mut self,
        s: &mut PixState,
        window_id: WindowId,
        event: WindowEvent,
    ) -> PixResult<()> {
        match event {
            WindowEvent::Close => {
                if matches!(&self.debugger, Some(debugger) if debugger.view.window_id == window_id)
                {
                    self.debugger = None;
                    self.control_deck.cpu_mut().debugging = false;
                    if self.control_deck.is_running() {
                        self.mode = Mode::Playing;
                    }
                }
                if matches!(self.ppu_viewer, Some(view) if view.window_id == window_id) {
                    self.ppu_viewer = None;
                    self.control_deck.ppu_mut().debugging = false;
                }
                if matches!(self.apu_viewer, Some(view) if view.window_id == window_id) {
                    self.apu_viewer = None;
                }
            }
            WindowEvent::Hidden | WindowEvent::FocusLost => {
                if self.mode == Mode::Playing && self.config.pause_in_bg && !s.focused() {
                    self.mode = Mode::PausedBg;
                }
            }
            WindowEvent::Restored | WindowEvent::FocusGained => {
                if self.mode == Mode::PausedBg {
                    self.mode = Mode::Playing;
                }
            }
            _ => (),
        }
        Ok(())
    }
}
