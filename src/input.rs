use crate::{
    common::{Clock, Kind, NesRegion, Reset},
    cpu::Cpu,
    ppu::Ppu,
};
use bitflags::bitflags;
use serde::{Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[must_use]
pub enum Slot {
    One,
    Two,
    Three,
    Four,
}

impl Default for Slot {
    fn default() -> Self {
        Self::One
    }
}

pub trait InputRegisters {
    fn read(&mut self, slot: Slot, ppu: &Ppu) -> u8;
    fn peek(&self, slot: Slot, ppu: &Ppu) -> u8;
    fn write(&mut self, val: u8);
}

#[derive(Default, Debug, Copy, Clone, Serialize, Deserialize)]
#[must_use]
pub struct Input {
    joypads: [Joypad; 4],
    signatures: [Joypad; 2],
    zappers: [Zapper; 2],
    turbo_timer: u8,
    fourscore: bool,
}

impl Input {
    pub const fn new() -> Self {
        Self {
            joypads: [Joypad::new(); 4],
            // Signature bits are reversed so they can shift right
            signatures: [
                Joypad::signature(0b0000_1000),
                Joypad::signature(0b0000_0100),
            ],
            zappers: [Zapper::new(); 2],
            turbo_timer: 30,
            fourscore: false,
        }
    }

    #[inline]
    pub const fn joypad(&self, slot: Slot) -> &Joypad {
        &self.joypads[slot as usize]
    }

    #[inline]
    pub fn joypad_mut(&mut self, slot: Slot) -> &mut Joypad {
        &mut self.joypads[slot as usize]
    }

    #[inline]
    pub const fn zapper(&self, slot: Slot) -> &Zapper {
        &self.zappers[slot as usize]
    }

    #[inline]
    pub fn zapper_mut(&mut self, slot: Slot) -> &mut Zapper {
        &mut self.zappers[slot as usize]
    }

    #[inline]
    #[must_use]
    pub const fn fourscore(&self) -> bool {
        self.fourscore
    }

    #[inline]
    pub fn set_fourscore(&mut self, enabled: bool) {
        self.fourscore = enabled;
    }
}

impl Input {
    fn read_slots(&mut self, a: usize, b: usize, ppu: &Ppu) -> u8 {
        if self.zappers[a].connected {
            self.zappers[a].read(ppu)
        } else {
            // Read $4016/$4017 D0 8x for controller #1/#2.
            // Read $4016/$4017 D0 8x for controller #3/#4.
            // Read $4016/$4017 D0 8x for signature: 0b00010000/0b00100000
            if self.joypads[a].index() < 8 {
                self.joypads[a].read()
            } else if self.fourscore {
                if self.joypads[b].index() < 8 {
                    self.joypads[b].read()
                } else if self.signatures[a].index() < 8 {
                    self.signatures[a].read()
                } else {
                    0x01
                }
            } else {
                0x01
            }
        }
    }

    fn peek_slots(&self, a: usize, b: usize, ppu: &Ppu) -> u8 {
        if self.zappers[a].connected {
            self.zappers[a].read(ppu)
        } else {
            // Read $4016/$4017 D0 8x for controller #1/#2.
            // Read $4016/$4017 D0 8x for controller #3/#4.
            // Read $4016/$4017 D0 8x for signature: 0b00010000/0b00100000
            if self.joypads[a].index() < 8 {
                self.joypads[a].peek()
            } else if self.fourscore {
                if self.joypads[b].index() < 8 {
                    self.joypads[b].peek()
                } else if self.signatures[a].index() < 8 {
                    self.signatures[a].peek()
                } else {
                    0x01
                }
            } else {
                0x01
            }
        }
    }
}

impl InputRegisters for Input {
    fn read(&mut self, slot: Slot, ppu: &Ppu) -> u8 {
        match slot {
            Slot::One => self.read_slots(0, 2, ppu) | 0x40,
            Slot::Two => self.read_slots(1, 3, ppu) | 0x40,
            _ => panic!("invalid input slot for read"),
        }
    }

    fn peek(&self, slot: Slot, ppu: &Ppu) -> u8 {
        match slot {
            Slot::One => self.peek_slots(0, 2, ppu) | 0x40,
            Slot::Two => self.peek_slots(1, 3, ppu) | 0x40,
            _ => panic!("invalid input slot for peek"),
        }
    }

    fn write(&mut self, val: u8) {
        for pad in &mut self.joypads {
            pad.write(val);
        }
        for sig in &mut self.signatures {
            sig.write(val);
        }
    }
}

impl Clock for Input {
    fn clock(&mut self) -> usize {
        for zapper in &mut self.zappers {
            zapper.clock();
        }
        self.turbo_timer -= 1;
        if self.turbo_timer == 0 {
            self.turbo_timer += 30;
            for pad in &mut self.joypads {
                if pad.button(JoypadBtnState::TURBO_A) {
                    let pressed = pad.button(JoypadBtnState::A);
                    pad.set_button(JoypadBtnState::A, !pressed);
                }
                if pad.button(JoypadBtnState::TURBO_B) {
                    let pressed = pad.button(JoypadBtnState::B);
                    pad.set_button(JoypadBtnState::B, !pressed);
                }
            }
        }
        1
    }
}

impl Reset for Input {
    fn reset(&mut self, kind: Kind) {
        for pad in &mut self.joypads {
            pad.reset(kind);
        }
        for sig in &mut self.signatures {
            sig.reset(kind);
        }
        for zapper in &mut self.zappers {
            zapper.reset(kind);
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum JoypadBtn {
    /// Left D-Pad.
    Left,
    /// Right D-Pad.
    Right,
    /// Up D-Pad.
    Up,
    /// Down D-Pad.
    Down,
    /// A Button.
    A,
    /// B Button.
    B,
    /// A Button (Turbo).
    TurboA,
    /// B Button (Turbo).
    TurboB,
    /// Select Button.
    Select,
    /// Start Button.
    Start,
}

impl AsRef<str> for JoypadBtn {
    fn as_ref(&self) -> &str {
        match *self {
            JoypadBtn::A => "A",
            JoypadBtn::B => "B",
            JoypadBtn::Select => "Select",
            JoypadBtn::Start => "Start",
            JoypadBtn::Up => "Up",
            JoypadBtn::Down => "Down",
            JoypadBtn::Left => "Left",
            JoypadBtn::Right => "Right",
            JoypadBtn::TurboA => "A (Turbo)",
            JoypadBtn::TurboB => "B (Turbo)",
        }
    }
}

bitflags! {
    #[derive(Default, Serialize, Deserialize)]
    #[must_use]
    pub struct JoypadBtnState: u16 {
        const A = 0x01;
        const B = 0x02;
        const SELECT = 0x04;
        const START = 0x08;
        const UP = 0x10;
        const DOWN = 0x20;
        const LEFT = 0x40;
        const RIGHT = 0x80;
        const TURBO_A = 0x100;
        const TURBO_B = 0x200;
        const DPAD = Self::UP.bits | Self::DOWN.bits | Self::LEFT.bits | Self::RIGHT.bits;
    }
}

impl From<JoypadBtn> for JoypadBtnState {
    fn from(button: JoypadBtn) -> Self {
        match button {
            JoypadBtn::A => Self::A,
            JoypadBtn::B => Self::B,
            JoypadBtn::Select => Self::SELECT,
            JoypadBtn::Start => Self::START,
            JoypadBtn::Up => Self::UP,
            JoypadBtn::Down => Self::DOWN,
            JoypadBtn::Left => Self::LEFT,
            JoypadBtn::Right => Self::RIGHT,
            JoypadBtn::TurboA => Self::TURBO_A,
            JoypadBtn::TurboB => Self::TURBO_B,
        }
    }
}

#[derive(Default, Debug, Copy, Clone, Serialize, Deserialize)]
#[must_use]
pub struct Joypad {
    buttons: JoypadBtnState,
    index: u8,
    strobe: bool,
}

impl Joypad {
    pub const fn new() -> Self {
        Self {
            buttons: JoypadBtnState::from_bits_truncate(0),
            index: 0,
            strobe: false,
        }
    }

    #[inline]
    #[must_use]
    pub const fn button(&self, button: JoypadBtnState) -> bool {
        self.buttons.contains(button)
    }

    #[inline]
    pub fn set_button(&mut self, button: JoypadBtnState, pressed: bool) {
        self.buttons.set(button, pressed);
    }

    pub const fn signature(val: u16) -> Self {
        Self {
            buttons: JoypadBtnState::from_bits_truncate(val),
            index: 0,
            strobe: false,
        }
    }

    #[must_use]
    pub fn read(&mut self) -> u8 {
        let val = self.peek();
        if !self.strobe && self.index <= 7 {
            self.index += 1;
        }
        val
    }

    #[must_use]
    pub const fn peek(&self) -> u8 {
        if self.index > 7 {
            0x01
        } else {
            ((self.buttons.bits as u8) & (1 << self.index)) >> self.index
        }
    }

    pub fn write(&mut self, val: u8) {
        let prev_strobe = self.strobe;
        self.strobe = val & 0x01 == 0x01;
        if prev_strobe && !self.strobe {
            self.index = 0;
        }
    }

    #[inline]
    #[must_use]
    pub const fn index(&self) -> u8 {
        self.index
    }
}

impl Reset for Joypad {
    fn reset(&mut self, _kind: Kind) {
        self.buttons.bits = 0;
        self.index = 0;
        self.strobe = false;
    }
}

#[derive(Default, Debug, Copy, Clone, Serialize, Deserialize)]
#[must_use]
pub struct Zapper {
    pub triggered: f32,
    pub x: i32,
    pub y: i32,
    pub radius: i32,
    pub connected: bool,
}

impl Zapper {
    #[inline]
    #[must_use]
    pub const fn x(&self) -> i32 {
        self.x
    }

    #[inline]
    #[must_use]
    pub const fn y(&self) -> i32 {
        self.y
    }

    #[inline]
    #[must_use]
    pub const fn connected(&self) -> bool {
        self.connected
    }

    #[inline]
    pub fn set_connected(&mut self, connected: bool) {
        self.connected = connected;
    }

    #[inline]
    pub fn trigger(&mut self) {
        if self.triggered <= 0.0 {
            // Zapper takes ~100ms to change to "released" after trigger is pulled
            self.triggered = Cpu::region_clock_rate(NesRegion::default()) / 10.0;
        }
    }

    #[inline]
    pub fn aim(&mut self, x: i32, y: i32) {
        self.x = x;
        self.y = y;
    }
}

impl Zapper {
    const fn new() -> Self {
        Self {
            triggered: 0.0,
            x: 0,
            y: 0,
            radius: 3,
            connected: false,
        }
    }

    #[inline]
    #[must_use]
    fn read(&self, ppu: &Ppu) -> u8 {
        self.triggered() | self.light_sense(ppu) | 0x40
    }

    #[inline]
    fn triggered(&self) -> u8 {
        if self.triggered > 0.0 {
            0x10
        } else {
            0x00
        }
    }

    fn light_sense(&self, ppu: &Ppu) -> u8 {
        let width = Ppu::WIDTH as i32;
        let height = Ppu::HEIGHT as i32;
        let scanline = ppu.scanline() as i32;
        let cycle = ppu.cycle() as i32;
        let x = self.x;
        let y = self.y;
        if x >= 0 && y >= 0 {
            for y in (y - self.radius)..=(y + self.radius) {
                if y >= 0 && y < height {
                    for x in (x - self.radius)..=(x + self.radius) {
                        let in_bounds = x >= 0 && x < width;
                        let behind_ppu =
                            scanline >= y && (scanline - y) <= 20 && (scanline != y || cycle > x);
                        if in_bounds && behind_ppu && ppu.pixel_brightness(x as u32, y as u32) >= 85
                        {
                            return 0x00;
                        }
                    }
                }
            }
        }
        0x08
    }
}

impl Clock for Zapper {
    fn clock(&mut self) -> usize {
        if self.triggered > 0.0 {
            self.triggered -= 1.0;
            1
        } else {
            0
        }
    }
}

impl Reset for Zapper {
    fn reset(&mut self, _kind: Kind) {
        self.triggered = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use crate::test_roms;

    test_roms!(
        "test_roms/input",
        #[ignore = "todo"]
        zapper_flip,
        #[ignore = "todo"]
        zapper_light,
        #[ignore = "todo"]
        zapper_stream,
        #[ignore = "todo"]
        zapper_trigger,
    );
}
