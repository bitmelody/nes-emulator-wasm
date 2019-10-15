//!AExROM (Mapper 7)
//!
//! [https://wiki.nesdev.com/w/index.php/AxROM]()

use crate::{
    cartridge::Cartridge,
    common::{Clocked, Powered},
    mapper::{Mapper, MapperRef, Mirroring},
    memory::{Banks, Memory, Ram, Rom},
    serialization::Savable,
    NesResult,
};
use std::{
    cell::RefCell,
    io::{Read, Write},
    rc::Rc,
};

const PRG_ROM_BANK_SIZE: usize = 32 * 1024;
const CHR_BANK_SIZE: usize = 8 * 1024;
const CHR_RAM_SIZE: usize = 8 * 1024;

/// AxROM
#[derive(Debug)]
pub struct Axrom {
    has_chr_ram: bool,
    mirroring: Mirroring,
    open_bus: u8,
    prg_rom_bank: usize,
    prg_rom_banks: Banks<Rom>,
    chr_banks: Banks<Ram>,
}

impl Axrom {
    pub fn load(cart: Cartridge) -> MapperRef {
        let prg_rom_banks = Banks::init(&cart.prg_rom, PRG_ROM_BANK_SIZE);
        let chr_banks = if cart.chr_rom.len() == 0 {
            let chr_ram = Ram::init(CHR_RAM_SIZE);
            Banks::init(&chr_ram, CHR_BANK_SIZE)
        } else {
            Banks::init(&cart.chr_rom.to_ram(), CHR_BANK_SIZE)
        };
        let axrom = Self {
            has_chr_ram: cart.chr_rom.len() == 0,
            mirroring: cart.mirroring(),
            open_bus: 0u8,
            prg_rom_bank: prg_rom_banks.len() - 1,
            prg_rom_banks,
            chr_banks,
        };
        Rc::new(RefCell::new(axrom))
    }
}

impl Mapper for Axrom {
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}

impl Memory for Axrom {
    fn read(&mut self, addr: u16) -> u8 {
        let val = self.peek(addr);
        self.open_bus = val;
        val
    }

    fn peek(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => self.chr_banks[0].peek(addr),
            0x6000..=0x7FFF => self.open_bus,
            0x8000..=0xFFFF => self.prg_rom_banks[self.prg_rom_bank].peek(addr - 0x8000),
            0x4020..=0x5FFF => self.open_bus, // Nothing at this range
            _ => {
                eprintln!("unhandled Axrom read at address: 0x{:04X}", addr);
                self.open_bus
            }
        }
    }

    fn write(&mut self, addr: u16, val: u8) {
        self.open_bus = val;
        match addr {
            0x0000..=0x1FFF => {
                if self.has_chr_ram {
                    self.chr_banks[0].write(addr, val)
                }
            }
            0x8000..=0xFFFF => {
                let bank = (val & 0x07) as usize;
                self.prg_rom_bank = if bank >= self.prg_rom_banks.len() {
                    (val & 0x03) as usize
                } else {
                    bank
                };
                self.mirroring = if val & 0x10 == 0x10 {
                    Mirroring::SingleScreenB
                } else {
                    Mirroring::SingleScreenA
                };
            }
            0x4020..=0x7FFF => (), // Nothing at this range
            _ => eprintln!("unhandled Axrom write at address: 0x{:04X}", addr),
        }
    }
}

impl Clocked for Axrom {}
impl Powered for Axrom {}

impl Savable for Axrom {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        self.has_chr_ram.save(fh)?;
        self.mirroring.save(fh)?;
        self.prg_rom_bank.save(fh)?;
        self.prg_rom_banks.save(fh)?;
        self.chr_banks.save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        self.has_chr_ram.load(fh)?;
        self.mirroring.load(fh)?;
        self.prg_rom_bank.load(fh)?;
        self.prg_rom_banks.load(fh)?;
        self.chr_banks.load(fh)
    }
}
