use crate::{
    common::{create_png, Clocked, Powered},
    nes_err,
    serialization::Savable,
    ui::{settings::DEFAULT_SPEED, Message, Ui, REWIND_TIMER},
    NesResult,
};
use chrono::prelude::{DateTime, Local};
use pix_engine::{
    event::{Axis, Button, Key, Mouse, PixEvent},
    StateData,
};
use std::{
    fs,
    io::{BufWriter, Read, Write},
    path::PathBuf,
};

const GAMEPAD_TRIGGER_PRESS: i16 = 32_700;
const GAMEPAD_AXIS_DEADZONE: i16 = 10_000;

impl Ui {
    fn rewind(&mut self) {
        if self.settings.rewind_enabled {
            // If we saved too recently, ignore it and go back further
            if self.rewind_timer > 3.0 {
                let _ = self.rewind_queue.pop_back();
            }
            if let Some(slot) = self.rewind_queue.pop_back() {
                self.rewind_timer = REWIND_TIMER;
                self.messages
                    .push(Message::new(&format!("Rewind Slot {}", slot)));
                self.rewind_save = slot + 1;
                self.load_state(slot);
            }
        }
    }

    pub(super) fn poll_events(&mut self, data: &mut StateData) -> NesResult<()> {
        let turbo = self.turbo_clock < 3;
        self.clock_turbo(turbo);
        let events = if self.playback && self.record_frame < self.record_buffer.len() {
            if let Some(events) = self.record_buffer.get(self.record_frame) {
                events.to_vec()
            } else {
                self.playback = false;
                data.poll()
            }
        } else {
            data.poll()
        };
        if self.recording && !self.playback {
            self.record_buffer.push(Vec::new());
        }
        for event in events {
            match event {
                PixEvent::WinClose(window_id) => match Some(window_id) {
                    i if i == self.ppu_viewer_window => self.toggle_ppu_viewer(data)?,
                    i if i == self.nt_viewer_window => self.toggle_nt_viewer(data)?,
                    _ => (),
                },
                PixEvent::Focus(window_id, focus) => {
                    self.focused_window = if focus { window_id } else { 0 };

                    // Pausing only applies to the main window
                    if self.focused_window == 1 {
                        // Only unpause if we weren't paused as a result of losing focus
                        if focus && self.lost_focus {
                            self.paused(false);
                        } else if !focus && !self.paused {
                            // Only pause and set lost_focus if we weren't already paused
                            self.lost_focus = true;
                            self.paused(true);
                        }
                    }
                }
                PixEvent::KeyPress(..) => self.handle_key_event(event, turbo, data)?,
                PixEvent::GamepadBtn(which, btn, pressed) => match btn {
                    Button::Guide if pressed => self.paused(!self.paused),
                    Button::LeftShoulder if pressed => self.change_speed(-0.25),
                    Button::RightShoulder if pressed => self.change_speed(0.25),
                    _ => {
                        if self.recording && !self.playback {
                            self.record_buffer[self.record_frame].push(event);
                        }
                        self.handle_gamepad_button(which, btn, pressed, turbo)?;
                    }
                },
                PixEvent::GamepadAxis(which, axis, value) => {
                    self.handle_gamepad_axis(which, axis, value)?
                }
                _ => (),
            }
        }
        self.record_frame += 1;
        Ok(())
    }

    fn clock_turbo(&mut self, turbo: bool) {
        let mut input = &mut self.cpu.bus.input;
        if input.gamepad1.turbo_a {
            input.gamepad1.a = turbo;
        }
        if input.gamepad1.turbo_b {
            input.gamepad1.b = turbo;
        }
        if input.gamepad2.turbo_a {
            input.gamepad2.a = turbo;
        }
        if input.gamepad2.turbo_b {
            input.gamepad2.b = turbo;
        }
    }

    fn handle_key_event(
        &mut self,
        event: PixEvent,
        turbo: bool,
        data: &mut StateData,
    ) -> NesResult<()> {
        if self.recording && !self.playback {
            if let PixEvent::KeyPress(key, ..) = event {
                match key {
                    Key::A
                    | Key::S
                    | Key::Z
                    | Key::X
                    | Key::Return
                    | Key::RShift
                    | Key::Left
                    | Key::Right
                    | Key::Up
                    | Key::Down => {
                        self.record_buffer[self.record_frame].push(event);
                    }
                    _ => (),
                }
            }
        }
        match event {
            PixEvent::KeyPress(key, true, true) => self.handle_keyrepeat(key),
            PixEvent::KeyPress(key, true, false) => self.handle_keydown(key, turbo, data)?,
            PixEvent::KeyPress(key, false, ..) => self.handle_keyup(key, turbo),
            _ => (),
        }
        Ok(())
    }

    fn handle_keyrepeat(&mut self, key: Key) {
        let d = self.debug;
        match key {
            // No modifiers
            Key::C if d => {
                let _ = self.clock();
            }
            Key::F if d => self.clock_frame(),
            Key::S if d => {
                let prev_scanline = self.cpu.bus.ppu.scanline;
                let mut scanline = prev_scanline;
                while scanline == prev_scanline {
                    let _ = self.clock();
                    scanline = self.cpu.bus.ppu.scanline;
                }
            }
            // Nametable/PPU Viewer Shortcuts
            Key::Up => {
                if Some(self.focused_window) == self.nt_viewer_window {
                    self.set_nt_scanline(self.nt_scanline.saturating_sub(1));
                } else {
                    self.set_pat_scanline(self.pat_scanline.saturating_sub(1));
                }
            }
            Key::Down => {
                if Some(self.focused_window) == self.nt_viewer_window {
                    self.set_nt_scanline(self.nt_scanline + 1);
                } else {
                    self.set_pat_scanline(self.pat_scanline + 1);
                }
            }
            _ => (),
        }
    }

    #[allow(clippy::cognitive_complexity)]
    fn handle_keydown(&mut self, key: Key, turbo: bool, data: &mut StateData) -> NesResult<()> {
        let c = self.ctrl;
        let s = self.shift;
        let d = self.debug;
        match key {
            // No modifiers
            Key::Ctrl => self.ctrl = true,
            Key::LShift => self.shift = true,
            Key::Escape => self.paused(!self.paused),
            Key::Space => self.change_speed(1.0),
            Key::Comma => self.rewind(),
            Key::C if d => {
                let _ = self.clock();
            }
            Key::D if d && !c => self.active_debug = !self.active_debug,
            Key::F if d => self.clock_frame(),
            Key::S if d => {
                let prev_scanline = self.cpu.bus.ppu.scanline;
                let mut scanline = prev_scanline;
                while scanline == prev_scanline {
                    let _ = self.clock();
                    scanline = self.cpu.bus.ppu.scanline;
                }
            }
            // Ctrl
            Key::Num1 if c => self.settings.save_slot = 1,
            Key::Num2 if c => self.settings.save_slot = 2,
            Key::Num3 if c => self.settings.save_slot = 3,
            Key::Num4 if c => self.settings.save_slot = 4,
            Key::Minus if c => self.change_speed(-0.25),
            Key::Equals if c => self.change_speed(0.25),
            Key::Return if c => {
                self.settings.fullscreen = !self.settings.fullscreen;
                data.fullscreen(self.settings.fullscreen)?;
            }
            Key::C if c => {
                self.menu = !self.menu;
                self.paused(self.menu);
            }
            Key::D if c => self.toggle_debug(data)?,
            Key::S if c => self.save_state(self.settings.save_slot),
            Key::L if c => self.load_state(self.settings.save_slot),
            Key::M if c => {
                if self.settings.unlock_fps {
                    self.add_message("Sound disabled while FPS unlocked");
                } else {
                    self.settings.sound_enabled = !self.settings.sound_enabled;
                    if self.settings.sound_enabled {
                        self.add_message("Sound Enabled");
                    } else {
                        self.add_message("Sound Disabled");
                    }
                }
            }
            Key::N if c => self.cpu.bus.ppu.ntsc_video = !self.cpu.bus.ppu.ntsc_video,
            Key::O if c => self.add_message("Open Dialog not implemented"), // TODO
            Key::R if c => {
                self.paused(false);
                self.reset();
                self.add_message("Reset");
            }
            Key::P if c && !s => {
                self.paused(false);
                self.power_cycle();
                self.add_message("Power Cycled");
            }
            Key::V if c => {
                self.settings.vsync = !self.settings.vsync;
                data.vsync(self.settings.vsync)?;
                if self.settings.vsync {
                    self.add_message("Vsync Enabled");
                } else {
                    self.add_message("Vsync Disabled");
                }
            }
            // Shift
            Key::N if s => self.toggle_nt_viewer(data)?,
            Key::P if s => self.toggle_ppu_viewer(data)?,
            Key::V if s => {
                self.recording = !self.recording;
                if self.recording {
                    self.add_message("Recording Started");
                } else {
                    self.add_message("Recording Stopped");
                    self.save_recording()?;
                }
            }
            // F# Keys
            Key::F10 => match screenshot(&self.frame()) {
                Ok(s) => self.add_message(&s),
                Err(e) => self.add_message(&e.to_string()),
            },
            _ => {
                if Some(self.focused_window) == self.nt_viewer_window {
                    match key {
                        Key::Up => self.set_nt_scanline(self.nt_scanline.saturating_sub(1)),
                        Key::Down => self.set_nt_scanline(self.nt_scanline + 1),
                        _ => (),
                    }
                } else if Some(self.focused_window) == self.ppu_viewer_window {
                    match key {
                        Key::Up => self.set_pat_scanline(self.pat_scanline.saturating_sub(1)),
                        Key::Down => self.set_pat_scanline(self.pat_scanline + 1),
                        _ => (),
                    }
                } else {
                    self.handle_input_event(key, true, turbo);
                }
            }
        }
        Ok(())
    }

    fn handle_keyup(&mut self, key: Key, turbo: bool) {
        match key {
            Key::Ctrl => self.ctrl = false,
            Key::LShift => self.shift = false,
            Key::Space => {
                self.settings.speed = DEFAULT_SPEED;
                self.cpu.bus.apu.set_speed(self.settings.speed);
            }
            _ => self.handle_input_event(key, false, turbo),
        }
    }

    fn handle_input_event(&mut self, key: Key, pressed: bool, turbo: bool) {
        if self.focused_window != 1 {
            return;
        }

        let mut input = &mut self.cpu.bus.input;
        match key {
            // Gamepad
            Key::Z => input.gamepad1.a = pressed,
            Key::X => input.gamepad1.b = pressed,
            Key::A => {
                input.gamepad1.turbo_a = pressed;
                input.gamepad1.a = turbo && pressed;
            }
            Key::S => {
                input.gamepad1.turbo_b = pressed;
                input.gamepad1.b = turbo && pressed;
            }
            Key::RShift => input.gamepad1.select = pressed,
            Key::Return => input.gamepad1.start = pressed,
            Key::Up => {
                if !self.settings.concurrent_dpad && pressed {
                    input.gamepad1.down = false;
                }
                input.gamepad1.up = pressed;
            }
            Key::Down => {
                if !self.settings.concurrent_dpad && pressed {
                    input.gamepad1.up = false;
                }
                input.gamepad1.down = pressed;
            }
            Key::Left => {
                if !self.settings.concurrent_dpad && pressed {
                    input.gamepad1.right = false;
                }
                input.gamepad1.left = pressed;
            }
            Key::Right => {
                if !self.settings.concurrent_dpad && pressed {
                    input.gamepad1.left = false;
                }
                input.gamepad1.right = pressed;
            }
            _ => (),
        }
    }

    fn handle_gamepad_button(
        &mut self,
        gamepad_id: i32,
        button: Button,
        pressed: bool,
        turbo: bool,
    ) -> NesResult<()> {
        if self.focused_window != 1 {
            return Ok(());
        }

        let input = &mut self.cpu.bus.input;
        let mut gamepad = match gamepad_id {
            0 => &mut input.gamepad1,
            1 => &mut input.gamepad2,
            _ => panic!("invalid gamepad id: {}", gamepad_id),
        };
        match button {
            Button::A => {
                gamepad.a = pressed;
            }
            Button::B => gamepad.b = pressed,
            Button::X => {
                gamepad.turbo_a = pressed;
                gamepad.a = turbo && pressed;
            }
            Button::Y => {
                gamepad.turbo_b = pressed;
                gamepad.b = turbo && pressed;
            }
            Button::Back => gamepad.select = pressed,
            Button::Start => gamepad.start = pressed,
            Button::DPadUp => gamepad.up = pressed,
            Button::DPadDown => gamepad.down = pressed,
            Button::DPadLeft => gamepad.left = pressed,
            Button::DPadRight => gamepad.right = pressed,
            _ => {}
        }
        Ok(())
    }
    fn handle_gamepad_axis(&mut self, gamepad_id: i32, axis: Axis, value: i16) -> NesResult<()> {
        if self.focused_window != 1 {
            return Ok(());
        }

        let input = &mut self.cpu.bus.input;
        let mut gamepad = match gamepad_id {
            0 => &mut input.gamepad1,
            1 => &mut input.gamepad2,
            _ => panic!("invalid gamepad id: {}", gamepad_id),
        };
        match axis {
            // Left/Right
            Axis::LeftX => {
                if value < -GAMEPAD_AXIS_DEADZONE {
                    gamepad.left = true;
                } else if value > GAMEPAD_AXIS_DEADZONE {
                    gamepad.right = true;
                } else {
                    gamepad.left = false;
                    gamepad.right = false;
                }
            }
            // Down/Up
            Axis::LeftY => {
                if value < -GAMEPAD_AXIS_DEADZONE {
                    gamepad.up = true;
                } else if value > GAMEPAD_AXIS_DEADZONE {
                    gamepad.down = true;
                } else {
                    gamepad.up = false;
                    gamepad.down = false;
                }
            }
            Axis::TriggerLeft if value > GAMEPAD_TRIGGER_PRESS => {
                self.save_state(self.settings.save_slot)
            }
            Axis::TriggerRight if value > GAMEPAD_TRIGGER_PRESS => {
                self.load_state(self.settings.save_slot)
            }
            _ => (),
        }
        Ok(())
    }

    pub fn save_recording(&mut self) -> NesResult<()> {
        let datetime: DateTime<Local> = Local::now();
        let mut path = PathBuf::from(
            datetime
                .format("Recording_%Y-%m-%d_at_%H.%M.%S")
                .to_string(),
        );
        path.set_extension("dat");
        let file = fs::File::create(&path)?;
        let mut file = BufWriter::new(file);
        self.record_buffer.save(&mut file)?;
        Ok(())
    }
}

impl Savable for PixEvent {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        match *self {
            PixEvent::GamepadBtn(id, button, pressed) => {
                0u8.save(fh)?;
                id.save(fh)?;
                button.save(fh)?;
                pressed.save(fh)?;
            }
            PixEvent::GamepadAxis(id, axis, value) => {
                1u8.save(fh)?;
                id.save(fh)?;
                axis.save(fh)?;
                value.save(fh)?;
            }
            PixEvent::KeyPress(key, pressed, repeat) => {
                2u8.save(fh)?;
                key.save(fh)?;
                pressed.save(fh)?;
                repeat.save(fh)?;
            }
            _ => (),
        }
        Ok(())
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => {
                let mut id: i32 = 0;
                let mut btn = Button::default();
                let mut pressed = false;
                id.load(fh)?;
                btn.load(fh)?;
                pressed.load(fh)?;
                PixEvent::GamepadBtn(id, btn, pressed)
            }
            1 => {
                let mut id: i32 = 0;
                let mut axis = Axis::default();
                let mut value = 0;
                id.load(fh)?;
                axis.load(fh)?;
                value.load(fh)?;
                PixEvent::GamepadAxis(id, axis, value)
            }
            2 => {
                let mut key = Key::default();
                let mut pressed = false;
                let mut repeat = false;
                key.load(fh)?;
                pressed.load(fh)?;
                repeat.load(fh)?;
                PixEvent::KeyPress(key, pressed, repeat)
            }
            _ => return nes_err!("invalid PixEvent value"),
        };
        Ok(())
    }
}

impl Savable for Button {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => Button::A,
            1 => Button::B,
            2 => Button::X,
            3 => Button::Y,
            4 => Button::Back,
            5 => Button::Start,
            6 => Button::Guide,
            7 => Button::DPadUp,
            8 => Button::DPadDown,
            9 => Button::DPadLeft,
            10 => Button::DPadRight,
            11 => Button::LeftStick,
            12 => Button::RightStick,
            13 => Button::LeftShoulder,
            14 => Button::RightShoulder,
            _ => nes_err!("invalid Button value")?,
        };
        Ok(())
    }
}

impl Savable for Axis {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => Axis::LeftX,
            1 => Axis::RightX,
            2 => Axis::LeftY,
            3 => Axis::RightY,
            4 => Axis::TriggerLeft,
            5 => Axis::TriggerRight,
            _ => nes_err!("invalid Axis value")?,
        };
        Ok(())
    }
}

impl Savable for Key {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        let val: u8 = match *self {
            Key::A => 0, // Turbo A
            Key::S => 1, // Turbo B
            Key::X => 2, // A
            Key::Z => 3, // B
            Key::Left => 4,
            Key::Up => 5,
            Key::Down => 6,
            Key::Right => 7,
            Key::Return => 8, // Start
            Key::RShift => 9, // Select
            _ => return Ok(()),
        };
        val.save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => Key::A, // Turbo A
            1 => Key::S, // Turbo B
            2 => Key::X, // A
            3 => Key::Z, // B
            4 => Key::Left,
            5 => Key::Up,
            6 => Key::Down,
            7 => Key::Right,
            8 => Key::Return, // Start
            9 => Key::RShift, // Select
            _ => nes_err!("invalid Key value")?,
        };
        Ok(())
    }
}

impl Savable for Mouse {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => Mouse::Left,
            1 => Mouse::Middle,
            2 => Mouse::Right,
            3 => Mouse::X1,
            4 => Mouse::X2,
            5 => Mouse::Unknown,
            _ => nes_err!("invalid Mouse value")?,
        };
        Ok(())
    }
}

/// Takes a screenshot and saves it to the current directory as a `.png` file
///
/// # Arguments
///
/// * `pixels` - An array of pixel data to save in `.png` format
///
/// # Errors
///
/// It's possible for this method to fail, but instead of erroring the program,
/// it'll simply log the error out to STDERR
/// TODO Move this into UI and have it use width/height
pub fn screenshot(pixels: &[u8]) -> NesResult<String> {
    let datetime: DateTime<Local> = Local::now();
    let mut png_path = PathBuf::from(
        datetime
            .format("Screen_Shot_%Y-%m-%d_at_%H.%M.%S")
            .to_string(),
    );
    png_path.set_extension("png");
    create_png(&png_path, pixels)
}
