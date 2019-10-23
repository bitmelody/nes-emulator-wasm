//! ExROM/MMC5 (Mapper 5)
//!
//! [https://wiki.nesdev.com/w/index.php/ExROM]()
//! [https://wiki.nesdev.com/w/index.php/MMC5]()

use crate::{
    cartridge::Cartridge,
    common::{Clocked, Powered},
    mapper::{Mapper, MapperRef, Mirroring},
    memory::{Memory, Ram, Rom},
    serialization::Savable,
    NesResult,
};
use std::{
    cell::RefCell,
    fmt,
    io::{Read, Write},
    rc::Rc,
};

const PRG_RAM_BANK_SIZE: usize = 8 * 1024;
const PRG_RAM_SIZE: usize = 32 * 1024;
const EXRAM_SIZE: usize = 1024;

/// ExROM
pub struct Exrom {
    regs: ExRegs,
    open_bus: u8,
    irq_pending: bool,
    mirroring: Mirroring,
    battery_backed: bool,
    prg_banks: [usize; 5],
    chr_banks_spr: [usize; 8],
    chr_banks_bg: [usize; 4],
    last_chr_write: ChrBank,
    spr_fetch_count: u32,
    ppu_prev_addr: u16,
    ppu_prev_match: u8,
    ppu_reading: bool,
    ppu_idle: u8,
    ppu_in_vblank: bool,
    ppu_cycle: u16,
    ppu_rendering: bool,
    exram: Ram,
    prg_ram: [Ram; 2],
    prg_rom: Rom,
    chr: Ram,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum ChrBank {
    Spr,
    Bg,
}

#[derive(Debug)]
pub struct ExRegs {
    sprite8x16: bool,          // $2000 PPUCTRL: false = 8x8, true = 8x16
    prg_mode: u8,              // $5100
    chr_mode: u8,              // $5101
    chr_hi_bit: u8,            // $5130
    prg_ram_protect_a: bool,   // $5102
    prg_ram_protect_b: bool,   // $5103
    exram_mode: u8,            // $5104
    nametable_mirroring: u8,   // $5105
    fill_tile: u8,             // $5106
    fill_attr: u8,             // $5107
    vertical_split_mode: u8,   // $5200
    vertical_split_scroll: u8, // $5201
    vertical_split_bank: u8,   // $5202
    scanline_num_irq: u16,     // $5203: Write $00 to disable IRQs
    irq_enabled: bool,         // $5204
    irq_counter: u16,
    in_frame: bool,
    multiplicand: u8, // $5205: write
    multiplier: u8,   // $5206: write
    mult_result: u16, // $5205: read lo, $5206: read hi
}

impl Exrom {
    pub fn load(cart: Cartridge) -> MapperRef {
        let prg_ram = [Ram::init(PRG_RAM_SIZE), Ram::init(PRG_RAM_SIZE)];
        let exram = Ram::init(EXRAM_SIZE);
        let num_rom_banks = cart.prg_rom.len() / (8 * 1024); // Default PRG ROM Bank size

        let mut exrom = Self {
            regs: ExRegs {
                sprite8x16: false,
                prg_mode: 0x03,
                chr_mode: 0x03,
                chr_hi_bit: 0u8,
                prg_ram_protect_a: false,
                prg_ram_protect_b: false,
                exram_mode: 0xFF,
                nametable_mirroring: 0xFF,
                fill_tile: 0xFF,
                fill_attr: 0xFF,
                vertical_split_mode: 0xFF,
                vertical_split_scroll: 0xFF,
                vertical_split_bank: 0xFF,
                scanline_num_irq: 0xFF,
                irq_enabled: false,
                irq_counter: 0u16,
                in_frame: false,
                multiplicand: 0xFF,
                multiplier: 0xFF,
                mult_result: 0xFE01,
            },
            open_bus: 0u8,
            irq_pending: false,
            mirroring: cart.mirroring(),
            battery_backed: cart.battery_backed(),
            prg_banks: [0; 5],
            chr_banks_spr: [0; 8],
            chr_banks_bg: [0; 4],
            last_chr_write: ChrBank::Spr,
            spr_fetch_count: 0u32,
            ppu_prev_addr: 0xFFFF,
            ppu_prev_match: 0u8,
            ppu_reading: false,
            ppu_idle: 0u8,
            ppu_in_vblank: false,
            ppu_cycle: 0,
            ppu_rendering: false,
            exram,
            prg_ram,
            prg_rom: cart.prg_rom,
            chr: cart.chr_rom.to_ram(),
        };
        exrom.prg_banks[3] = 0x80 | (num_rom_banks - 2);
        exrom.prg_banks[4] = 0x80 | (num_rom_banks - 1);
        Rc::new(RefCell::new(exrom))
    }

    // $5113: [.... .CPP]
    //      8k PRG-RAM @ $6000
    //      C = Chip select
    // $5114-5117: [RPPP PPPP]
    //      R = ROM select (0=select RAM, 1=select ROM)  **unused in $5117**
    //      P = PRG page
    //
    //              $6000   $8000   $A000   $C000   $E000
    //            +-------+-------------------------------+
    // P=%00:     | $5113 |           <<$5117>>           |
    //            +-------+-------------------------------+
    // P=%01:     | $5113 |    <$5115>    |    <$5117>    |
    //            +-------+---------------+-------+-------+
    // P=%10:     | $5113 |    <$5115>    | $5116 | $5117 |
    //            +-------+---------------+-------+-------+
    // P=%11:     | $5113 | $5114 | $5115 | $5116 | $5117 |
    //            +-------+-------+-------+-------+-------+
    fn write_prg_bankswitching(&mut self, addr: u16, val: u8) {
        let rom_mask = (val & 0x80) as usize;
        let bank = (val & 0x7F) as usize;
        match addr {
            0x5113 => self.prg_banks[0] = bank,
            0x5114 if self.regs.prg_mode == 0x03 => self.prg_banks[1] = bank | rom_mask,
            0x5115 => {
                match self.regs.prg_mode {
                    1 | 2 => self.prg_banks[1] = bank >> 1 | rom_mask,
                    3 => self.prg_banks[2] = bank | rom_mask,
                    _ => (), // Do nothing
                }
            }
            0x5116 if self.regs.prg_mode > 0x01 => {
                // 0x02 selects bank 2
                // 0x03 selects bank 3
                self.prg_banks[self.regs.prg_mode as usize] = bank | rom_mask;
            }
            0x5117 => {
                // 0x00 shifts 2, and uses bank 1
                // 0x01 shifts 1 and uses bank 2
                // 0x02 shifts 0 and uses bank 3
                // 0x03 shifts 0 and uses bank 4
                let shift = 2usize.saturating_sub(self.regs.prg_mode as usize);
                self.prg_banks[1 + self.regs.prg_mode as usize] = (bank >> shift) | rom_mask;
            }
            _ => (), // Do nothing
        }
    }

    // 'A' Set (sprites):
    //               $0000   $0400   $0800   $0C00   $1000   $1400   $1800   $1C00
    //             +---------------------------------------------------------------+
    //   C=%00:    |                             $5127                             |
    //             +---------------------------------------------------------------+
    //   C=%01:    |             $5123             |             $5127             |
    //             +-------------------------------+-------------------------------+
    //   C=%10:    |     $5121     |     $5123     |     $5125     |     $5127     |
    //             +---------------+---------------+---------------+---------------+
    //   C=%11:    | $5120 | $5121 | $5122 | $5123 | $5124 | $5125 | $5126 | $5127 |
    //             +-------+-------+-------+-------+-------+-------+-------+-------+
    //
    // 'B' Set (BG):
    //               $0000   $0400   $0800   $0C00   $1000   $1400   $1800   $1C00
    //             +-------------------------------+-------------------------------+
    //   C=%00:    |                             $512B                             |
    //             +-------------------------------+-------------------------------+
    //   C=%01:    |             $512B             |             $512B             |
    //             +-------------------------------+-------------------------------+
    //   C=%10:    |     $5129     |     $512B     |     $5129     |     $512B     |
    //             +---------------+---------------+---------------+---------------+
    //   C=%11:    | $5128 | $5129 | $512A | $512B | $5128 | $5129 | $512A | $512B |
    //             +-------+-------+-------+-------+-------+-------+-------+-------+
    fn get_chr_addr(&self, addr: u16) -> usize {
        let (bank_size, bank_idx_a, bank_idx_b) = match self.regs.chr_mode {
            0 => (8 * 1024, 7, 3),
            1 => (4 * 1024, if addr < 0x1000 { 3 } else { 7 }, 3),
            2 => {
                let bank_size = 2 * 1024;
                let bank_idx_a = match addr {
                    0x0000..=0x07FF => 1,
                    0x0800..=0x0FFF => 3,
                    0x1000..=0x17FF => 5,
                    0x1800..=0x1FFF => 7,
                    _ => panic!("invalid addr"),
                };
                let bank_idx_b = match addr {
                    0x0000..=0x07FF => 1,
                    0x0800..=0x0FFF => 3,
                    0x1000..=0x17FF => 1,
                    0x1800..=0x1FFF => 3,
                    _ => panic!("invalid addr"),
                };
                (bank_size, bank_idx_a, bank_idx_b)
            }
            _ => (1024, (addr >> 10) & 0x0F, (addr >> 10) & 0x03),
        };
        let bank = if self.regs.sprite8x16 {
            // Means we've gotten our 32 BG tiles fetched (32 * 4)
            if self.spr_fetch_count >= 127 && self.spr_fetch_count <= 158 {
                self.chr_banks_spr[bank_idx_a as usize]
            } else {
                self.chr_banks_bg[bank_idx_b as usize]
            }
        } else if self.last_chr_write == ChrBank::Spr {
            self.chr_banks_spr[bank_idx_a as usize]
        } else {
            self.chr_banks_bg[bank_idx_b as usize]
        };
        let offset = addr as usize % bank_size;
        bank * bank_size + offset
    }

    fn nametable_mode(&self, addr: u16) -> u16 {
        let table_size = 0x0400;
        let addr = (addr - 0x2000) % 0x1000 as u16;
        let table = addr / table_size;
        u16::from((self.regs.nametable_mirroring >> (2 * table)) & 0x03)
    }
}

impl Mapper for Exrom {
    fn irq_pending(&mut self) -> bool {
        self.regs.irq_enabled && self.irq_pending
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    fn vram_change(&mut self, addr: u16) {
        self.spr_fetch_count += 1;
        if (addr >> 12) == 0x02 && addr == self.ppu_prev_addr {
            self.ppu_prev_match += 1;
            if self.ppu_prev_match == 2 {
                if !self.regs.in_frame {
                    self.regs.in_frame = true;
                    self.regs.irq_counter = 0;
                } else {
                    self.regs.irq_counter = self.regs.irq_counter.wrapping_add(1);
                    if self.regs.irq_counter == self.regs.scanline_num_irq {
                        self.irq_pending = true;
                    }
                }
                self.spr_fetch_count = 0;
            }
        } else {
            self.ppu_prev_match = 0;
        }
        self.ppu_prev_addr = addr;
        self.ppu_reading = true;
    }

    fn use_ciram(&self, addr: u16) -> bool {
        let mode = self.nametable_mode(addr);
        match mode {
            0 | 1 => true,
            _ => false,
        }
    }

    fn nametable_addr(&self, addr: u16) -> u16 {
        let mode = self.nametable_mode(addr);
        match mode {
            0 | 1 => {
                let table_size = 0x0400;
                let offset = addr % table_size;
                0x2000 + mode * table_size + offset
            }
            _ => 0,
        }
    }

    fn ppu_write(&mut self, addr: u16, val: u8) {
        match addr {
            0x2000 => {
                self.regs.sprite8x16 = val & 0x20 > 0;
            }
            0x2001 => {
                self.ppu_rendering = val & 0x18 > 0; // 1, 2, or 3
                if !self.ppu_rendering {
                    self.regs.in_frame = false;
                }
            }
            0x2002 => self.ppu_in_vblank = val & 0x80 > 0,
            _ => (),
        }
    }
    fn open_bus(&mut self, _addr: u16, val: u8) {
        self.open_bus = val;
    }
}

impl Memory for Exrom {
    fn read(&mut self, addr: u16) -> u8 {
        let val = self.peek(addr);
        match addr {
            0x5204 => {
                // Reading from IRQ status clears it
                self.irq_pending = false;
            }
            0xFFFA | 0xFFFB => {
                self.regs.in_frame = false;
            }
            _ => (),
        }
        val
    }

    fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => {
                let addr = self.get_chr_addr(addr) % self.chr.len();
                self.chr[addr]
            }
            0x2000..=0x3EFF => {
                let mode = self.nametable_mode(addr);
                let addr = addr as usize % 0x0400;
                match mode {
                    2 => {
                        if self.regs.exram_mode == 0x02 {
                            0
                        } else {
                            self.exram[addr]
                        }
                    }
                    3 => {
                        if addr < 0x03C0 {
                            self.regs.fill_tile
                        } else {
                            self.regs.fill_attr
                        }
                    }
                    _ => 0,
                }
            }
            0x6000..=0x7FFF => {
                let bank = self.prg_banks[(addr - 0x6000) as usize / PRG_RAM_BANK_SIZE];
                let offset = (addr - 0x6000) as usize % PRG_RAM_BANK_SIZE;
                let addr = (bank * PRG_RAM_BANK_SIZE + offset) % self.prg_ram.len();
                let chip = (addr as usize >> 2) & 0x01;
                self.prg_ram[chip][addr]
            }
            0x8000..=0xFFFF => {
                let bank_size = match self.regs.prg_mode {
                    0 => 32 * 1024,
                    1 => 16 * 1024,
                    2 => match addr {
                        0x8000..=0xBFFF => 16 * 1024,
                        _ => 8 * 1024,
                    },
                    3 => 8 * 1024,
                    _ => panic!("invalid prg_mode"),
                };
                let bank = self.prg_banks[1 + (addr - 0x8000) as usize / bank_size];
                let offset = (addr - 0x8000) as usize % bank_size;
                // If bank is ROM
                let addr = ((bank & 0x7F) * bank_size + offset) % self.prg_rom.len();
                if bank & 0x80 > 0 {
                    self.prg_rom[addr]
                } else {
                    let chip = (addr as usize >> 2) & 0x01;
                    self.prg_ram[chip][addr]
                }
            }
            0x5C00..=0x5FFF => {
                // Modes 0-1 are nametable/attr modes and not used for RAM, thus are not readable
                if self.regs.exram_mode < 2 {
                    self.open_bus
                } else {
                    self.exram[addr as usize % 0x0400]
                }
            }
            0x5113..=0x5117 => 0, // TODO read prg_bank?
            0x5120..=0x5127 => self.chr_banks_spr[(addr & 0x07) as usize] as u8,
            0x5128..=0x512B => self.chr_banks_bg[(addr & 0x03) as usize] as u8,
            0x5000..=0x5003 => 0, // TODO Sound Pulse 1
            0x5004..=0x5007 => 0, // TODO Sound Pulse 2
            0x5010..=0x5011 => 0, // TODO Sound PCM
            0x5015 => 0,          // TODO Sound General
            0x5100 => self.regs.prg_mode,
            0x5101 => self.regs.chr_mode,
            0x5130 => self.regs.chr_hi_bit,
            0x5104 => self.regs.exram_mode,
            0x5105 => self.regs.nametable_mirroring,
            0x5106 => self.regs.fill_tile,
            0x5107 => self.regs.fill_attr,
            0x5200 => self.regs.vertical_split_mode,
            0x5201 => self.regs.vertical_split_scroll,
            0x5202 => self.regs.vertical_split_bank,
            0x5203 => self.regs.scanline_num_irq as u8,
            0x5204 => (self.irq_pending as u8) << 7 | (self.regs.in_frame as u8) << 6,
            0x5205 => (self.regs.mult_result & 0xFF) as u8,
            0x5206 => ((self.regs.mult_result >> 8) & 0xFF) as u8,
            0x5207 => self.open_bus, // TODO MMC5A only CL3 / SL3 Data Direction and Output Data Source
            0x5208 => self.open_bus, // TODO MMC5A only CL3 / SL3 Status
            0x5209 => self.open_bus, // TODO MMC5A only 6-bit Hardware Timer with IRQ
            0x5800..=0x5BFF => self.open_bus, // MMC5A unknown - reads open_bus
            _ => self.open_bus,
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x2000..=0x3EFF => {
                let mode = self.nametable_mode(addr);
                let addr = addr as usize % 0x0400;
                if mode == 2 && self.regs.exram_mode == 0x02 {
                    self.exram[addr] = val;
                }
            }
            0x6000..=0x7FFF => {
                let chip = (addr as usize >> 2) & 0x01;
                let bank = self.prg_banks[(addr - 0x6000) as usize / PRG_RAM_BANK_SIZE];
                let offset = (addr - 0x6000) as usize % PRG_RAM_BANK_SIZE;
                let addr = (bank * PRG_RAM_BANK_SIZE + offset) % self.prg_ram.len();
                self.prg_ram[chip][addr] = val;
            }
            0x8000..=0xDFFF => {
                let bank_size = match self.regs.prg_mode {
                    0 => 32 * 1024,
                    1 => 16 * 1024,
                    2 => match addr {
                        0x8000..=0xBFFF => 16 * 1024,
                        _ => 8 * 1024,
                    },
                    3 => 8 * 1024,
                    _ => panic!("invalid prg_mode"),
                };
                let bank = self.prg_banks[1 + (addr - 0x8000) as usize / bank_size];
                let offset = (addr - 0x8000) as usize % bank_size;
                if bank & 0x80 != 0x80 && self.regs.prg_ram_protect_a && self.regs.prg_ram_protect_b
                {
                    let chip = (addr as usize >> 2) & 0x01;
                    let addr = ((bank & 0x7F) * bank_size + offset) % self.prg_ram.len();
                    self.prg_ram[chip][addr] = val;
                }
            }
            // [DDCC BBAA]
            //
            // Allows each Nametable slot to be configured:
            //   [   A   ][   B   ]
            //   [   C   ][   D   ]
            //
            // Values can be the following:
            //   %00 = NES internal NTA
            //   %01 = NES internal NTB
            //   %10 = use ExRAM as NT
            //   %11 = Fill Mode
            //
            // For example... some typical mirroring setups would be:
            //                        (  D  C  B  A)
            //   Horizontal:     $50  (%01 01 00 00)
            //   Vertical:       $44  (%01 00 01 00)
            //   SingleScreenA:  $00  (%00 00 00 00)
            //   SingleScreenB:  $55  (%01 01 01 01)
            //   Fill:           $ff  (%11 11 11 11)
            0x5105 => {
                self.regs.nametable_mirroring = val;
                self.mirroring = match self.regs.nametable_mirroring {
                    0x50 => Mirroring::Horizontal,
                    0x44 => Mirroring::Vertical,
                    0x00 => Mirroring::SingleScreenA,
                    0x55 => Mirroring::SingleScreenB,
                    _ => Mirroring::FourScreen,
                };
            }
            // 'A' Chr Regs
            0x5120..=0x5127 => {
                self.last_chr_write = ChrBank::Spr;
                self.chr_banks_spr[(addr & 0x07) as usize] =
                    val as usize | (self.regs.chr_hi_bit as usize) << 8;
            }
            // 'B' Chr Regs
            0x5128..=0x512B => {
                self.last_chr_write = ChrBank::Bg;
                self.chr_banks_bg[(addr & 0x03) as usize] =
                    val as usize | (self.regs.chr_hi_bit as usize) << 8;
            }
            // PRG Bank Switching
            // $5113: [.... .PPP]
            //      8k PRG-RAM @ $6000
            // $5114-5117: [RPPP PPPP]
            //      R = ROM select (0=select RAM, 1=select ROM)  **unused in $5117**
            //      P = PRG page
            0x5113..=0x5117 => self.write_prg_bankswitching(addr, val),
            0x5C00..=0x5FFF => {
                // Mode 2 is writable
                if self.regs.exram_mode == 0x02 {
                    self.exram[addr as usize % 0x0400] = val;
                }
            }
            0x5000..=0x5003 => (), // TODO Sound Pulse 1
            0x5004..=0x5007 => (), // TODO Sound Pulse 2
            0x5010..=0x5011 => (), // TODO Sound PCM
            0x5015 => (),          // TODO Sound General
            // [.... ..PP]    PRG Mode
            //      %00 = 32k
            //      %01 = 16k
            //      %10 = 16k+8k
            //      %11 = 8k
            0x5100 => self.regs.prg_mode = val & 0x03,
            // [.... ..CC]    CHR Mode
            //      %00 = 8k Mode
            //      %01 = 4k Mode
            //      %10 = 2k Mode
            //      %11 = 1k Mode
            0x5101 => self.regs.chr_mode = val & 0x03,
            // [.... ..HH]
            0x5130 => self.regs.chr_hi_bit = val & 0x03,
            // [.... ..AA]    PRG-RAM Protect A
            // [.... ..BB]    PRG-RAM Protect B
            //      To allow writing to PRG-RAM you must set these regs to the following values:
            //         A=%10
            //         B=%01
            //      Any other values will prevent PRG-RAM writing.
            0x5102 => self.regs.prg_ram_protect_a = (val & 0x03) == 0x02,
            0x5103 => self.regs.prg_ram_protect_b = (val & 0x03) == 0x01,
            // [.... ..XX]    ExRAM mode
            //     %00 = Extra Nametable mode    ("Ex0")
            //     %01 = Extended Attribute mode ("Ex1")
            //     %10 = CPU access mode         ("Ex2")
            //     %11 = CPU read-only mode      ("Ex3")
            0x5104 => self.regs.exram_mode = val & 0x03,
            // [TTTT TTTT]     Fill Tile
            0x5106 => self.regs.fill_tile = val,
            // [.... ..AA]     Fill Attribute bits
            0x5107 => self.regs.fill_attr = val & 0x03,
            0x5200 => self.regs.vertical_split_mode = val,
            0x5201 => self.regs.vertical_split_scroll = val,
            0x5202 => self.regs.vertical_split_bank = val,
            0x5203 => self.regs.scanline_num_irq = u16::from(val),
            0x5204 => self.regs.irq_enabled = val & 0x80 > 0,
            0x5205 => self.regs.multiplicand = val,
            0x5206 => self.regs.mult_result = u16::from(self.regs.multiplicand) * u16::from(val),
            0x5207 => (), // TODO MMC5A only CL3 / SL3 Data Direction and Output Data Source
            0x5208 => (), // TODO MMC5A only CL3 / SL3 Status
            0x5209 => (), // TODO MMC5A only 6-bit Hardware Timer with IRQ
            0x5800..=0x5BFF => (), // MMC5A unknown
            0x0000..=0x1FFF => (), // ROM is write-only
            0xE000..=0xFFFF => (), // ROM is write-only
            _ => (),
        }
    }
}

impl Clocked for Exrom {
    fn clock(&mut self) -> usize {
        if self.ppu_reading {
            self.ppu_idle = 0;
        } else {
            self.ppu_idle += 1;
            if self.ppu_idle == 9 {
                // 3 CPU clocks == 9 Mapper clocks
                self.ppu_idle = 0;
                self.regs.in_frame = false;
            }
        }
        self.ppu_reading = false;
        1
    }
}

impl Powered for Exrom {
    fn reset(&mut self) {
        self.regs.prg_mode = 0x03;
        self.regs.chr_mode = 0x03;
    }
}

impl Savable for Exrom {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        self.regs.save(fh)?;
        self.open_bus.save(fh)?;
        self.irq_pending.save(fh)?;
        self.mirroring.save(fh)?;
        self.battery_backed.save(fh)?;
        self.prg_banks.save(fh)?;
        self.chr_banks_spr.save(fh)?;
        self.chr_banks_bg.save(fh)?;
        self.last_chr_write.save(fh)?;
        self.spr_fetch_count.save(fh)?;
        self.ppu_prev_addr.save(fh)?;
        self.ppu_prev_match.save(fh)?;
        self.ppu_reading.save(fh)?;
        self.ppu_idle.save(fh)?;
        self.ppu_in_vblank.save(fh)?;
        self.ppu_cycle.save(fh)?;
        self.ppu_rendering.save(fh)?;
        self.exram.save(fh)?;
        self.prg_ram.save(fh)?;
        self.prg_rom.save(fh)?;
        self.chr.save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        self.regs.load(fh)?;
        self.open_bus.load(fh)?;
        self.irq_pending.load(fh)?;
        self.mirroring.load(fh)?;
        self.battery_backed.load(fh)?;
        self.prg_banks.load(fh)?;
        self.chr_banks_spr.load(fh)?;
        self.chr_banks_bg.load(fh)?;
        self.last_chr_write.load(fh)?;
        self.spr_fetch_count.load(fh)?;
        self.ppu_prev_addr.load(fh)?;
        self.ppu_prev_match.load(fh)?;
        self.ppu_reading.load(fh)?;
        self.ppu_idle.load(fh)?;
        self.ppu_in_vblank.load(fh)?;
        self.ppu_cycle.load(fh)?;
        self.ppu_rendering.load(fh)?;
        self.exram.load(fh)?;
        self.prg_ram.load(fh)?;
        self.prg_rom.load(fh)?;
        self.chr.load(fh)
    }
}

impl Savable for ExRegs {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        self.sprite8x16.save(fh)?;
        self.prg_mode.save(fh)?;
        self.chr_mode.save(fh)?;
        self.chr_hi_bit.save(fh)?;
        self.prg_ram_protect_a.save(fh)?;
        self.prg_ram_protect_b.save(fh)?;
        self.exram_mode.save(fh)?;
        self.nametable_mirroring.save(fh)?;
        self.fill_tile.save(fh)?;
        self.fill_attr.save(fh)?;
        self.vertical_split_mode.save(fh)?;
        self.vertical_split_scroll.save(fh)?;
        self.vertical_split_bank.save(fh)?;
        self.scanline_num_irq.save(fh)?;
        self.irq_enabled.save(fh)?;
        self.irq_counter.save(fh)?;
        self.in_frame.save(fh)?;
        self.multiplicand.save(fh)?;
        self.multiplier.save(fh)?;
        self.mult_result.save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        self.sprite8x16.load(fh)?;
        self.prg_mode.load(fh)?;
        self.chr_mode.load(fh)?;
        self.chr_hi_bit.load(fh)?;
        self.prg_ram_protect_a.load(fh)?;
        self.prg_ram_protect_b.load(fh)?;
        self.exram_mode.load(fh)?;
        self.nametable_mirroring.load(fh)?;
        self.fill_tile.load(fh)?;
        self.fill_attr.load(fh)?;
        self.vertical_split_mode.load(fh)?;
        self.vertical_split_scroll.load(fh)?;
        self.vertical_split_bank.load(fh)?;
        self.scanline_num_irq.load(fh)?;
        self.irq_enabled.load(fh)?;
        self.irq_counter.load(fh)?;
        self.in_frame.load(fh)?;
        self.multiplicand.load(fh)?;
        self.multiplier.load(fh)?;
        self.mult_result.load(fh)
    }
}

impl Savable for ChrBank {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => ChrBank::Spr,
            1 => ChrBank::Bg,
            _ => panic!("invalid ChrBank value"),
        };
        Ok(())
    }
}

impl fmt::Debug for Exrom {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Exrom {{ }}")
    }
}
