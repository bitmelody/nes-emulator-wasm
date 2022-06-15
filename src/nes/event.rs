use crate::{
    apu::AudioChannel,
    common::{NesRegion, Powered},
    cpu::instr::Operation,
    input::{GamepadBtn, GamepadSlot},
    mapper::MapperRevision,
    nes::{menu::Menu, Mode, Nes, NesResult, ReplayMode, NES_FRAME_SRC},
    ppu::{VideoFilter, RENDER_HEIGHT},
};
use pix_engine::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::HashMap,
    fmt,
    ops::{ControlFlow, Deref, DerefMut},
    time::{Duration, Instant},
};

/// Indicates an [Axis] direction.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[must_use]
pub(crate) enum AxisDirection {
    /// No direction, axis is in a deadzone/not pressed.
    None,
    /// Positive (Right or Down)
    Positive,
    /// Negative (Left or Up)
    Negative,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[must_use]
pub(crate) struct ActionEvent {
    pub(crate) frame: u32,
    pub(crate) slot: GamepadSlot,
    pub(crate) action: Action,
    pub(crate) pressed: bool,
    pub(crate) repeat: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[must_use]
pub(crate) enum Input {
    Key((GamepadSlot, Key, KeyMod)),
    Button((GamepadSlot, ControllerButton)),
    Axis((GamepadSlot, Axis, AxisDirection)),
    Mouse((GamepadSlot, Mouse)),
}

impl fmt::Display for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Input::Key((_, key, keymod)) => {
                if keymod.is_empty() {
                    write!(f, "{:?}", key)
                } else {
                    write!(f, "{:?} {:?}", keymod, key)
                }
            }
            Input::Button((_, btn)) => write!(f, "{:?}", btn),
            Input::Axis((_, axis, _)) => write!(f, "{:?}", axis),
            Input::Mouse((_, btn)) => write!(f, "{:?}", btn),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct KeyBinding {
    pub(crate) player: GamepadSlot,
    pub(crate) key: Key,
    pub(crate) keymod: KeyMod,
    pub(crate) action: Action,
}

impl KeyBinding {
    pub(crate) const fn new(player: GamepadSlot, key: Key, keymod: KeyMod, action: Action) -> Self {
        Self {
            player,
            key,
            keymod,
            action,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct MouseBinding {
    pub(crate) player: GamepadSlot,
    pub(crate) button: Mouse,
    pub(crate) action: Action,
}

impl MouseBinding {
    pub(crate) const fn new(player: GamepadSlot, button: Mouse, action: Action) -> Self {
        Self {
            player,
            button,
            action,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct ControllerButtonBinding {
    pub(crate) player: GamepadSlot,
    pub(crate) button: ControllerButton,
    pub(crate) action: Action,
}

impl ControllerButtonBinding {
    pub(crate) const fn new(player: GamepadSlot, button: ControllerButton, action: Action) -> Self {
        Self {
            player,
            button,
            action,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct ControllerAxisBinding {
    pub(crate) player: GamepadSlot,
    pub(crate) axis: Axis,
    pub(crate) direction: AxisDirection,
    pub(crate) action: Action,
}

impl ControllerAxisBinding {
    pub(crate) const fn new(
        player: GamepadSlot,
        axis: Axis,
        direction: AxisDirection,
        action: Action,
    ) -> Self {
        Self {
            player,
            axis,
            direction,
            action,
        }
    }
}

/// A binding of a inputs to an [Action].
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct InputBindings {
    pub(crate) keys: Vec<KeyBinding>,
    pub(crate) mouse: Vec<MouseBinding>,
    pub(crate) buttons: Vec<ControllerButtonBinding>,
    pub(crate) axes: Vec<ControllerAxisBinding>,
}

impl InputBindings {
    pub(crate) fn update_from_map(&mut self, input_map: &InputMapping) {
        self.keys.clear();
        self.mouse.clear();
        self.buttons.clear();
        self.axes.clear();
        for (&input, &action) in input_map.iter() {
            match input {
                Input::Key((slot, key, keymod)) => {
                    self.keys.push(KeyBinding::new(slot, key, keymod, action));
                }
                Input::Mouse((slot, button)) => {
                    self.mouse.push(MouseBinding::new(slot, button, action));
                }
                Input::Button((slot, button)) => self
                    .buttons
                    .push(ControllerButtonBinding::new(slot, button, action)),
                Input::Axis((slot, axis, direction)) => self
                    .axes
                    .push(ControllerAxisBinding::new(slot, axis, direction, action)),
            }
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputMapping(HashMap<Input, Action>);

impl Deref for InputMapping {
    type Target = HashMap<Input, Action>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for InputMapping {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[allow(variant_size_differences)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Action {
    Nes(NesState),
    Menu(Menu),
    Feature(Feature),
    Setting(Setting),
    Gamepad(GamepadBtn),
    ZapperTrigger,
    ZeroAxis([GamepadBtn; 2]),
    Debug(DebugAction),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum NesState {
    ToggleMenu,
    Quit,
    TogglePause,
    Reset,
    PowerCycle,
    MapperRevision(MapperRevision),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Feature {
    ToggleGameplayRecording,
    ToggleSoundRecording,
    Rewind,
    TakeScreenshot,
    SaveState,
    LoadState,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Setting {
    SetSaveSlot(u8),
    ToggleFullscreen,
    ToggleVsync,
    ToggleNtscFilter,
    SetVideoFilter(VideoFilter),
    SetNesFormat(NesRegion),
    ToggleSound,
    TogglePulse1,
    TogglePulse2,
    ToggleTriangle,
    ToggleNoise,
    ToggleDmc,
    FastForward,
    IncSpeed,
    DecSpeed,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum DebugAction {
    ToggleCpuDebugger,
    TogglePpuDebugger,
    ToggleApuDebugger,
    StepInto,
    StepOver,
    StepOut,
    StepFrame,
    StepScanline,
    IncScanline,
    DecScanline,
}

fn render_message(s: &mut PixState, message: &str, color: Color) -> NesResult<()> {
    s.push();
    s.stroke(None);
    s.fill(rgb!(0, 200));
    let pady = s.theme().spacing.frame_pad.y();
    s.rect([
        0,
        s.cursor_pos().y() - pady,
        s.width()? as i32,
        s.theme().font_size as i32 + 2 * pady,
    ])?;
    s.fill(color);
    s.text(message)?;
    s.pop();
    Ok(())
}

impl Nes {
    #[inline]
    pub(crate) fn add_message<S>(&mut self, text: S)
    where
        S: Into<String>,
    {
        let text = text.into();
        self.messages.push((text, Instant::now()));
    }

    #[inline]
    pub(crate) fn render_messages(&mut self, s: &mut PixState) -> NesResult<()> {
        self.messages
            .retain(|(_, created)| created.elapsed() < Duration::from_secs(3));
        self.messages.dedup_by(|a, b| a.0.eq(&b.0));
        for (message, _) in &self.messages {
            render_message(s, message, Color::WHITE)?;
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn render_status(&mut self, s: &mut PixState, status: &str) -> PixResult<()> {
        render_message(s, status, Color::WHITE)?;
        if let Some(ref err) = self.error {
            render_message(s, err, Color::RED)?;
        }
        Ok(())
    }

    #[inline]
    pub(crate) fn handle_input(
        &mut self,
        s: &mut PixState,
        slot: GamepadSlot,
        input: Input,
        pressed: bool,
        repeat: bool,
    ) -> NesResult<bool> {
        self.config
            .input_map
            .get(&input)
            .copied()
            .map_or(Ok(false), |action| {
                if pressed && self.replay.mode == ReplayMode::Playback {
                    match action {
                        Action::Feature(Feature::ToggleGameplayRecording) => self.stop_replay(),
                        Action::Nes(state) => self.handle_nes_state(s, state)?,
                        Action::Menu(menu) => self.open_menu(s, menu)?,
                        _ => return Ok(false),
                    }
                    Ok(true)
                } else {
                    self.handle_action(s, slot, action, pressed, repeat)
                }
            })
    }

    #[inline]
    pub(crate) fn handle_key_event(
        &mut self,
        s: &mut PixState,
        event: KeyEvent,
        pressed: bool,
    ) -> bool {
        for slot in [
            GamepadSlot::One,
            GamepadSlot::Two,
            GamepadSlot::Three,
            GamepadSlot::Four,
        ] {
            let input = Input::Key((slot, event.key, event.keymod));
            if let Ok(true) = self.handle_input(s, slot, input, pressed, event.repeat) {
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn handle_mouse_click(&mut self, s: &mut PixState, btn: Mouse) -> bool {
        // To avoid consuming events while in menus
        if self.mode == Mode::Playing {
            for slot in [GamepadSlot::One, GamepadSlot::Two] {
                let input = Input::Mouse((slot, btn));
                if let Ok(true) = self.handle_input(s, slot, input, true, false) {
                    return true;
                }
            }
        }
        false
    }

    #[inline]
    fn handle_zapper_trigger(&mut self, slot: GamepadSlot) {
        if self.control_deck.zapper_connected(slot) {
            self.control_deck.trigger_zapper(slot);
        }
    }

    #[inline]
    pub fn set_zapper_pos(&mut self, pos: Point<i32>) -> bool {
        for slot in [GamepadSlot::One, GamepadSlot::Two] {
            if self.control_deck.zapper_connected(slot) {
                let mut pos = pos / self.config.scale as i32;
                pos.set_x((pos.x() as f32 * 8.0 / 7.0 + 0.5) as i32); // Adjust ratio
                if pos.y() < NES_FRAME_SRC.top() {
                    pos.set_y(8);
                } else if pos.y() >= NES_FRAME_SRC.bottom() {
                    pos.set_y(NES_FRAME_SRC.bottom() - 1);
                }
                self.control_deck.aim_zapper(slot, pos.x(), pos.y());
                return true;
            }
        }
        false
    }

    #[inline]
    pub fn handle_mouse_motion(&mut self, pos: Point<i32>) -> bool {
        // To avoid consuming events while in menus
        if self.mode == Mode::Playing {
            self.set_zapper_pos(pos)
        } else {
            false
        }
    }

    #[inline]
    pub(crate) fn handle_controller_event(
        &mut self,
        s: &mut PixState,
        event: ControllerEvent,
        pressed: bool,
    ) -> PixResult<bool> {
        self.get_controller_slot(event.controller_id)
            .map_or(Ok(false), |slot| {
                let input = Input::Button((slot, event.button));
                self.handle_input(s, slot, input, pressed, false)
            })
    }

    #[inline]
    pub(crate) fn handle_controller_axis(
        &mut self,
        s: &mut PixState,
        controller_id: ControllerId,
        axis: Axis,
        value: i32,
    ) -> PixResult<bool> {
        self.get_controller_slot(controller_id)
            .map_or(Ok(false), |slot| {
                let direction = match value.cmp(&0) {
                    Ordering::Greater => AxisDirection::Positive,
                    Ordering::Less => AxisDirection::Negative,
                    Ordering::Equal => AxisDirection::None,
                };
                let input = Input::Axis((slot, axis, direction));
                self.handle_input(s, slot, input, true, false)
            })
    }

    #[inline]
    pub(crate) fn handle_action(
        &mut self,
        s: &mut PixState,
        slot: GamepadSlot,
        action: Action,
        pressed: bool,
        repeat: bool,
    ) -> PixResult<bool> {
        if !repeat {
            log::debug!(
                "Input: {{ action: {:?}, slot: {:?}, pressed: {} }}",
                action,
                slot,
                pressed
            );
        }

        if repeat && pressed {
            match action {
                Action::Debug(action) => self.handle_debug(s, action, repeat)?,
                Action::Feature(Feature::Rewind) => {
                    if self.config.rewind {
                        self.mode = Mode::Rewinding;
                    } else {
                        self.add_message("Rewind disabled. You can enable it in the Config menu.");
                    }
                }
                Action::Setting(Setting::FastForward) => {
                    self.set_speed(2.0);
                }
                _ => return Ok(false),
            }
        } else if !pressed {
            match action {
                Action::Setting(Setting::FastForward) => {
                    self.set_speed(1.0);
                }
                Action::Feature(Feature::Rewind) => {
                    if self.mode == Mode::Rewinding {
                        self.resume_play();
                    } else {
                        self.instant_rewind();
                    }
                }
                _ => return Ok(false),
            }
        } else {
            match action {
                Action::Debug(action) if pressed => self.handle_debug(s, action, repeat)?,
                Action::Feature(feature) if pressed => self.handle_feature(s, feature),
                Action::Nes(state) if pressed => self.handle_nes_state(s, state)?,
                Action::Menu(menu) if pressed => self.open_menu(s, menu)?,
                Action::Setting(setting) if pressed => self.handle_setting(s, setting)?,
                Action::Gamepad(button) => self.handle_gamepad_pressed(slot, button, pressed),
                Action::ZapperTrigger => self.handle_zapper_trigger(slot),
                Action::ZeroAxis(buttons) => {
                    for button in buttons {
                        self.handle_gamepad_pressed(slot, button, pressed);
                    }
                }
                _ => return Ok(false),
            }
        }

        if self.replay.mode == ReplayMode::Recording
            && !matches!(
                action,
                Action::Feature(Feature::ToggleGameplayRecording)
                    | Action::Nes(NesState::TogglePause | NesState::ToggleMenu),
            )
        {
            self.replay
                .buffer
                .push(self.action_event(slot, action, pressed, repeat));
        }

        Ok(true)
    }

    pub(crate) fn replay_action(&mut self, s: &mut PixState) -> NesResult<()> {
        let current_frame = self.control_deck.frame_number();
        while let Some(action_event) = self.replay.buffer.last() {
            match action_event.frame.cmp(&current_frame) {
                Ordering::Equal => {
                    let ActionEvent {
                        slot,
                        action,
                        pressed,
                        repeat,
                        ..
                    } = self.replay.buffer.pop().expect("valid action event");
                    self.handle_action(s, slot, action, pressed, repeat)?;
                }
                Ordering::Less => {
                    log::warn!(
                        "Encountered action event out of order: {} < {}",
                        action_event.frame,
                        current_frame
                    );
                    self.replay.buffer.pop();
                }
                Ordering::Greater => break,
            }
        }
        if self.replay.buffer.is_empty() {
            self.stop_replay();
        }
        Ok(())
    }
}

impl Nes {
    #[inline]
    const fn action_event(
        &self,
        slot: GamepadSlot,
        action: Action,
        pressed: bool,
        repeat: bool,
    ) -> ActionEvent {
        ActionEvent {
            frame: self.control_deck.frame_number(),
            slot,
            action,
            pressed,
            repeat,
        }
    }

    #[inline]
    fn get_controller_slot(&self, controller_id: ControllerId) -> Option<GamepadSlot> {
        self.players.iter().find_map(|(&slot, &id)| {
            if id == controller_id {
                Some(slot)
            } else {
                None
            }
        })
    }

    #[inline]
    fn handle_nes_state(&mut self, s: &mut PixState, state: NesState) -> NesResult<()> {
        if self.replay.mode == ReplayMode::Recording {
            return Ok(());
        }
        match state {
            NesState::ToggleMenu => self.toggle_menu(Menu::Config, s)?,
            NesState::Quit => {
                self.pause_play();
                s.quit();
            }
            NesState::TogglePause => self.toggle_pause(s)?,
            NesState::Reset => {
                self.error = None;
                self.control_deck.reset();
                self.add_message("Reset");
                if self.debugger.is_some() && self.mode != Mode::Paused {
                    self.mode = Mode::Paused;
                }
            }
            NesState::PowerCycle => {
                self.error = None;
                self.control_deck.power_cycle();
                self.add_message("Power Cycled");
                if self.debugger.is_some() {
                    self.mode = Mode::Paused;
                }
            }
            NesState::MapperRevision(_) => todo!("mapper revision"),
        }
        Ok(())
    }

    #[inline]
    fn handle_feature(&mut self, s: &mut PixState, feature: Feature) {
        match feature {
            Feature::ToggleGameplayRecording => match self.replay.mode {
                ReplayMode::Off => self.start_replay(),
                ReplayMode::Recording | ReplayMode::Playback => self.stop_replay(),
            },
            Feature::ToggleSoundRecording => self.toggle_sound_recording(s),
            Feature::TakeScreenshot => self.save_screenshot(s),
            Feature::SaveState => self.save_state(self.config.save_slot),
            Feature::LoadState => self.load_state(self.config.save_slot),
            Feature::Rewind => (), // Rewinds on key release instead
        }
    }

    #[inline]
    fn handle_setting(&mut self, s: &mut PixState, setting: Setting) -> NesResult<()> {
        match setting {
            Setting::SetSaveSlot(slot) => {
                self.config.save_slot = slot;
                self.add_message(&format!("Set Save Slot to {}", slot));
            }
            Setting::ToggleFullscreen => {
                self.config.fullscreen = !self.config.fullscreen;
                s.fullscreen(self.config.fullscreen)?;
            }
            Setting::ToggleVsync => {
                self.config.vsync = !self.config.vsync;
                s.vsync(self.config.vsync)?;
                if self.config.vsync {
                    self.add_message("Vsync Enabled");
                } else {
                    self.add_message("Vsync Disabled");
                }
            }
            Setting::ToggleNtscFilter => {
                self.config.filter = match self.config.filter {
                    VideoFilter::Pixellate => VideoFilter::Ntsc,
                    VideoFilter::Ntsc => VideoFilter::Pixellate,
                };
                self.control_deck.set_filter(self.config.filter);
            }
            Setting::ToggleSound => {
                self.config.sound = !self.config.sound;
                if self.config.sound {
                    self.add_message("Sound Enabled");
                } else {
                    self.add_message("Sound Disabled");
                }
            }
            Setting::TogglePulse1 => self.control_deck.toggle_channel(AudioChannel::Pulse1),
            Setting::TogglePulse2 => self.control_deck.toggle_channel(AudioChannel::Pulse2),
            Setting::ToggleTriangle => self.control_deck.toggle_channel(AudioChannel::Triangle),
            Setting::ToggleNoise => self.control_deck.toggle_channel(AudioChannel::Noise),
            Setting::ToggleDmc => self.control_deck.toggle_channel(AudioChannel::Dmc),
            Setting::IncSpeed => self.change_speed(0.25),
            Setting::DecSpeed => self.change_speed(-0.25),
            Setting::FastForward => (), // Toggling fast forward happens on key release
            _ => (),
        }
        Ok(())
    }

    #[inline]
    fn handle_gamepad_pressed(&mut self, slot: GamepadSlot, button: GamepadBtn, pressed: bool) {
        let mut gamepad = self.control_deck.gamepad_mut(slot);
        if !self.config.concurrent_dpad && pressed {
            match button {
                GamepadBtn::Left => gamepad.right = !pressed,
                GamepadBtn::Right => gamepad.left = !pressed,
                GamepadBtn::Up => gamepad.down = !pressed,
                GamepadBtn::Down => gamepad.up = !pressed,
                _ => (),
            }
        }
        match button {
            GamepadBtn::Left => gamepad.left = pressed,
            GamepadBtn::Right => gamepad.right = pressed,
            GamepadBtn::Up => gamepad.up = pressed,
            GamepadBtn::Down => gamepad.down = pressed,
            GamepadBtn::A => gamepad.a = pressed,
            GamepadBtn::B => gamepad.b = pressed,
            GamepadBtn::TurboA => {
                gamepad.turbo_a = pressed;
                gamepad.a = pressed; // Ensures that primary button isn't stuck pressed
            }
            GamepadBtn::TurboB => {
                gamepad.turbo_b = pressed;
                gamepad.b = pressed; // Ensures that primary button isn't stuck pressed
            }
            GamepadBtn::Select => gamepad.select = pressed,
            GamepadBtn::Start => gamepad.start = pressed,
        };
    }

    fn handle_debug(
        &mut self,
        s: &mut PixState,
        action: DebugAction,
        repeat: bool,
    ) -> NesResult<()> {
        let debugging = self.debugger.is_some();
        match action {
            DebugAction::ToggleCpuDebugger if !repeat => self.toggle_debugger(s)?,
            DebugAction::TogglePpuDebugger if !repeat => self.toggle_ppu_viewer(s)?,
            DebugAction::ToggleApuDebugger if !repeat => self.toggle_apu_viewer(s)?,
            DebugAction::StepInto if debugging => self.debug_step_into(s)?,
            DebugAction::StepOver if debugging => self.debug_step_over(s)?,
            DebugAction::StepOut if debugging => self.debug_step_out(s)?,
            DebugAction::StepFrame if debugging => self.debug_step_frame(s)?,
            DebugAction::StepScanline if debugging => self.debug_step_scanline(s)?,
            DebugAction::IncScanline if self.ppu_viewer.is_some() => {
                let increment = if s.keymod_down(KeyMod::SHIFT) { 10 } else { 1 };
                self.scanline = (self.scanline + increment).clamp(0, RENDER_HEIGHT - 1);
                self.control_deck
                    .ppu_mut()
                    .set_viewer_scanline(self.scanline);
            }
            DebugAction::DecScanline if self.ppu_viewer.is_some() => {
                let decrement = if s.keymod_down(KeyMod::SHIFT) { 10 } else { 1 };
                self.scanline = self.scanline.saturating_sub(decrement);
                self.control_deck
                    .ppu_mut()
                    .set_viewer_scanline(self.scanline);
            }
            _ => (),
        }
        Ok(())
    }

    fn debug_step_into(&mut self, s: &mut PixState) -> NesResult<()> {
        self.pause_play();
        match self.control_deck.clock_debug() {
            Ok(control) => self.handle_debugger(control),
            Err(err) => self.handle_emulation_error(s, &err)?,
        }
        Ok(())
    }

    fn debug_step_over(&mut self, s: &mut PixState) -> NesResult<()> {
        self.pause_play();
        let instr = self.control_deck.next_instr();
        match self.control_deck.clock_debug() {
            Ok(control) => self.handle_debugger(control),
            Err(err) => self.handle_emulation_error(s, &err)?,
        }
        if instr.op() == Operation::JSR {
            let rti_addr = self.control_deck.stack_addr().wrapping_add(1);
            while self.control_deck.pc() != rti_addr {
                match self.control_deck.clock_debug() {
                    Ok(control) => {
                        self.handle_debugger(control);
                        if let ControlFlow::Break(_) = control {
                            break;
                        }
                    }
                    Err(err) => {
                        self.handle_emulation_error(s, &err)?;
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn debug_step_out(&mut self, s: &mut PixState) -> NesResult<()> {
        let mut instr = self.control_deck.next_instr();
        while !matches!(instr.op(), Operation::RTS | Operation::RTI) {
            match self.control_deck.clock_debug() {
                Ok(control) => {
                    self.handle_debugger(control);
                    if let ControlFlow::Break(_) = control {
                        break;
                    }
                }
                Err(err) => {
                    self.handle_emulation_error(s, &err)?;
                    break;
                }
            }
            instr = self.control_deck.next_instr();
        }
        match self.control_deck.clock_debug() {
            Ok(control) => self.handle_debugger(control),
            Err(err) => self.handle_emulation_error(s, &err)?,
        }

        Ok(())
    }

    fn debug_step_frame(&mut self, s: &mut PixState) -> NesResult<()> {
        self.pause_play();
        match self.control_deck.clock_frame() {
            Ok(control) => self.handle_debugger(control),
            Err(err) => self.handle_emulation_error(s, &err)?,
        }
        Ok(())
    }

    fn debug_step_scanline(&mut self, s: &mut PixState) -> NesResult<()> {
        self.pause_play();
        match self.control_deck.clock_scanline() {
            Ok(control) => self.handle_debugger(control),
            Err(err) => self.handle_emulation_error(s, &err)?,
        }
        Ok(())
    }
}
