//! Audio Processing Unit
//!
//! [https://wiki.nesdev.com/w/index.php/APU]()

use crate::console::cpu::Cpu;
use crate::console::CPU_CLOCK_RATE;
use crate::filter::{Filter, HiPassFilter, LoPassFilter};
use crate::memory::Memory;
use crate::serialization::Savable;
use crate::util::Result;
use std::fmt;
use std::io::{Read, Write};

pub const SAMPLE_RATE: f64 = 96_000.0; // in Hz
pub const SAMPLE_BUFFER_SIZE: usize = 4096;

/// Audio Processing Unit
pub struct Apu {
    pub irq_pending: bool, // Set by $4017 if irq_enabled is clear or set during step 4 of Step4 mode
    irq_enabled: bool,     // Set by $4017 D6
    pub open_bus: u8,      // This open bus gets set during any write to PPU registers
    clock_rate: f64,       // Same as CPU but is affected by speed changes
    cycle: u64,            // Current APU cycle = CPU cycle / 2
    samples: Vec<f32>,     // Buffer of samples
    frame: FrameCounter,   // Clocks length, linear, sweep, and envelope units
    pulse1: Pulse,
    pulse2: Pulse,
    triangle: Triangle,
    noise: Noise,
    pub dmc: DMC,
    filters: [Box<Filter>; 3],
    pulse_table: [f32; Self::PULSE_TABLE_SIZE],
    tnd_table: [f32; Self::TND_TABLE_SIZE],
}

impl Apu {
    const PULSE_TABLE_SIZE: usize = 31;
    const TND_TABLE_SIZE: usize = 203;

    pub fn new() -> Self {
        let mut apu = Self {
            irq_pending: false,
            irq_enabled: false,
            open_bus: 0u8,
            clock_rate: CPU_CLOCK_RATE,
            cycle: 0u64,
            samples: Vec::with_capacity(SAMPLE_BUFFER_SIZE),
            frame: FrameCounter {
                step: 1u8,
                counter: 0u16,
                mode: FCMode::Step4,
            },
            pulse1: Pulse::new(PulseChannel::One),
            pulse2: Pulse::new(PulseChannel::Two),
            triangle: Triangle::new(),
            noise: Noise::new(),
            dmc: DMC::new(),
            filters: [
                Box::new(HiPassFilter::new(90.0, SAMPLE_RATE)),
                Box::new(HiPassFilter::new(440.0, SAMPLE_RATE)),
                Box::new(LoPassFilter::new(14_000.0, SAMPLE_RATE)),
            ],
            pulse_table: [0f32; Self::PULSE_TABLE_SIZE],
            tnd_table: [0f32; Self::TND_TABLE_SIZE],
        };
        apu.frame.counter = apu.next_frame_counter();
        for i in 1..Self::PULSE_TABLE_SIZE {
            apu.pulse_table[i] = 95.52 / (8_128.0 / (i as f32) + 100.0);
        }
        for i in 1..Self::TND_TABLE_SIZE {
            apu.tnd_table[i] = 163.67 / (24_329.0 / (i as f32) + 100.0);
        }
        apu
    }

    pub fn reset(&mut self) {
        self.cycle = 0;
        self.samples.clear();
        self.irq_pending = false;
        self.irq_enabled = false;
        self.frame = FrameCounter {
            step: 1u8,
            counter: 0u16,
            mode: FCMode::Step4,
        };
        self.pulse1.reset();
        self.pulse2.reset();
        self.triangle.reset();
        self.noise.reset();
        self.dmc.reset();
    }

    pub fn power_cycle(&mut self) {
        self.reset();
    }

    pub fn clock(&mut self) {
        if self.cycle % 2 == 0 {
            self.pulse1.clock();
            self.pulse2.clock();
            self.noise.clock();
            self.dmc.clock();
        }
        self.triangle.clock();
        self.clock_frame_counter();

        if self.cycle % (self.clock_rate / SAMPLE_RATE) as u64 == 0 {
            let mut sample = self.output();
            for i in 0..self.filters.len() {
                let filter = &mut self.filters[i];
                sample = filter.process(sample);
            }
            self.samples.push(sample);
        }
        self.cycle += 1;
    }

    pub fn samples(&mut self) -> &mut Vec<f32> {
        &mut self.samples
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.clock_rate = CPU_CLOCK_RATE * speed;
    }

    // Counts CPU clocks and determines when to clock quarter/half frames
    // counter is in CPU clocks to avoid APU half-frames
    fn clock_frame_counter(&mut self) {
        if self.frame.counter > 0 {
            self.frame.counter -= 1;
        } else {
            match self.frame.step {
                1 | 3 => self.clock_quarter_frame(),
                2 | 5 => {
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                }
                _ => (), // Noop
            }
            if self.irq_enabled && self.frame.mode == FCMode::Step4 && self.frame.step >= 4 {
                self.irq_pending = true;
            }

            self.frame.step += 1;
            let max_step = 6;
            if self.frame.step > max_step {
                self.frame.step = 1;
            }
            self.frame.counter = self.next_frame_counter();
        }
    }

    fn clock_quarter_frame(&mut self) {
        self.pulse1.clock_quarter_frame();
        self.pulse2.clock_quarter_frame();
        self.triangle.clock_quarter_frame();
        self.noise.clock_quarter_frame();
    }

    fn clock_half_frame(&mut self) {
        self.pulse1.clock_half_frame();
        self.pulse2.clock_half_frame();
        self.triangle.clock_half_frame();
        self.noise.clock_half_frame();
    }

    fn next_frame_counter(&self) -> u16 {
        match self.frame.mode {
            FCMode::Step4 => match self.frame.step {
                1 => 7457,
                2 => 7456,
                3 => 7458,
                4 => 7457,
                5 => 1,
                6 => 1,
                _ => panic!("shouldn't happen"),
            },
            FCMode::Step5 => match self.frame.step {
                1 => 7457,
                2 => 7456,
                3 => 7458,
                4 => 7457,
                5 => 7452,
                6 => 1,
                _ => panic!("shouldn't happen"),
            },
        }
    }

    fn output(&mut self) -> f32 {
        self.triangle.enabled = false;
        let pulse1 = self.pulse1.output();
        let pulse2 = self.pulse2.output();
        let triangle = self.triangle.output();
        let noise = self.noise.output();
        let dmc = self.dmc.output();

        let pulse_out = self.pulse_table[(pulse1 + pulse2) as usize];
        let tnd_out = self.tnd_table[(3.0 * triangle + 2.0 * noise + dmc) as usize];
        pulse_out + tnd_out
    }

    // $4015 READ
    fn read_status(&mut self) -> u8 {
        let val = self.peek_status();
        self.irq_pending = false;
        val
    }

    fn peek_status(&self) -> u8 {
        let mut status = 0;
        if self.pulse1.length.counter > 0 {
            status |= 0x01;
        }
        if self.pulse2.length.counter > 0 {
            status |= 0x02;
        }
        if self.triangle.length.counter > 0 {
            status |= 0x04;
        }
        if self.noise.length.counter > 0 {
            status |= 0x08;
        }
        if self.dmc.length > 0 {
            status |= 0x10;
        }
        if self.irq_pending {
            status |= 0x40;
        }
        if self.dmc.irq_pending {
            status |= 0x80;
        }
        status
    }

    // $4015 WRITE
    fn write_status(&mut self, val: u8) {
        self.pulse1.enabled = val & 1 == 1;
        if !self.pulse1.enabled {
            self.pulse1.length.counter = 0;
        }
        self.pulse2.enabled = (val >> 1) & 1 == 1;
        if !self.pulse2.enabled {
            self.pulse2.length.counter = 0;
        }
        self.triangle.enabled = (val >> 2) & 1 == 1;
        if !self.triangle.enabled {
            self.triangle.length.counter = 0;
        }
        self.noise.enabled = (val >> 3) & 1 == 1;
        if !self.noise.enabled {
            self.noise.length.counter = 0;
        }
        let dmc_enabled = (val >> 4) & 1 == 1;
        if dmc_enabled {
            if self.dmc.length == 0 {
                self.dmc.length = self.dmc.length_load;
                self.dmc.addr = self.dmc.addr_load;
            }
        } else {
            self.dmc.length = 0;
        }
        self.dmc.irq_pending = false;
    }

    // $4017 APU frame counter
    fn write_frame_counter(&mut self, val: u8) {
        // D7
        self.frame.mode = if (val >> 7) & 1 == 0 {
            FCMode::Step4
        } else {
            FCMode::Step5
        };
        self.frame.step = 1u8;
        self.frame.counter = self.next_frame_counter();
        if self.cycle % 2 == 0 {
            // During an APU cycle
            self.frame.counter += 3;
        } else {
            // Between APU cycles
            self.frame.counter += 4;
        }
        // If step 5 clock immediately
        if self.frame.mode == FCMode::Step5 {
            self.clock_quarter_frame();
            self.clock_half_frame();
        }
        self.irq_enabled = (val >> 6) & 1 == 0; // D6
        if !self.irq_enabled {
            self.irq_pending = false;
        }
    }
}

impl Memory for Apu {
    fn read(&mut self, addr: u16) -> u8 {
        if addr == 0x4015 {
            let val = self.read_status();
            self.open_bus = val;
            val
        } else {
            self.open_bus
        }
    }

    fn peek(&self, addr: u16) -> u8 {
        if addr == 0x4015 {
            self.peek_status()
        } else {
            self.open_bus
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.open_bus = val;
        match addr {
            0x4000 => self.pulse1.write_control(val),
            0x4001 => self.pulse1.write_sweep(val),
            0x4002 => self.pulse1.write_timer_lo(val),
            0x4003 => self.pulse1.write_timer_hi(val),
            0x4004 => self.pulse2.write_control(val),
            0x4005 => self.pulse2.write_sweep(val),
            0x4006 => self.pulse2.write_timer_lo(val),
            0x4007 => self.pulse2.write_timer_hi(val),
            0x4008 => self.triangle.write_linear_counter(val),
            0x400A => self.triangle.write_timer_lo(val),
            0x400B => self.triangle.write_timer_hi(val),
            0x400C => self.noise.write_control(val),
            0x400E => self.noise.write_timer(val),
            0x400F => self.noise.write_length(val),
            0x4010 => self.dmc.write_timer(val),
            0x4011 => self.dmc.write_output(val),
            0x4012 => self.dmc.write_addr_load(val),
            0x4013 => self.dmc.write_length(val),
            0x4015 => self.write_status(val),
            0x4017 => self.write_frame_counter(val),
            _ => (),
        }
    }
}

impl Savable for Apu {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.irq_pending.save(fh)?;
        self.irq_enabled.save(fh)?;
        self.open_bus.save(fh)?;
        self.cycle.save(fh)?;
        self.samples.save(fh)?;
        self.frame.save(fh)?;
        self.pulse1.save(fh)?;
        self.pulse2.save(fh)?;
        self.triangle.save(fh)?;
        self.noise.save(fh)?;
        self.dmc.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.irq_pending.load(fh)?;
        self.irq_enabled.load(fh)?;
        self.open_bus.load(fh)?;
        self.cycle.load(fh)?;
        self.samples.load(fh)?;
        self.frame.load(fh)?;
        self.pulse1.load(fh)?;
        self.pulse2.load(fh)?;
        self.triangle.load(fh)?;
        self.noise.load(fh)?;
        self.dmc.load(fh)
    }
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Apu {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), fmt::Error> {
        write!(f, "APU {{ cyc: {} }}", self.cycle)
    }
}

/// Frame Counter for the APU
///
/// [https://wiki.nesdev.com/w/index.php/APU_Frame_Counter]()
struct FrameCounter {
    step: u8,     // The current step # of the 4-Step or 5-Step sequence
    counter: u16, // Counts CPU clocks until next step in the sequence
    mode: FCMode, // Either 4-Step sequence or 5-Step sequence
}

impl Savable for FrameCounter {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.step.save(fh)?;
        self.counter.save(fh)?;
        self.mode.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.step.load(fh)?;
        self.counter.load(fh)?;
        self.mode.load(fh)
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum FCMode {
    Step4,
    Step5,
}

impl Savable for FCMode {
    fn save(&self, fh: &mut Write) -> Result<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => FCMode::Step4,
            1 => FCMode::Step5,
            _ => panic!("invalid FCMode value"),
        };
        Ok(())
    }
}

struct Pulse {
    enabled: bool,
    duty_cycle: u8,        // Select row in DUTY_TABLE
    duty_counter: u8,      // Select column in DUTY_TABLE
    freq_timer: u16,       // timer freq_counter reload value
    freq_counter: u16,     // Current frequency timer value
    channel: PulseChannel, // One or Two
    length: LengthCounter,
    envelope: Envelope,
    sweep: Sweep,
}

impl Pulse {
    const DUTY_TABLE: [[u8; 8]; 4] = [
        [0, 1, 0, 0, 0, 0, 0, 0],
        [0, 1, 1, 0, 0, 0, 0, 0],
        [0, 1, 1, 1, 1, 0, 0, 0],
        [1, 0, 0, 1, 1, 1, 1, 1],
    ];

    fn new(channel: PulseChannel) -> Self {
        Self {
            enabled: false,
            duty_cycle: 0u8,
            duty_counter: 0u8,
            freq_timer: 0u16,
            freq_counter: 0u16,
            channel,
            length: LengthCounter::new(),
            envelope: Envelope::new(),
            sweep: Sweep {
                enabled: false,
                reload: false,
                negate: false,
                timer: 0u8,
                counter: 0u8,
                shift: 0u8,
            },
        }
    }

    fn reset(&mut self) {
        *self = Self::new(self.channel);
    }

    fn clock(&mut self) {
        if self.freq_counter > 0 {
            self.freq_counter -= 1;
        } else {
            self.freq_counter = self.freq_timer;
            self.duty_counter = (self.duty_counter + 1) % 8;
        }
    }
    fn clock_quarter_frame(&mut self) {
        self.envelope.clock();
    }
    fn clock_half_frame(&mut self) {
        let sweep_forcing_silence = self.sweep_forcing_silence();
        let mut swp = &mut self.sweep;
        if swp.reload {
            swp.counter = swp.timer;
            swp.reload = false;
        } else if swp.counter > 0 {
            swp.counter -= 1;
        } else {
            swp.counter = swp.timer;
            if swp.enabled && !sweep_forcing_silence {
                let delta = self.freq_timer >> swp.shift;
                if swp.negate {
                    self.freq_timer -= delta + 1;
                    if self.channel == PulseChannel::One {
                        self.freq_timer += 1;
                    }
                } else {
                    self.freq_timer += delta
                }
            }
        }

        self.length.clock();
    }

    fn sweep_forcing_silence(&self) -> bool {
        let next_freq = self.freq_timer + (self.freq_timer >> self.sweep.shift);
        self.freq_timer < 8 || (!self.sweep.negate && next_freq >= 0x800)
    }

    fn output(&self) -> f32 {
        if Self::DUTY_TABLE[self.duty_cycle as usize][self.duty_counter as usize] != 0
            && self.length.counter != 0
            && !self.sweep_forcing_silence()
        {
            if self.envelope.enabled {
                f32::from(self.envelope.volume)
            } else {
                f32::from(self.envelope.constant_volume)
            }
        } else {
            0f32
        }
    }

    // $4000 Pulse control
    fn write_control(&mut self, val: u8) {
        self.duty_cycle = (val >> 6) & 0x03; // D7..D6
        self.length.write_control(val);
        self.envelope.write_control(val);
    }
    // $4001 Pulse sweep
    fn write_sweep(&mut self, val: u8) {
        self.sweep.timer = (val >> 4) & 0x07; // D6..D4
        self.sweep.negate = (val >> 3) & 1 == 1; // D3
        self.sweep.shift = val & 0x07; // D2..D0
        self.sweep.enabled = ((val >> 7) & 1 == 1) && (self.sweep.shift != 0); // D7
        self.sweep.reload = true;
    }
    // $4002 Pulse timer lo
    fn write_timer_lo(&mut self, val: u8) {
        self.freq_timer = (self.freq_timer & 0xFF00) | u16::from(val); // D7..D0
    }
    // $4003 Pulse timer hi
    fn write_timer_hi(&mut self, val: u8) {
        self.freq_timer = (self.freq_timer & 0x00FF) | u16::from(val & 0x07) << 8; // D2..D0
        self.freq_counter = self.freq_timer;
        self.duty_counter = 0;
        self.envelope.reset = true;
        if self.enabled {
            self.length.load_value(val);
        }
    }
}

impl Savable for Pulse {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.enabled.save(fh)?;
        self.duty_cycle.save(fh)?;
        self.duty_counter.save(fh)?;
        self.freq_timer.save(fh)?;
        self.freq_counter.save(fh)?;
        self.channel.save(fh)?;
        self.length.save(fh)?;
        self.envelope.save(fh)?;
        self.sweep.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.enabled.load(fh)?;
        self.duty_cycle.load(fh)?;
        self.duty_counter.load(fh)?;
        self.freq_timer.load(fh)?;
        self.freq_counter.load(fh)?;
        self.channel.load(fh)?;
        self.length.load(fh)?;
        self.envelope.load(fh)?;
        self.sweep.load(fh)
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum PulseChannel {
    One,
    Two,
}

impl Savable for PulseChannel {
    fn save(&self, fh: &mut Write) -> Result<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => PulseChannel::One,
            1 => PulseChannel::Two,
            _ => panic!("invalid PulseChannel value"),
        };
        Ok(())
    }
}

struct Triangle {
    enabled: bool,
    ultrasonic: bool,
    step: u8,
    freq_timer: u16,
    freq_counter: u16,
    length: LengthCounter,
    linear: LinearCounter,
}

impl Triangle {
    fn new() -> Self {
        Self {
            enabled: false,
            ultrasonic: false,
            step: 0u8,
            freq_timer: 0u16,
            freq_counter: 0u16,
            length: LengthCounter::new(),
            linear: LinearCounter::new(),
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn clock(&mut self) {
        self.ultrasonic = false;
        if self.length.counter > 0 && self.freq_timer < 2 && self.freq_counter == 0 {
            self.ultrasonic = true;
        }

        let should_clock =
            !(self.length.counter == 0 || self.linear.counter == 0 || self.ultrasonic);
        if should_clock {
            if self.freq_counter > 0 {
                self.freq_counter -= 1;
            } else {
                self.freq_counter = self.freq_timer;
                self.step = (self.step + 1) & 0x1F;
            }
        }
    }

    fn clock_quarter_frame(&mut self) {
        if self.linear.reload {
            self.linear.counter = self.linear.load;
        } else if self.linear.counter > 0 {
            self.linear.counter -= 1;
        }
        if !self.linear.control {
            self.linear.reload = false;
        }
    }

    fn clock_half_frame(&mut self) {
        self.length.clock();
    }

    fn output(&self) -> f32 {
        if self.ultrasonic {
            7.5
        } else if self.step & 0x10 == 0x10 {
            f32::from(self.step ^ 0x1F)
        } else {
            f32::from(self.step)
        }
    }

    fn write_linear_counter(&mut self, val: u8) {
        self.linear.control = (val >> 7) & 1 == 1; // D7
        self.length.enabled = (val >> 7) & 1 == 0; // !D7
        self.linear.load_value(val);
    }

    fn write_timer_lo(&mut self, val: u8) {
        self.freq_timer = (self.freq_timer & 0xFF00) | u16::from(val); // D7..D0
    }

    fn write_timer_hi(&mut self, val: u8) {
        self.freq_timer = (self.freq_timer & 0x00FF) | u16::from(val & 0x07) << 8; // D2..D0
        self.freq_counter = self.freq_timer;
        self.linear.reload = true;
        if self.enabled {
            self.length.load_value(val);
        }
    }
}

impl Savable for Triangle {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.enabled.save(fh)?;
        self.ultrasonic.save(fh)?;
        self.step.save(fh)?;
        self.freq_timer.save(fh)?;
        self.freq_counter.save(fh)?;
        self.length.save(fh)?;
        self.linear.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.enabled.load(fh)?;
        self.ultrasonic.load(fh)?;
        self.step.load(fh)?;
        self.freq_timer.load(fh)?;
        self.freq_counter.load(fh)?;
        self.length.load(fh)?;
        self.linear.load(fh)
    }
}

struct Noise {
    enabled: bool,
    freq_timer: u16,       // timer freq_counter reload value
    freq_counter: u16,     // Current frequency timer value
    shift: u16,            // Must never be 0
    shift_mode: ShiftMode, // Zero (XOR bits 0 and 1) or One (XOR bits 0 and 6)
    length: LengthCounter,
    envelope: Envelope,
}

impl Noise {
    const FREQ_TABLE: [u16; 16] = [
        4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
    ];
    const SHIFT_BIT_15_MASK: u16 = !0x8000;

    fn new() -> Self {
        Self {
            enabled: false,
            freq_timer: 0u16,
            freq_counter: 0u16,
            shift: 1u16, // Must never be 0
            shift_mode: ShiftMode::Zero,
            length: LengthCounter::new(),
            envelope: Envelope::new(),
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn clock(&mut self) {
        if self.freq_counter > 0 {
            self.freq_counter -= 1;
        } else {
            self.freq_counter = self.freq_timer;
            let shift_amount = if self.shift_mode == ShiftMode::One {
                6
            } else {
                1
            };
            let bit1 = self.shift & 1; // Bit 0
            let bit2 = (self.shift >> shift_amount) & 1; // Bit 1 or 6 from above
            self.shift = (self.shift & Self::SHIFT_BIT_15_MASK) | ((bit1 ^ bit2) << 14);
            self.shift >>= 1;
        }
    }

    fn clock_quarter_frame(&mut self) {
        self.envelope.clock();
    }

    fn clock_half_frame(&mut self) {
        self.length.clock();
    }

    fn output(&self) -> f32 {
        if self.shift & 1 == 0 && self.length.counter != 0 {
            if self.envelope.enabled {
                f32::from(self.envelope.volume)
            } else {
                f32::from(self.envelope.constant_volume)
            }
        } else {
            0f32
        }
    }

    fn write_control(&mut self, val: u8) {
        self.length.write_control(val);
        self.envelope.write_control(val);
    }

    // $400E Noise timer
    fn write_timer(&mut self, val: u8) {
        self.freq_timer = Self::FREQ_TABLE[(val & 0x0F) as usize];
        self.shift_mode = if (val >> 7) & 1 == 1 {
            ShiftMode::One
        } else {
            ShiftMode::Zero
        };
    }

    fn write_length(&mut self, val: u8) {
        if self.enabled {
            self.length.load_value(val);
        }
        self.envelope.reset = true;
    }
}

impl Savable for Noise {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.enabled.save(fh)?;
        self.freq_timer.save(fh)?;
        self.freq_counter.save(fh)?;
        self.shift.save(fh)?;
        self.shift_mode.save(fh)?;
        self.length.save(fh)?;
        self.envelope.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.enabled.load(fh)?;
        self.freq_timer.load(fh)?;
        self.freq_counter.load(fh)?;
        self.shift.load(fh)?;
        self.shift_mode.load(fh)?;
        self.length.load(fh)?;
        self.envelope.load(fh)
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum ShiftMode {
    Zero,
    One,
}

impl Savable for ShiftMode {
    fn save(&self, fh: &mut Write) -> Result<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => ShiftMode::Zero,
            1 => ShiftMode::One,
            _ => panic!("invalid ShiftMode value"),
        };
        Ok(())
    }
}

pub struct DMC {
    pub cpu: *mut Cpu,
    irq_enabled: bool,
    irq_pending: bool,
    loops: bool,
    freq_timer: u16,
    freq_counter: u16,
    addr: u16,
    addr_load: u16,
    length: u8,
    length_load: u8,
    sample_buffer: u8,
    sample_buffer_empty: bool,
    output: u8,
    output_bits: u8,
    output_shift: u8,
    output_silent: bool,
}

impl DMC {
    // NTSC
    const NTSC_FREQ_TABLE: [u16; 16] = [
        0x1AC, 0x17C, 0x154, 0x140, 0x11E, 0x0FE, 0x0E2, 0x0D6, 0x0BE, 0x0A0, 0x08E, 0x080, 0x06A,
        0x054, 0x048, 0x036,
    ];
    // TODO PAL
    fn new() -> Self {
        Self {
            cpu: std::ptr::null_mut(),
            irq_enabled: false,
            irq_pending: false,
            loops: false,
            freq_timer: 0u16,
            freq_counter: 0u16,
            addr: 0u16,
            addr_load: 0u16,
            length: 0u8,
            length_load: 0u8,
            sample_buffer: 0u8,
            sample_buffer_empty: false,
            output: 0u8,
            output_bits: 0u8,
            output_shift: 0u8,
            output_silent: false,
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    fn clock(&mut self) {
        if self.freq_counter > 0 {
            self.freq_counter -= 1;
        } else {
            self.freq_counter = self.freq_timer;
            if !self.output_silent {
                if (self.output_shift & 1 == 1) && self.output < 0x7E {
                    self.output += 2;
                }
                if (self.output_shift & 1 == 0) && self.output > 0x01 {
                    self.output -= 2;
                }
            }
            self.output_shift >>= 1;

            self.output_bits = self.output_bits.saturating_sub(1);
            if self.output_bits == 0 {
                self.output_bits = 8;
                self.output_shift = self.sample_buffer;
                self.output_silent = self.sample_buffer_empty;
                self.sample_buffer_empty = true;
            }
        }

        if self.length > 0 && self.sample_buffer_empty {
            let cpu: &mut Cpu = unsafe { &mut *self.cpu }; // TODO ugly work-around to access CPU
            self.sample_buffer = cpu.read(self.addr);
            self.sample_buffer_empty = false;
            self.addr = (self.addr + 1) | 0x8000;
            self.length -= 1;

            if self.length == 0 {
                if self.loops {
                    self.length = self.length_load;
                    self.addr = self.addr_load;
                } else if self.irq_enabled {
                    self.irq_pending = true;
                }
            }
        }
    }

    fn output(&self) -> f32 {
        f32::from(self.output)
    }

    // $4010 DMC timer
    fn write_timer(&mut self, val: u8) {
        self.irq_enabled = (val >> 7) & 1 == 1;
        self.loops = (val >> 6) & 1 == 1;
        self.freq_timer = Self::NTSC_FREQ_TABLE[(val & 0x0F) as usize];
        if !self.irq_enabled {
            self.irq_pending = false;
        }
    }

    // $4011 DMC output
    fn write_output(&mut self, val: u8) {
        self.output = val >> 1;
    }

    // $4012 DMC addr load
    fn write_addr_load(&mut self, val: u8) {
        self.addr_load = 0xC000 | (u16::from(val) << 6);
    }

    // $4013 DMC length
    fn write_length(&mut self, val: u8) {
        self.length_load = (val << 4) + 1;
    }
}

impl Savable for DMC {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.irq_enabled.save(fh)?;
        self.irq_pending.save(fh)?;
        self.loops.save(fh)?;
        self.freq_timer.save(fh)?;
        self.freq_counter.save(fh)?;
        self.addr.save(fh)?;
        self.addr_load.save(fh)?;
        self.length.save(fh)?;
        self.length_load.save(fh)?;
        self.sample_buffer.save(fh)?;
        self.sample_buffer_empty.save(fh)?;
        self.output.save(fh)?;
        self.output_bits.save(fh)?;
        self.output_shift.save(fh)?;
        self.output_silent.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.irq_enabled.load(fh)?;
        self.irq_pending.load(fh)?;
        self.loops.load(fh)?;
        self.freq_timer.load(fh)?;
        self.freq_counter.load(fh)?;
        self.addr.load(fh)?;
        self.addr_load.load(fh)?;
        self.length.load(fh)?;
        self.length_load.load(fh)?;
        self.sample_buffer.load(fh)?;
        self.sample_buffer_empty.load(fh)?;
        self.output.load(fh)?;
        self.output_bits.load(fh)?;
        self.output_shift.load(fh)?;
        self.output_silent.load(fh)
    }
}

struct LengthCounter {
    enabled: bool,
    counter: u8, // Entry into LENGTH_TABLE
}

impl LengthCounter {
    const LENGTH_TABLE: [u8; 32] = [
        10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96,
        22, 192, 24, 72, 26, 16, 28, 32, 30,
    ];

    fn new() -> Self {
        Self {
            enabled: false,
            counter: 0u8,
        }
    }

    fn clock(&mut self) {
        if self.enabled && self.counter > 0 {
            self.counter -= 1;
        }
    }

    fn load_value(&mut self, val: u8) {
        self.counter = Self::LENGTH_TABLE[(val >> 3) as usize]; // D7..D3
    }

    fn write_control(&mut self, val: u8) {
        self.enabled = (val >> 5) & 1 == 0; // !D5
    }
}

impl Savable for LengthCounter {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.enabled.save(fh)?;
        self.counter.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.enabled.load(fh)?;
        self.counter.load(fh)
    }
}

struct LinearCounter {
    reload: bool,
    control: bool,
    load: u8,
    counter: u8,
}

impl LinearCounter {
    fn new() -> Self {
        Self {
            reload: false,
            control: false,
            load: 0u8,
            counter: 0u8,
        }
    }

    fn load_value(&mut self, val: u8) {
        self.load = val >> 1; // D6..D0
    }
}

impl Savable for LinearCounter {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.reload.save(fh)?;
        self.control.save(fh)?;
        self.load.save(fh)?;
        self.counter.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.reload.load(fh)?;
        self.control.load(fh)?;
        self.load.load(fh)?;
        self.counter.load(fh)
    }
}

struct Envelope {
    enabled: bool,
    loops: bool,
    reset: bool,
    volume: u8,
    constant_volume: u8,
    counter: u8,
}

impl Envelope {
    fn new() -> Self {
        Self {
            enabled: false,
            loops: false,
            reset: false,
            volume: 0u8,
            constant_volume: 0u8,
            counter: 0u8,
        }
    }

    fn clock(&mut self) {
        if self.reset {
            self.reset = false;
            self.volume = 0x0F;
            self.counter = self.constant_volume;
        } else if self.counter > 0 {
            self.counter -= 1;
        } else {
            self.counter = self.constant_volume;
            if self.volume > 0 {
                self.volume -= 1;
            } else if self.loops {
                self.volume = 0x0F;
            }
        }
    }

    // $4000/$4004/$400C Envelope control
    fn write_control(&mut self, val: u8) {
        self.loops = (val >> 5) & 1 == 1; // D5
        self.enabled = (val >> 4) & 1 == 0; // !D4
        self.constant_volume = val & 0x0F; // D3..D0
    }
}

impl Savable for Envelope {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.enabled.save(fh)?;
        self.loops.save(fh)?;
        self.reset.save(fh)?;
        self.volume.save(fh)?;
        self.constant_volume.save(fh)?;
        self.counter.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.enabled.load(fh)?;
        self.loops.load(fh)?;
        self.reset.load(fh)?;
        self.volume.load(fh)?;
        self.constant_volume.load(fh)?;
        self.counter.load(fh)
    }
}

struct Sweep {
    enabled: bool,
    reload: bool,
    negate: bool, // Treats PulseChannel 1 differently than PulseChannel 2
    timer: u8,    // counter reload value
    counter: u8,  // current timer value
    shift: u8,
}

impl Savable for Sweep {
    fn save(&self, fh: &mut Write) -> Result<()> {
        self.enabled.save(fh)?;
        self.reload.save(fh)?;
        self.negate.save(fh)?;
        self.timer.save(fh)?;
        self.counter.save(fh)?;
        self.shift.save(fh)
    }
    fn load(&mut self, fh: &mut Read) -> Result<()> {
        self.enabled.load(fh)?;
        self.reload.load(fh)?;
        self.negate.load(fh)?;
        self.timer.load(fh)?;
        self.counter.load(fh)?;
        self.shift.load(fh)
    }
}
