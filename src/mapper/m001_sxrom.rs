//! `SxROM`/`MMC1` (Mapper 1)
//!
//! <http://wiki.nesdev.com/w/index.php/SxROM>
//! <http://wiki.nesdev.com/w/index.php/MMC1>

use crate::{
    cartridge::Cartridge,
    common::{Clocked, Powered},
    mapper::{Mapper, MapperType, Mirroring},
    memory::{BankedMemory, MemRead, MemWrite, RamState},
    serialization::Savable,
    NesResult,
};
use std::{
    fmt,
    io::{Read, Write},
};

const PRG_RAM_WINDOW: usize = 8 * 1024;
const PRG_ROM_WINDOW: usize = 16 * 1024;
const CHR_WINDOW: usize = 4 * 1024;
const PRG_RAM_SIZE: usize = 32 * 1024; // 32KB is safely compatible sans NES 2.0 header
const CHR_RAM_SIZE: usize = 8 * 1024;

const SHIFT_REG_RESET: u8 = 0x80; // Reset shift register when bit 7 is set
const DEFAULT_SHIFT_REGISTER: u8 = 0x10; // 0b10000 the 1 is used to tell when register is full
const MIRRORING_MASK: u8 = 0x03; // 0b00011
const PRG_MODE_MASK: u8 = 0x0C; // 0b01100
const CHR_MODE_MASK: u8 = 0x10; // 0b10000

// Mode 1 is 0 or 1 for switch32
const PRG_MODE_FIX_FIRST: u8 = 0x08; // Mode 2
const PRG_MODE_FIX_LAST: u8 = 0x0C; // Mode 3
const PRG_BANK_MASK: u8 = 0x0F;
const PRG_RAM_DISABLED: u8 = 0x10; // 0b10000

#[derive(Debug, Clone)]
#[must_use]
pub struct Sxrom {
    regs: SxRegs,
    submapper_num: u8,
    has_chr_ram: bool,
    mirroring: Mirroring,
    battery_backed: bool,
    prg_ram: BankedMemory, // CPU $6000..=$7FFF 8K PRG RAM Bank (optional)
    // CPU $8000..=$BFFF 16KB PRG ROM Bank Switchable or Fixed to First Bank
    // CPU $C000..=$FFFF 16KB PRG ROM Bank Fixed to Last Bank or Switchable
    prg_rom: BankedMemory,
    chr: BankedMemory, // PPU $0000..=$1FFF 2 4KB CHR ROM/RAM Bank Switchable
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
enum Mmc1Regs {
    A000,
    C000,
}

#[derive(Clone)]
#[must_use]
struct SxRegs {
    write_just_occurred: u8,
    shift_register: u8, // $8000-$FFFF - 5 bit shift register
    control: u8,        // $8000-$9FFF
    chr_bank0: u8,      // $A000-$BFFF
    chr_bank1: u8,      // $C000-$DFFF
    prg_bank: u8,       // $E000-$FFFF
    last_chr_reg: Mmc1Regs,
    open_bus: u8,
}

impl Sxrom {
    pub fn load(cart: Cartridge, state: RamState) -> MapperType {
        let prg_ram_size = cart.prg_ram_size.unwrap_or(PRG_RAM_SIZE);
        let has_chr_ram = cart.chr_rom.is_empty();
        let battery_backed = cart.battery_backed();
        let prg_rom = BankedMemory::from(cart.prg_rom, PRG_ROM_WINDOW);
        let mut sxrom = Self {
            regs: SxRegs {
                write_just_occurred: 0x00,
                shift_register: DEFAULT_SHIFT_REGISTER,
                control: PRG_MODE_FIX_LAST,
                chr_bank0: 0x00,
                chr_bank1: 0x00,
                prg_bank: PRG_RAM_DISABLED,
                last_chr_reg: Mmc1Regs::A000,
                open_bus: 0x00,
            },
            submapper_num: cart.header.submapper_num,
            has_chr_ram,
            mirroring: Mirroring::SingleScreenA,
            battery_backed,
            prg_ram: BankedMemory::ram(prg_ram_size, PRG_RAM_WINDOW, state),
            prg_rom,
            chr: if has_chr_ram {
                BankedMemory::ram(CHR_RAM_SIZE, CHR_WINDOW, state)
            } else {
                BankedMemory::from(cart.chr_rom, CHR_WINDOW)
            },
        };
        sxrom.prg_ram.add_bank(0x6000, 0x7FFF);
        sxrom.prg_rom.add_bank_range(0x8000, 0xFFFF);
        sxrom.chr.add_bank_range(0x0000, 0x1FFF);
        sxrom.update_banks();
        sxrom.into()
    }

    /// Writes data into a shift register. At every 5th
    /// write, the data is written out to the `SxROM` registers
    /// and the shift register is cleared
    ///
    /// Load Register $8000-$FFFF
    /// 7654 3210
    /// Rxxx xxxD
    /// |       +- Data bit to be shifted into shift register, LSB first
    /// +--------- 1: Reset shift register and write control with (Control OR $0C),
    ///               locking PRG ROM at $C000-$FFFF to the last bank.
    ///
    /// Control $8000-$9FFF
    /// 43210
    /// CPPMM
    /// |||++- Mirroring (0: one-screen, lower bank; 1: one-screen, upper bank;
    /// |||               2: vertical; 3: horizontal)
    /// |++--- PRG ROM bank mode (0, 1: switch 32 KB at $8000, ignoring low bit of bank number;
    /// |                         2: fix first bank at $8000 and switch 16 KB bank at $C000;
    /// |                         3: fix last bank at $C000 and switch 16 KB bank at $8000)
    /// +----- CHR ROM bank mode (0: switch 8 KB at a time; 1: switch two separate 4 KB banks)
    ///
    /// CHR bank 0 $A000-$BFFF
    /// 42310
    /// CCCCC
    /// +++++- Select 4 KB or 8 KB CHR bank at PPU $0000 (low bit ignored in 8 KB mode)
    ///
    /// CHR bank 1 $C000-$DFFF
    /// 43210
    /// CCCCC
    /// +++++- Select 4 KB CHR bank at PPU $1000 (ignored in 8 KB mode)
    ///
    /// For Mapper001
    /// $A000 and $C000:
    /// 43210
    /// EDCBA
    /// |||||
    /// ||||+- CHR A12
    /// |||+-- CHR A13, if extant (CHR >= 16k)
    /// ||+--- CHR A14, if extant; and PRG RAM A14, if extant (PRG RAM = 32k)
    /// |+---- CHR A15, if extant; and PRG RAM A13, if extant (PRG RAM >= 16k)
    /// +----- CHR A16, if extant; and PRG ROM A18, if extant (PRG ROM = 512k)
    ///
    /// PRG bank $E000-$FFFF
    /// 43210
    /// RPPPP
    /// |++++- Select 16 KB PRG ROM bank (low bit ignored in 32 KB mode)
    /// +----- PRG RAM chip enable (0: enabled; 1: disabled; ignored on MMC1A)
    fn write_registers(&mut self, addr: u16, val: u8) {
        if self.regs.write_just_occurred > 0 {
            return;
        }
        self.regs.write_just_occurred = 2;
        if val & SHIFT_REG_RESET == SHIFT_REG_RESET {
            self.regs.shift_register = DEFAULT_SHIFT_REGISTER;
            self.regs.control |= PRG_MODE_FIX_LAST;
        } else {
            // Check if its time to write
            let write = self.regs.shift_register & 1 == 1;
            // Move shift register and write lowest bit of val
            self.regs.shift_register >>= 1;
            self.regs.shift_register |= (val & 1) << 4;
            if write {
                match addr {
                    0x8000..=0x9FFF => self.regs.control = self.regs.shift_register,
                    0xA000..=0xBFFF => {
                        self.regs.last_chr_reg = Mmc1Regs::A000;
                        self.regs.chr_bank0 = self.regs.shift_register;
                    }
                    0xC000..=0xDFFF => {
                        self.regs.last_chr_reg = Mmc1Regs::C000;
                        self.regs.chr_bank1 = self.regs.shift_register;
                    }
                    0xE000..=0xFFFF => self.regs.prg_bank = self.regs.shift_register,
                    _ => unreachable!("impossible write"),
                }
                self.regs.shift_register = DEFAULT_SHIFT_REGISTER;
                self.update_banks();
            }
        }
    }

    fn update_banks(&mut self) {
        self.mirroring = match self.regs.control & MIRRORING_MASK {
            0 => Mirroring::SingleScreenA,
            1 => Mirroring::SingleScreenB,
            2 => Mirroring::Vertical,
            3 => Mirroring::Horizontal,
            _ => unreachable!("impossible mirroring mode"),
        };

        //         self.prg_ram_enabled = self.regs.prg_bank & PRG_RAM_DISABLED == 0;
        //         self.bank_select = if self.regs.control & 0x04 == 0x04 {
        //             BankSelect::x8000
        //         } else {
        //             BankSelect::xC000
        //         };
        //         self.prg_mode = if self.regs.control & 0x08 == 0x08 {
        //             PrgMode::Bank16k
        //         } else {
        //             PrgMode::Bank32K
        //         };
        //         self.chr_mode = if self.regs.control & 0x10 == 0x10 {
        //             ChrMode::Bank4k
        //         } else {
        //             ChrMode::Bank8K
        //         };

        //         let chr_bank0 = self.regs.chr_bank0 as usize & 0x1F;
        //         let chr_bank1 = self.regs.chr_bank1 as usize & 0x1F;
        //         let prg_bank = self.regs.prg_bank as usize & 0x0F;

        //         let extra_bank = if self.last_chr_bank == MMC1Regs::C000 && self.chr_mode == ChrMode::Bank4k
        //         {
        //             chr_bank1
        //         } else {
        //             chr_bank0
        //         };
        //         let prg_bank_select = if self.prg_rom.len() == 0x80000 {
        //             // 512kb carts use bit 7 of $A000/$C000 to select page
        //             // This is used for SUROM (Dragon Warrior 3/4, Dragon Quest 4)
        //             extra_bank & 0x10;
        //         } else {
        //             0
        //         };

        let extra_reg = if self.regs.last_chr_reg == Mmc1Regs::C000
            && self.regs.control & CHR_MODE_MASK == CHR_MODE_MASK
        {
            self.regs.chr_bank1
        } else {
            self.regs.chr_bank0
        };
        let prg_bank = (self.regs.prg_bank & PRG_BANK_MASK) as usize;
        let prg_bank_select = if self.prg_rom.len() == 0x80000 {
            (extra_reg & CHR_MODE_MASK) as usize
        } else {
            0x00
        };
        if self.submapper_num == 5 {
            // "001: 5 Fixed PRG    SEROM, SHROM, SH1ROM use a fixed 32k PRG ROM with no banking support.
            self.prg_rom.set_bank_range(0x8000, 0xFFFF, 0);
        } else {
            match self.regs.control & PRG_MODE_MASK {
                PRG_MODE_FIX_FIRST => {
                    self.prg_rom.set_bank(0x8000, prg_bank_select);
                    self.prg_rom.set_bank(0xC000, prg_bank | prg_bank_select);
                }
                PRG_MODE_FIX_LAST => {
                    let last_bank = self.prg_rom.last_bank();
                    self.prg_rom.set_bank(0x8000, prg_bank | prg_bank_select);
                    self.prg_rom.set_bank(0xC000, last_bank | prg_bank_select);
                }
                _ => {
                    // Switch32
                    self.prg_rom
                        .set_bank(0x8000, (prg_bank & 0xFE) | prg_bank_select);
                    self.prg_rom
                        .set_bank(0xC000, prg_bank | 0x01 | prg_bank_select);
                }
            }
        }

        let chr_bank0 = self.regs.chr_bank0 as usize;
        let chr_bank1 = self.regs.chr_bank1 as usize;
        if self.regs.control & CHR_MODE_MASK == CHR_MODE_MASK {
            self.chr.set_bank(0x0000, chr_bank0);
            self.chr.set_bank(0x1000, chr_bank1);
        } else {
            self.chr.set_bank(0x0000, chr_bank0 & 0xFE);
            self.chr.set_bank(0x1000, chr_bank0 | 0x01);
        }
    }

    const fn prg_ram_enabled(&self) -> bool {
        self.regs.prg_bank & PRG_RAM_DISABLED == 0
    }
}

impl Mapper for Sxrom {
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
    fn battery_backed(&self) -> bool {
        self.battery_backed
    }
    fn save_sram<F: Write>(&self, fh: &mut F) -> NesResult<()> {
        if self.battery_backed {
            self.prg_ram.save(fh)?;
        }
        Ok(())
    }
    fn load_sram<F: Read>(&mut self, fh: &mut F) -> NesResult<()> {
        if self.battery_backed {
            self.prg_ram.load(fh)?;
        }
        Ok(())
    }
    fn open_bus(&mut self, _addr: u16, val: u8) {
        self.regs.open_bus = val;
    }
}

impl MemRead for Sxrom {
    fn read(&mut self, addr: u16) -> u8 {
        self.peek(addr)
    }

    fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.chr.peek(addr),
            0x6000..=0x7FFF if self.prg_ram_enabled() => self.prg_ram.peek(addr),
            0x8000..=0xFFFF => self.prg_rom.peek(addr),
            // 0x4020..=0x5FFF Nothing at this range
            _ => self.regs.open_bus,
        }
    }
}

impl MemWrite for Sxrom {
    fn write(&mut self, addr: u16, val: u8) {
        match addr {
            0x0000..=0x1FFF => self.chr.write(addr, val),
            0x6000..=0x7FFF if self.prg_ram_enabled() => self.prg_ram.write(addr, val),
            0x8000..=0xFFFF => self.write_registers(addr, val),
            // 0x4020..=0x5FFF Nothing at this range
            _ => (),
        }
    }
}

impl Clocked for Sxrom {
    fn clock(&mut self) -> usize {
        if self.regs.write_just_occurred > 0 {
            self.regs.write_just_occurred -= 1;
        }
        1
    }
}

impl Powered for Sxrom {
    fn reset(&mut self) {
        self.regs.shift_register = DEFAULT_SHIFT_REGISTER;
        self.regs.control = PRG_MODE_FIX_LAST;
        self.regs.prg_bank = PRG_RAM_DISABLED;
        self.update_banks();
    }
    fn power_cycle(&mut self) {
        self.regs.write_just_occurred = 0;
        self.reset();
    }
}

impl Savable for Sxrom {
    fn save<F: Write>(&self, fh: &mut F) -> NesResult<()> {
        self.regs.save(fh)?;
        self.prg_ram.save(fh)?;
        if self.has_chr_ram {
            self.chr.save(fh)?;
        }
        Ok(())
    }
    fn load<F: Read>(&mut self, fh: &mut F) -> NesResult<()> {
        self.regs.load(fh)?;
        self.update_banks();
        self.prg_ram.load(fh)?;
        if self.has_chr_ram {
            self.chr.load(fh)?;
        }
        Ok(())
    }
}

impl Savable for Mmc1Regs {
    fn save<F: Write>(&self, fh: &mut F) -> NesResult<()> {
        (*self as u8).save(fh)
    }
    fn load<F: Read>(&mut self, fh: &mut F) -> NesResult<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => Mmc1Regs::A000,
            1 => Mmc1Regs::C000,
            _ => panic!("invalid Mmc1Regs value"),
        };
        Ok(())
    }
}

impl Savable for SxRegs {
    fn save<F: Write>(&self, fh: &mut F) -> NesResult<()> {
        self.write_just_occurred.save(fh)?;
        self.shift_register.save(fh)?;
        self.control.save(fh)?;
        self.chr_bank0.save(fh)?;
        self.chr_bank1.save(fh)?;
        self.prg_bank.save(fh)?;
        self.last_chr_reg.save(fh)?;
        self.open_bus.save(fh)?;
        Ok(())
    }
    fn load<F: Read>(&mut self, fh: &mut F) -> NesResult<()> {
        self.write_just_occurred.load(fh)?;
        self.shift_register.load(fh)?;
        self.control.load(fh)?;
        self.chr_bank0.load(fh)?;
        self.chr_bank1.load(fh)?;
        self.prg_bank.load(fh)?;
        self.last_chr_reg.load(fh)?;
        self.open_bus.load(fh)?;
        Ok(())
    }
}

impl fmt::Debug for SxRegs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prg_ram_enabled = self.prg_bank & PRG_RAM_DISABLED == PRG_RAM_DISABLED;
        f.debug_struct("SxRegs")
            .field("write_just_occurred", &self.write_just_occurred)
            .field(
                "shift_register",
                &format_args!("0b{:08b}", self.shift_register),
            )
            .field("control", &format_args!("0x{:02X}", self.control))
            .field("chr_bank0", &format_args!("0x{:02X}", self.chr_bank0))
            .field("chr_bank1", &format_args!("0x{:02X}", self.chr_bank1))
            .field("prg_bank", &format_args!("0x{:02X}", self.prg_bank & 0x0F))
            .field("prg_ram_enabled", &prg_ram_enabled)
            .field("open_bus", &format_args!("0x{:02X}", self.open_bus))
            .finish()
    }
}
