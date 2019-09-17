//! A 6502 Central Processing Unit
//!
//! [http://wiki.nesdev.com/w/index.php/CPU]()

#[cfg(debug_assertions)]
use crate::console::debugger::Debugger;
use crate::memory::Memory;
use crate::serialization::Savable;
use crate::Result;
use std::fmt;
use std::io::{Read, Write};

pub const MASTER_CLOCK_RATE: f64 = 21_477_270.0; // 21.47727 MHz

// 1.79 MHz (~559 ns/cycle) - May want to use 1_786_830 for a stable 60 FPS
// http://forums.nesdev.com/viewtopic.php?p=223679#p223679
pub const CPU_CLOCK_RATE: f64 = MASTER_CLOCK_RATE / 12.0; // 1.7897725 MHz

const NMI_ADDR: u16 = 0xFFFA; // NMI Vector address
const IRQ_ADDR: u16 = 0xFFFE; // IRQ Vector address
const RESET_ADDR: u16 = 0xFFFC; // Vector address at reset
const POWER_ON_SP: u8 = 0xFD; // Because reasons. Possibly because of NMI/IRQ/BRK messing with SP on reset
const POWER_ON_STATUS: u8 = 0x24; // 0010 0100 - Unused and Interrupt Disable set - nestest seems to keep Unused set
const POWER_ON_CYCLES: u64 = 7; // Power up takes 7 cycles according to nestest - though some docs say 8
const SP_BASE: u16 = 0x0100; // Stack-pointer starting address

// Status Registers
// http://wiki.nesdev.com/w/index.php/Status_flags
// 7654 3210
// NVUB DIZC
// |||| ||||
// |||| |||+- Carry
// |||| ||+-- Zero
// |||| |+--- Interrupt Disable
// |||| +---- Decimal Mode - Not used in the NES but still has to function
// |||+------ Break - 1 when pushed to stack from PHP/BRK, 0 from IRQ/NMI
// ||+------- Unused - always set to 1 when pushed to stack
// |+-------- Overflow
// +--------- Negative
enum StatusRegs {
    C = 1,        // Carry
    Z = (1 << 1), // Zero
    I = (1 << 2), // Disable Interrupt
    D = (1 << 3), // Decimal Mode
    B = (1 << 4), // Break
    U = (1 << 5), // Unused
    V = (1 << 6), // Overflow
    N = (1 << 7), // Negative
}
use StatusRegs::*;

/// The Central Processing Unit status and registers
pub struct Cpu<M>
where
    M: Memory,
{
    pub mem: M,
    pub cycle_count: u64,     // total number of cycles ran
    stall: u64,               // Number of cycles to stall with nop (used by DMA)
    pub step: u64,            // total number of CPU instructions run
    pub pc: u16,              // program counter
    sp: u8,                   // stack pointer - stack is at $0100-$01FF
    acc: u8,                  // accumulator
    x: u8,                    // x register
    y: u8,                    // y register
    status: u8,               // Status Registers
    instr: Instr,             // The currently executing instruction
    abs_addr: u16,            // Used memory addresses get set here
    rel_addr: u16,            // Relative address for branch instructions
    fetched_data: u8,         // Represents data fetched for the ALU
    pub interrupt: Interrupt, // Pending interrupt
    #[cfg(debug_assertions)]
    debugger: Debugger,
    #[cfg(debug_assertions)]
    pub log_enabled: bool,
    #[cfg(test)]
    pub nestestlog: Vec<String>,
}

impl<M> Cpu<M>
where
    M: Memory,
{
    pub fn init(mem: M) -> Self {
        let mut cpu = Self {
            mem,
            cycle_count: POWER_ON_CYCLES,
            stall: 0u64,
            step: 0u64,
            pc: 0x0000,
            sp: POWER_ON_SP,
            acc: 0x00,
            x: 0x00,
            y: 0x00,
            status: POWER_ON_STATUS,
            instr: INSTRUCTIONS[0x00],
            abs_addr: 0x0000,
            rel_addr: 0x0000,
            fetched_data: 0x00,
            interrupt: Interrupt::None,
            #[cfg(debug_assertions)]
            debugger: Debugger::new(),
            #[cfg(debug_assertions)]
            log_enabled: false,
            #[cfg(test)]
            nestestlog: Vec::with_capacity(10000),
        };
        cpu.pc = cpu.readw(RESET_ADDR);
        cpu
    }

    pub fn power_on(&mut self) {
        self.pc = self.readw(RESET_ADDR);
    }

    #[cfg(debug_assertions)]
    pub fn log(&mut self, val: bool) {
        self.log_enabled = val;
    }

    /// Runs the CPU one cycle
    pub fn clock(&mut self) -> u64 {
        if self.stall > 0 {
            self.stall -= 1;
            return 1;
        }

        let start_cycle = self.cycle_count;

        match self.interrupt {
            Interrupt::IRQ => self.irq(),
            Interrupt::NMI => self.nmi(),
            _ => (),
        }
        self.interrupt = Interrupt::None;

        let opcode = self.read(self.pc);
        self.set_flag(U, true);
        self.pc = self.pc.wrapping_add(1);
        #[cfg(debug_assertions)]
        let log_pc = self.pc;

        self.instr = INSTRUCTIONS[opcode as usize];

        // let extra_cycle_req1 = (self.instr.decode_addr_mode())(self); // Set address based on addr_mode
        let mode_cycle = match self.instr.addr_mode() {
            IMM => self.imm(),
            ZP0 => self.zp0(),
            ZPX => self.zpx(),
            ZPY => self.zpy(),
            ABS => self.abs(),
            ABX => self.abx(),
            ABY => self.aby(),
            IND => self.ind(),
            IDX => self.idx(),
            IDY => self.idy(),
            REL => self.rel(),
            ACC => self.acc(),
            IMP => self.imp(),
        } as u64;

        #[cfg(debug_assertions)]
        {
            if self.log_enabled {
                self.print_instruction(log_pc);
            }
            // else if self.debugger.enabled() {
            //     let debugger: *mut Debugger = &mut self.debugger;
            //     let cpu: *mut Cpu<MemoryMap> = self;

            //     unsafe { (*debugger).on_clock(&mut (*cpu), log_pc) };
            // }
        }

        // let op_cycle = (self.instr.execute())(self); // Execute operation
        let op_cycle = match self.instr.op() {
            ADC => self.adc(), // ADd with Carry M with A
            AND => self.and(), // AND M with A
            ASL => self.asl(), // Arithmatic Shift Left M or A
            BCC => self.bcc(), // Branch on Carry Clear
            BCS => self.bcs(), // Branch if Carry Set
            BEQ => self.beq(), // Branch if EQual to zero
            BIT => self.bit(), // Test BITs of M with A (Affects N, V and Z)
            BMI => self.bmi(), // Branch on MInus (negative)
            BNE => self.bne(), // Branch if Not Equal to zero
            BPL => self.bpl(), // Branch on PLus (positive)
            BRK => self.brk(), // BReaK (forced interrupt)
            BVC => self.bvc(), // Branch if no oVerflow Set
            BVS => self.bvs(), // Branch on oVerflow Set
            CLC => self.clc(), // CLear Carry flag
            CLD => self.cld(), // CLear Decimal mode
            CLI => self.cli(), // CLear Interrupt disable
            CLV => self.clv(), // CLear oVerflow flag
            CMP => self.cmp(), // CoMPare
            CPX => self.cpx(), // ComPare with X
            CPY => self.cpy(), // ComPare with Y
            DEC => self.dec(), // DECrement M or A
            DEX => self.dex(), // DEcrement X
            DEY => self.dey(), // DEcrement Y
            EOR => self.eor(), // Exclusive-OR M with A
            INC => self.inc(), // INCrement M or A
            INX => self.inx(), // INcrement X
            INY => self.iny(), // INcrement Y
            JMP => self.jmp(), // JuMP - safe to unwrap because JMP is Absolute
            JSR => self.jsr(), // Jump and Save Return addr - safe to unwrap because JSR is Absolute
            LDA => self.lda(), // LoaD A with M
            LDX => self.ldx(), // LoaD X with M
            LDY => self.ldy(), // LoaD Y with M
            LSR => self.lsr(), // Logical Shift Right M or A
            NOP => self.nop(), // NO oPeration
            SKB => self.skb(), // Like NOP, but issues a dummy read
            IGN => self.ign(), // Like NOP, but issues a dummy read
            ORA => self.ora(), // OR with A
            PHA => self.pha(), // PusH A to the stack
            PHP => self.php(), // PusH Processor status to the stack
            PLA => self.pla(), // PulL A from the stack
            PLP => self.plp(), // PulL Processor status from the stack
            ROL => self.rol(), // ROtate Left M or A
            ROR => self.ror(), // ROtate Right M or A
            RTI => self.rti(), // ReTurn from Interrupt
            RTS => self.rts(), // ReTurn from Subroutine
            SBC => self.sbc(), // Subtract M from A with carry
            SEC => self.sec(), // SEt Carry flag
            SED => self.sed(), // SEt Decimal mode
            SEI => self.sei(), // SEt Interrupt disable
            STA => self.sta(), // STore A into M
            STX => self.stx(), // STore X into M
            STY => self.sty(), // STore Y into M
            TAX => self.tax(), // Transfer A to X
            TAY => self.tay(), // Transfer A to Y
            TSX => self.tsx(), // Transfer SP to X
            TXA => self.txa(), // TRansfer X to A
            TXS => self.txs(), // Transfer X to SP
            TYA => self.tya(), // Transfer Y to A
            ISB => self.isb(), // INC & SBC
            DCP => self.dcp(), // DEC & CMP
            AXS => self.axs(), // (A & X) - val into X
            LAS => self.las(), // LDA & TSX
            LAX => self.lax(), // LDA & TAX
            AHX => self.ahx(), // Store A & X & H in M
            SAX => self.sax(), // Sotre A & X in M
            XAA => self.xaa(), // TXA & AND
            SHX => self.shx(), // Store X & H in M
            RRA => self.rra(), // ROR & ADC
            TAS => self.tas(), // STA & TXS
            SHY => self.shy(), // Store Y & H in M
            ARR => self.arr(), // AND #imm & ROR
            SRE => self.sre(), // LSR & EOR
            ALR => self.alr(), // AND #imm & LSR
            RLA => self.rla(), // ROL & AND
            ANC => self.anc(), // AND #imm
            SLO => self.slo(), // ASL & ORA
            XXX => self.xxx(), // Unimplemented opcode
        } as u64;
        self.step += 1;
        self.cycle_count = self
            .cycle_count
            .wrapping_add(self.instr.cycles())
            .wrapping_add(mode_cycle & op_cycle);
        self.cycle_count - start_cycle
    }

    #[cfg(debug_assertions)]
    pub fn debug(&mut self, val: bool) {
        if val {
            self.debugger.start();
        } else {
            self.debugger.stop();
        }
    }

    /// Sends an IRQ Interrupt to the CPU
    ///
    /// http://wiki.nesdev.com/w/index.php/IRQ
    pub fn trigger_irq(&mut self) {
        if self.get_flag(I) > 0 {
            return;
        }
        self.interrupt = Interrupt::IRQ;
    }
    pub fn irq(&mut self) {
        // #[cfg(debug_assertions)]
        // {
        //     let debugger: *mut Debugger = &mut self.debugger;
        //     let cpu: *mut Cpu<MemoryMap> = self;
        //     unsafe { (*debugger).on_nmi(&mut (*cpu)) };
        // }
        self.push_stackw(self.pc);
        // Handles status flags differently than php()
        self.push_stackb((self.status | U as u8) & !(B as u8));
        self.pc = self.readw(IRQ_ADDR);
        self.set_flag(I, true);
        self.cycle_count = self.cycle_count.wrapping_add(7);
    }

    /// Sends a NMI Interrupt to the CPU
    ///
    /// http://wiki.nesdev.com/w/index.php/NMI
    pub fn trigger_nmi(&mut self) {
        self.interrupt = Interrupt::NMI;
    }
    fn nmi(&mut self) {
        // #[cfg(debug_assertions)]
        // {
        //     let debugger: *mut Debugger = &mut self.debugger;
        //     let cpu: *mut Cpu<MemoryMap> = self;
        //     unsafe { (*debugger).on_nmi(&mut (*cpu)) };
        // }
        self.push_stackw(self.pc);
        // Handles status flags differently than php()
        self.push_stackb((self.status | U as u8) & !(B as u8));
        self.pc = self.readw(NMI_ADDR);
        self.set_flag(I, true);
        self.cycle_count = self.cycle_count.wrapping_add(7);
    }

    // Getters/Setters

    // Used for testing to manually set the PC to a known value
    #[cfg(test)]
    pub fn set_pc(&mut self, addr: u16) {
        self.pc = addr;
    }

    // Status Register functions

    // Convenience method to set both Z and N
    fn set_flags_zn(&mut self, val: u8) {
        self.set_flag(Z, val == 0x00);
        self.set_flag(N, val & 0x80 == 0x80);
    }

    fn get_flag(&self, flag: StatusRegs) -> u8 {
        return if (self.status & flag as u8) > 0 { 1 } else { 0 };
    }

    fn set_flag(&mut self, flag: StatusRegs, val: bool) {
        if val {
            self.status |= flag as u8;
        } else {
            self.status &= !(flag as u8);
        }
    }

    // Stack Functions

    // Push a byte to the stack
    fn push_stackb(&mut self, val: u8) {
        self.write(SP_BASE | u16::from(self.sp), val);
        self.sp = self.sp.wrapping_sub(1);
    }

    // Pull a byte from the stack
    fn pop_stackb(&mut self) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        self.read(SP_BASE | u16::from(self.sp))
    }

    // Push a word (two bytes) to the stack
    fn push_stackw(&mut self, val: u16) {
        let lo = (val & 0xFF) as u8;
        let hi = (val >> 8) as u8;
        self.push_stackb(hi);
        self.push_stackb(lo);
    }

    // Pull a word (two bytes) from the stack
    fn pop_stackw(&mut self) -> u16 {
        let lo = u16::from(self.pop_stackb());
        let hi = u16::from(self.pop_stackb());
        hi << 8 | lo
    }

    /// Addressing Modes
    ///
    /// The 6502 can address 64KB from 0x0000 - 0xFFFF. The high byte is usually the page and the
    /// low byte the offset into the page. There are 256 total pages of 256 bytes.
    ///
    /// Several addressing modes require an additional clock if they cross a page boundary.  Each
    /// function returns either 0 or 1 if it requires an extra clock. This combined with the return
    /// from the operation will determine if a page boundary was crossed and if an extra clock was
    /// required.

    /// Accumulator
    /// No additional data is required, but the default target will be the accumulator.
    fn acc(&mut self) -> u8 {
        let _ = self.read(self.pc); // dummy read
        self.fetched_data = self.acc;
        return 0;
    }

    /// Implied
    /// No additional data is required, but the default target will be the accumulator.
    fn imp(&mut self) -> u8 {
        let _ = self.read(self.pc); // dummy read
        self.fetched_data = self.acc;
        return 0;
    }

    /// Immediate
    /// Uses the next byte as the value, so we'll update the abs_addr to the next byte.
    fn imm(&mut self) -> u8 {
        let _ = self.read(self.pc); // dummy read
        self.abs_addr = self.pc;
        self.pc = self.pc.wrapping_add(1);
        return 0;
    }

    /// Zero Page
    /// Accesses the first 0xFF bytes of the address range, so this only requires one extra byte
    /// instead of the usual two.
    fn zp0(&mut self) -> u8 {
        self.abs_addr = u16::from(self.read(self.pc)) & 0x00FF;
        self.pc = self.pc.wrapping_add(1);
        return 0;
    }

    /// Zero Page w/ X offset
    /// Same as Zero Page, but is offset by adding the x register.
    fn zpx(&mut self) -> u8 {
        self.abs_addr = u16::from(self.read(self.pc).wrapping_add(self.x)) & 0x00FF;
        self.pc = self.pc.wrapping_add(1);
        return 0;
    }

    /// Zero Page w/ Y offset
    /// Same as Zero Page, but is offset by adding the y register.
    fn zpy(&mut self) -> u8 {
        self.abs_addr = u16::from(self.read(self.pc).wrapping_add(self.y)) & 0x00FF;
        self.pc = self.pc.wrapping_add(1);
        return 0;
    }

    /// Relative
    /// This mode is only used by branching instructions. The address must be between -128 and +127,
    /// allowing the branching instruction to move backward or forward relative to the current
    /// program counter.
    fn rel(&mut self) -> u8 {
        self.rel_addr = self.read(self.pc).into();
        self.pc = self.pc.wrapping_add(1);
        if self.rel_addr & 0x80 == 0x80 {
            // If address is negative, extend sign to 16-bits
            self.rel_addr |= 0xFF00;
        }
        return 0;
    }

    /// Absolute
    /// Uses a full 16-bit address as the next value.
    fn abs(&mut self) -> u8 {
        self.abs_addr = self.readw(self.pc);

        // dummy read for read-modify-write instructions
        match self.instr.op() {
            ASL | LSR | ROL | ROR | INC | DEC | SLO | SRE | RLA | RRA | ISB | DCP => {
                let _ = self.read(self.abs_addr);
            }
            _ => (), // Do nothing
        }

        self.pc = self.pc.wrapping_add(2);
        return 0;
    }

    /// Absolute w/ X offset
    /// Same as Absolute, but is offset by adding the x register. If a page boundary is crossed, an
    /// additional clock is required.
    fn abx(&mut self) -> u8 {
        let addr = self.readw(self.pc);
        self.pc = self.pc.wrapping_add(2);
        self.abs_addr = addr.wrapping_add(self.x.into());

        // dummy read
        if (addr & 0x00FF).wrapping_add(self.x.into()) > 0x00FF {
            self.read((addr & 0xFF00) | (self.abs_addr & 0x00FF));
        }

        if self.pages_differ(addr, self.abs_addr) {
            return 1;
        } else {
            return 0;
        }
    }

    /// Absolute w/ Y offset
    /// Same as Absolute, but is offset by adding the y register. If a page boundary is crossed, an
    /// additional clock is required.
    fn aby(&mut self) -> u8 {
        let addr = self.readw(self.pc);
        self.pc = self.pc.wrapping_add(2);
        self.abs_addr = addr.wrapping_add(self.y.into());

        // dummy ST* read
        match self.instr.op() {
            STA | STX | STY if self.abs_addr == 0x2007 => {
                self.read((addr & 0xFF00) | (self.abs_addr & 0x00FF));
            }
            _ => (), // Do nothing
        }

        if self.pages_differ(addr, self.abs_addr) {
            return 1;
        } else {
            return 0;
        }
    }

    /// Indirect
    /// The next 16-bit address is used to get the actual 16-bit address. This instruction has
    /// a bug in the original hardware. If the lo byte is 0xFF, the hi byte would cross a page
    /// boundary. However, this doesn't work correctly on the original hardware and instead
    /// wraps back around to 0.
    fn ind(&mut self) -> u8 {
        let addr = self.readw(self.pc);
        self.pc = self.pc.wrapping_add(2);
        if addr & 0x00FF == 0x00FF {
            // Simulate bug
            self.abs_addr = (u16::from(self.read(addr & 0xFF00)) << 8) | u16::from(self.read(addr));
        } else {
            // Normal behavior
            self.abs_addr = (u16::from(self.read(addr + 1)) << 8) | u16::from(self.read(addr));
        }
        return 0;
    }

    /// Indirect X
    /// The next 8-bit address is offset by the X register to get the actual 16-bit address from
    /// page 0x00.
    fn idx(&mut self) -> u8 {
        let addr = self.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        let x_offset = addr.wrapping_add(self.x);
        self.abs_addr = self.readw_zp(x_offset);
        return 0;
    }

    /// Indirect Y
    /// The next 8-bit address is read to get a 16-bit address from page 0x00, which is then offset
    /// by the Y register. If a page boundary is crossed, add a clock cycle.
    fn idy(&mut self) -> u8 {
        let addr = self.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        let addr = self.readw_zp(addr);
        self.abs_addr = addr.wrapping_add(self.y.into());

        // dummy read
        if (addr & 0x00FF).wrapping_add(self.y.into()) > 0x00FF {
            self.read((addr & 0xFF00) | (self.abs_addr & 0x00FF));
        }

        if self.pages_differ(addr, self.abs_addr) {
            return 1;
        } else {
            return 0;
        }
    }

    // Source the data used by an instruction. Some instructions don't fetch data as the source
    // is implied by the instruction such as INX which increments the X register.
    fn fetch_data(&mut self) {
        let mode = self.instr.addr_mode();
        if mode != IMP && mode != ACC {
            self.fetched_data = self.read(self.abs_addr);
        }
    }

    // Writes data back to where fetched_data was sourced from. Either accumulator or memory
    // specified in abs_addr.
    fn write_fetched(&mut self, val: u8) {
        let mode = self.instr.addr_mode();
        if mode == IMP || mode == ACC {
            self.acc = val;
        } else {
            self.write(self.abs_addr, val);
        }
    }

    // Memory accesses

    // Utility to read a full 16-bit word
    fn readw(&mut self, addr: u16) -> u16 {
        let lo = u16::from(self.read(addr));
        let hi = u16::from(self.read(addr.wrapping_add(1)));
        (hi << 8) | lo
    }

    // readw but don't accidentally modify state
    fn peekw(&self, addr: u16) -> u16 {
        let lo = u16::from(self.peek(addr));
        let hi = u16::from(self.peek(addr.wrapping_add(1)));
        (hi << 8) | lo
    }

    // Like readw, but for Zero Page which means it'll wrap around at 0xFF
    fn readw_zp(&mut self, addr: u8) -> u16 {
        let lo = u16::from(self.read(addr.into()));
        let hi = u16::from(self.read(addr.wrapping_add(1).into()));
        (hi << 8) | lo
    }

    // Like peekw, but for Zero Page which means it'll wrap around at 0xFF
    fn peekw_zp(&self, addr: u8) -> u16 {
        let lo = u16::from(self.peek(addr.into()));
        let hi = u16::from(self.peek(addr.wrapping_add(1).into()));
        (hi << 8) | lo
    }

    // Copies data to the PPU OAMDATA ($2004) using DMA (Direct Memory Access)
    // http://wiki.nesdev.com/w/index.php/PPU_registers#OAMDMA
    fn write_oamdma(&mut self, addr: u8) {
        let mut addr = u16::from(addr) << 8; // Start at $XX00
        let oam_addr = 0x2004;
        for _ in 0..256 {
            // Copy 256 bytes from $XX00-$XXFF
            let val = self.read(addr);
            self.write(oam_addr, val);
            addr = addr.saturating_add(1);
        }
        self.stall += 513; // +2 for every read/write and +1 dummy cycle
        if self.cycle_count & 0x01 == 1 {
            // +1 cycle if on an odd cycle
            self.stall += 1;
        }
    }

    // Print the current instruction and status
    pub fn print_instruction(&mut self, pc: u16) {
        let mut bytes = Vec::new();
        let disasm = match self.instr.addr_mode() {
            IMM => {
                bytes.push(self.peek(pc));
                format!("#${:02X}", bytes[0])
            }
            ZP0 => {
                bytes.push(self.peek(pc));
                let val = self.peek(bytes[0].into());
                format!("${:02X} = {:02X}", bytes[0], val)
            }
            ZPX => {
                bytes.push(self.peek(pc));
                let x_offset = bytes[0].wrapping_add(self.x);
                let val = self.peek(x_offset.into());
                format!("${:02X},X @ {:02X} = {:02X}", bytes[0], x_offset, val)
            }
            ZPY => {
                bytes.push(self.peek(pc));
                let y_offset = bytes[0].wrapping_add(self.y);
                let val = self.peek(y_offset.into());
                format!("${:02X},Y @ {:02X} = {:02X}", bytes[0], y_offset, val)
            }
            ABS => {
                bytes.push(self.peek(pc));
                bytes.push(self.peek(pc.wrapping_add(1)));
                let addr = self.peekw(pc);
                if self.instr.op() == JMP || self.instr.op() == JSR {
                    format!("${:04X}", addr)
                } else {
                    let val = self.peek(addr.into());
                    format!("${:04X} = {:02X}", addr, val)
                }
            }
            ABX => {
                bytes.push(self.peek(pc));
                bytes.push(self.peek(pc.wrapping_add(1)));
                let addr = self.peekw(pc);
                let x_offset = addr.wrapping_add(self.x.into());
                let val = self.peek(x_offset.into());
                format!("${:04X},X @ {:04X} = {:02X}", addr, x_offset, val)
            }
            ABY => {
                bytes.push(self.peek(pc));
                bytes.push(self.peek(pc.wrapping_add(1)));
                let addr = self.peekw(pc);
                let y_offset = addr.wrapping_add(self.y.into());
                let val = self.peek(y_offset.into());
                format!("${:04X},Y @ {:04X} = {:02X}", addr, y_offset, val)
            }
            IND => {
                bytes.push(self.peek(pc));
                bytes.push(self.peek(pc.wrapping_add(1)));
                let addr = self.peekw(pc);
                let val = if addr & 0x00FF == 0x00FF {
                    (u16::from(self.peek(addr & 0xFF00)) << 8) | u16::from(self.peek(addr))
                } else {
                    (u16::from(self.peek(addr + 1)) << 8) | u16::from(self.peek(addr))
                };
                if self.instr.op() == JMP {
                    format!("(${:04X}) = {:04X}", addr, val)
                } else {
                    format!("(${:04X})", val)
                }
            }
            IDX => {
                bytes.push(self.peek(pc));
                let x_offset = bytes[0].wrapping_add(self.x);
                let addr = self.peekw_zp(x_offset);
                let val = self.peek(addr);
                format!(
                    "(${:02X},X) @ {:02X} = {:04X} = {:02X}",
                    bytes[0], x_offset, addr, val,
                )
            }
            IDY => {
                bytes.push(self.peek(pc));
                let addr = self.peekw_zp(bytes[0]);
                let y_offset = addr.wrapping_add(self.y.into());
                let val = self.peek(y_offset);
                format!(
                    "(${:02X}),Y = {:04X} @ {:04X} = {:02X}",
                    bytes[0], addr, y_offset, val,
                )
            }
            REL => {
                bytes.push(self.peek(pc));
                format!("${:04X}", pc.wrapping_add(1).wrapping_add(self.rel_addr))
            }
            ACC => "A ".to_string(),
            IMP => "".to_string(),
        };
        let mut bytes_str = String::new();
        for i in 0..2 {
            if i < bytes.len() {
                bytes_str.push_str(&format!("{:02X} ", bytes[i]));
            } else {
                bytes_str.push_str(&"   ".to_string());
            }
        }
        let opstr = format!(
            "{:04X}  {:02X} {}{:?} {:<26}  A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X} PPU:{:>3},{:>3} CYC:{}\n",
            pc.wrapping_sub(1),
            self.instr.opcode(),
            bytes_str,
            self.instr,
            disasm,
            self.acc,
            self.x,
            self.y,
            self.status,
            self.sp,
            0, // self.mem.ppu.cycle,
            0, // self.mem.ppu.scanline,
            self.cycle_count,
        );
        print!("{}", opstr);
        #[cfg(test)]
        self.nestestlog.push(opstr);
    }

    /// Utilities

    fn pages_differ(&self, addr1: u16, addr2: u16) -> bool {
        return (addr1 & 0xFF00) != (addr2 & 0xFF00);
    }
}

impl<M> Memory for Cpu<M>
where
    M: Memory,
{
    fn read(&mut self, addr: u16) -> u8 {
        self.mem.read(addr)
    }

    fn peek(&self, addr: u16) -> u8 {
        self.mem.peek(addr)
    }

    fn write(&mut self, addr: u16, val: u8) {
        if addr == 0x4014 {
            self.write_oamdma(val);
        } else {
            self.mem.write(addr, val);
        }
    }

    /// Resets the CPU
    ///
    /// Updates the PC, SP, and Status values to defined constants.
    ///
    /// These operations take the CPU 7 cycle.
    fn reset(&mut self) {
        self.mem.reset();
        self.cycle_count = POWER_ON_CYCLES;
        self.stall = 0u64;
        self.pc = self.readw(RESET_ADDR);
        self.sp = self.sp.saturating_sub(3);
        self.set_flag(I, true);
        #[cfg(test)]
        self.nestestlog.clear();
    }

    /// Power cycle the CPU
    ///
    /// Updates all status as if powered on for the first time
    ///
    /// These operations take the CPU 7 cycle.
    fn power_cycle(&mut self) {
        self.mem.power_cycle();
        self.cycle_count = POWER_ON_CYCLES;
        self.stall = 0u64;
        self.pc = self.readw(RESET_ADDR);
        self.sp = POWER_ON_SP;
        self.acc = 0u8;
        self.x = 0u8;
        self.y = 0u8;
        self.status = POWER_ON_STATUS;
        #[cfg(test)]
        self.nestestlog.clear();
    }
}

impl<M> Savable for Cpu<M>
where
    M: Memory + Savable,
{
    fn save(&self, fh: &mut dyn Write) -> Result<()> {
        self.mem.save(fh)?;
        self.cycle_count.save(fh)?;
        self.stall.save(fh)?;
        self.step.save(fh)?;
        self.pc.save(fh)?;
        self.sp.save(fh)?;
        self.acc.save(fh)?;
        self.x.save(fh)?;
        self.y.save(fh)?;
        self.status.save(fh)?;
        self.instr.save(fh)?;
        self.abs_addr.save(fh)?;
        self.rel_addr.save(fh)?;
        self.fetched_data.save(fh)?;
        self.interrupt.save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> Result<()> {
        self.mem.load(fh)?;
        self.cycle_count.load(fh)?;
        self.stall.load(fh)?;
        self.step.load(fh)?;
        self.pc.load(fh)?;
        self.sp.load(fh)?;
        self.acc.load(fh)?;
        self.x.load(fh)?;
        self.y.load(fh)?;
        self.status.load(fh)?;
        self.instr.load(fh)?;
        self.abs_addr.load(fh)?;
        self.rel_addr.load(fh)?;
        self.fetched_data.load(fh)?;
        self.interrupt.load(fh)
    }
}

#[derive(PartialEq, Eq, Copy, Clone)]
pub enum Interrupt {
    None,
    IRQ,
    NMI,
}

impl Savable for Interrupt {
    fn save(&self, fh: &mut dyn Write) -> Result<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> Result<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => Interrupt::None,
            1 => Interrupt::IRQ,
            2 => Interrupt::NMI,
            _ => panic!("invalid Interrupt value"),
        };
        Ok(())
    }
}

#[rustfmt::skip]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
// List of all CPU official and unofficial operations
// http://wiki.nesdev.com/w/index.php/6502_instructions
// http://archive.6502.org/datasheets/rockwell_r650x_r651x.pdf
pub enum Operation {
    ADC, AND, ASL, BCC, BCS, BEQ, BIT, BMI, BNE, BPL, BRK, BVC, BVS, CLC, CLD, CLI, CLV, CMP, CPX,
    CPY, DEC, DEX, DEY, EOR, INC, INX, INY, JMP, JSR, LDA, LDX, LDY, LSR, NOP, ORA, PHA, PHP, PLA,
    PLP, ROL, ROR, RTI, RTS, SBC, SEC, SED, SEI, STA, STX, STY, TAX, TAY, TSX, TXA, TXS, TYA,
    // "Unofficial" opcodes
    SKB, IGN, ISB, DCP, AXS, LAS, LAX, AHX, SAX, XAA, SHX, RRA, TAS, SHY, ARR, SRE, ALR, RLA, ANC,
    SLO, XXX
}

impl Savable for Operation {
    fn save(&self, fh: &mut dyn Write) -> Result<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> Result<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => Operation::ADC,
            1 => Operation::AND,
            2 => Operation::ASL,
            3 => Operation::BCC,
            4 => Operation::BCS,
            5 => Operation::BEQ,
            6 => Operation::BIT,
            7 => Operation::BMI,
            8 => Operation::BNE,
            9 => Operation::BPL,
            10 => Operation::BRK,
            11 => Operation::BVC,
            12 => Operation::BVS,
            13 => Operation::CLC,
            14 => Operation::CLD,
            15 => Operation::CLI,
            16 => Operation::CLV,
            17 => Operation::CMP,
            18 => Operation::CPX,
            19 => Operation::CPY,
            20 => Operation::DEC,
            21 => Operation::DEX,
            22 => Operation::DEY,
            23 => Operation::EOR,
            24 => Operation::INC,
            25 => Operation::INX,
            26 => Operation::INY,
            27 => Operation::JMP,
            28 => Operation::JSR,
            29 => Operation::LDA,
            30 => Operation::LDX,
            31 => Operation::LDY,
            32 => Operation::LSR,
            33 => Operation::NOP,
            34 => Operation::ORA,
            35 => Operation::PHA,
            36 => Operation::PHP,
            37 => Operation::PLA,
            38 => Operation::PLP,
            39 => Operation::ROL,
            40 => Operation::ROR,
            41 => Operation::RTI,
            42 => Operation::RTS,
            43 => Operation::SBC,
            44 => Operation::SEC,
            45 => Operation::SED,
            46 => Operation::SEI,
            47 => Operation::STA,
            48 => Operation::STX,
            49 => Operation::STY,
            50 => Operation::TAX,
            51 => Operation::TAY,
            52 => Operation::TSX,
            53 => Operation::TXA,
            54 => Operation::TXS,
            55 => Operation::TYA,
            56 => Operation::SKB,
            57 => Operation::IGN,
            58 => Operation::ISB,
            59 => Operation::DCP,
            60 => Operation::AXS,
            61 => Operation::LAS,
            62 => Operation::LAX,
            63 => Operation::AHX,
            64 => Operation::SAX,
            65 => Operation::XAA,
            66 => Operation::SHX,
            67 => Operation::RRA,
            68 => Operation::TAS,
            69 => Operation::SHY,
            70 => Operation::ARR,
            71 => Operation::SRE,
            72 => Operation::ALR,
            73 => Operation::RLA,
            74 => Operation::ANC,
            75 => Operation::SLO,
            76 => Operation::XXX,
            _ => panic!("invalid Operation value"),
        };
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[rustfmt::skip]
pub enum AddrMode {
    IMM,
    ZP0, ZPX, ZPY,
    ABS, ABX, ABY,
    IND, IDX, IDY,
    REL, ACC, IMP,
}

impl Savable for AddrMode {
    fn save(&self, fh: &mut dyn Write) -> Result<()> {
        (*self as u8).save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> Result<()> {
        let mut val = 0u8;
        val.load(fh)?;
        *self = match val {
            0 => AddrMode::IMM,
            1 => AddrMode::ZP0,
            2 => AddrMode::ZPX,
            3 => AddrMode::ZPY,
            4 => AddrMode::ABS,
            5 => AddrMode::ABX,
            6 => AddrMode::ABY,
            7 => AddrMode::IND,
            8 => AddrMode::IDX,
            9 => AddrMode::IDY,
            10 => AddrMode::REL,
            11 => AddrMode::ACC,
            12 => AddrMode::IMP,
            _ => panic!("invalid AddrMode value"),
        };
        Ok(())
    }
}

use AddrMode::*;
use Operation::*;

// (opcode, Addressing Mode, Operation, cycles taken)
#[derive(Copy, Clone)]
pub struct Instr(u8, AddrMode, Operation, u64);

impl Instr {
    pub fn opcode(&self) -> u8 {
        self.0
    }
    pub fn addr_mode(&self) -> AddrMode {
        self.1
    }
    pub fn op(&self) -> Operation {
        self.2
    }
    pub fn cycles(&self) -> u64 {
        self.3
    }
}

impl Savable for Instr {
    fn save(&self, fh: &mut dyn Write) -> Result<()> {
        self.0.save(fh)?;
        self.1.save(fh)?;
        self.2.save(fh)?;
        self.3.save(fh)
    }
    fn load(&mut self, fh: &mut dyn Read) -> Result<()> {
        self.0.load(fh)?;
        self.1.load(fh)?;
        self.2.load(fh)?;
        self.3.load(fh)
    }
}

// 16x16 grid of 6502 opcodes. Matches datasheet matrix for easy lookup
#[rustfmt::skip]
pub const INSTRUCTIONS: [Instr; 256] = [
    Instr(0x00, IMM, BRK, 7), Instr(0x01, IDX, ORA, 6), Instr(0x02, IMP, XXX, 2), Instr(0x03, IDX, SLO, 8), Instr(0x04, ZP0, NOP, 3), Instr(0x05, ZP0, ORA, 3), Instr(0x06, ZP0, ASL, 5), Instr(0x07, ZP0, SLO, 5), Instr(0x08, IMP, PHP, 3), Instr(0x09, IMM, ORA, 2), Instr(0x0A, ACC, ASL, 2), Instr(0x0B, IMM, ANC, 2), Instr(0x0C, ABS, NOP, 4), Instr(0x0D, ABS, ORA, 4), Instr(0x0E, ABS, ASL, 6), Instr(0x0F, ABS, SLO, 6),
    Instr(0x10, REL, BPL, 2), Instr(0x11, IDY, ORA, 5), Instr(0x12, IMP, XXX, 2), Instr(0x13, IDY, SLO, 8), Instr(0x14, ZPX, NOP, 4), Instr(0x15, ZPX, ORA, 4), Instr(0x16, ZPX, ASL, 6), Instr(0x17, ZPX, SLO, 6), Instr(0x18, IMP, CLC, 2), Instr(0x19, ABY, ORA, 4), Instr(0x1A, IMP, NOP, 2), Instr(0x1B, ABY, SLO, 7), Instr(0x1C, ABX, IGN, 4), Instr(0x1D, ABX, ORA, 4), Instr(0x1E, ABX, ASL, 7), Instr(0x1F, ABX, SLO, 7),
    Instr(0x20, ABS, JSR, 6), Instr(0x21, IDX, AND, 6), Instr(0x22, IMP, XXX, 2), Instr(0x23, IDX, RLA, 8), Instr(0x24, ZP0, BIT, 3), Instr(0x25, ZP0, AND, 3), Instr(0x26, ZP0, ROL, 5), Instr(0x27, ZP0, RLA, 5), Instr(0x28, IMP, PLP, 4), Instr(0x29, IMM, AND, 2), Instr(0x2A, ACC, ROL, 2), Instr(0x2B, IMM, ANC, 2), Instr(0x2C, ABS, BIT, 4), Instr(0x2D, ABS, AND, 4), Instr(0x2E, ABS, ROL, 6), Instr(0x2F, ABS, RLA, 6),
    Instr(0x30, REL, BMI, 2), Instr(0x31, IDY, AND, 5), Instr(0x32, IMP, XXX, 2), Instr(0x33, IDY, RLA, 8), Instr(0x34, ZPX, NOP, 4), Instr(0x35, ZPX, AND, 4), Instr(0x36, ZPX, ROL, 6), Instr(0x37, ZPX, RLA, 6), Instr(0x38, IMP, SEC, 2), Instr(0x39, ABY, AND, 4), Instr(0x3A, IMP, NOP, 2), Instr(0x3B, ABY, RLA, 7), Instr(0x3C, ABX, IGN, 4), Instr(0x3D, ABX, AND, 4), Instr(0x3E, ABX, ROL, 7), Instr(0x3F, ABX, RLA, 7),
    Instr(0x40, IMP, RTI, 6), Instr(0x41, IDX, EOR, 6), Instr(0x42, IMP, XXX, 2), Instr(0x43, IDX, SRE, 8), Instr(0x44, ZP0, NOP, 3), Instr(0x45, ZP0, EOR, 3), Instr(0x46, ZP0, LSR, 5), Instr(0x47, ZP0, SRE, 5), Instr(0x48, IMP, PHA, 3), Instr(0x49, IMM, EOR, 2), Instr(0x4A, ACC, LSR, 2), Instr(0x4B, IMM, ALR, 2), Instr(0x4C, ABS, JMP, 3), Instr(0x4D, ABS, EOR, 4), Instr(0x4E, ABS, LSR, 6), Instr(0x4F, ABS, SRE, 6),
    Instr(0x50, REL, BVC, 2), Instr(0x51, IDY, EOR, 5), Instr(0x52, IMP, XXX, 2), Instr(0x53, IDY, SRE, 8), Instr(0x54, ZPX, NOP, 4), Instr(0x55, ZPX, EOR, 4), Instr(0x56, ZPX, LSR, 6), Instr(0x57, ZPX, SRE, 6), Instr(0x58, IMP, CLI, 2), Instr(0x59, ABY, EOR, 4), Instr(0x5A, IMP, NOP, 2), Instr(0x5B, ABY, SRE, 7), Instr(0x5C, ABX, IGN, 4), Instr(0x5D, ABX, EOR, 4), Instr(0x5E, ABX, LSR, 7), Instr(0x5F, ABX, SRE, 7),
    Instr(0x60, IMP, RTS, 6), Instr(0x61, IDX, ADC, 6), Instr(0x62, IMP, XXX, 2), Instr(0x63, IDX, RRA, 8), Instr(0x64, ZP0, NOP, 3), Instr(0x65, ZP0, ADC, 3), Instr(0x66, ZP0, ROR, 5), Instr(0x67, ZP0, RRA, 5), Instr(0x68, IMP, PLA, 4), Instr(0x69, IMM, ADC, 2), Instr(0x6A, ACC, ROR, 2), Instr(0x6B, IMM, ARR, 2), Instr(0x6C, IND, JMP, 5), Instr(0x6D, ABS, ADC, 4), Instr(0x6E, ABS, ROR, 6), Instr(0x6F, ABS, RRA, 6),
    Instr(0x70, REL, BVS, 2), Instr(0x71, IDY, ADC, 5), Instr(0x72, IMP, XXX, 2), Instr(0x73, IDY, RRA, 8), Instr(0x74, ZPX, NOP, 4), Instr(0x75, ZPX, ADC, 4), Instr(0x76, ZPX, ROR, 6), Instr(0x77, ZPX, RRA, 6), Instr(0x78, IMP, SEI, 2), Instr(0x79, ABY, ADC, 4), Instr(0x7A, IMP, NOP, 2), Instr(0x7B, ABY, RRA, 7), Instr(0x7C, ABX, IGN, 4), Instr(0x7D, ABX, ADC, 4), Instr(0x7E, ABX, ROR, 7), Instr(0x7F, ABX, RRA, 7),
    Instr(0x80, IMM, SKB, 2), Instr(0x81, IDX, STA, 6), Instr(0x82, IMM, SKB, 2), Instr(0x83, IDX, SAX, 6), Instr(0x84, ZP0, STY, 3), Instr(0x85, ZP0, STA, 3), Instr(0x86, ZP0, STX, 3), Instr(0x87, ZP0, SAX, 3), Instr(0x88, IMP, DEY, 2), Instr(0x89, IMM, SKB, 2), Instr(0x8A, IMP, TXA, 2), Instr(0x8B, IMM, XAA, 2), Instr(0x8C, ABS, STY, 4), Instr(0x8D, ABS, STA, 4), Instr(0x8E, ABS, STX, 4), Instr(0x8F, ABS, SAX, 4),
    Instr(0x90, REL, BCC, 2), Instr(0x91, IDY, STA, 6), Instr(0x92, IMP, XXX, 2), Instr(0x93, IDY, AHX, 6), Instr(0x94, ZPX, STY, 4), Instr(0x95, ZPX, STA, 4), Instr(0x96, ZPY, STX, 4), Instr(0x97, ZPY, SAX, 4), Instr(0x98, IMP, TYA, 2), Instr(0x99, ABY, STA, 5), Instr(0x9A, IMP, TXS, 2), Instr(0x9B, ABY, TAS, 5), Instr(0x9C, ABX, SHY, 5), Instr(0x9D, ABX, STA, 5), Instr(0x9E, ABY, SHX, 5), Instr(0x9F, ABY, AHX, 5),
    Instr(0xA0, IMM, LDY, 2), Instr(0xA1, IDX, LDA, 6), Instr(0xA2, IMM, LDX, 2), Instr(0xA3, IDX, LAX, 6), Instr(0xA4, ZP0, LDY, 3), Instr(0xA5, ZP0, LDA, 3), Instr(0xA6, ZP0, LDX, 3), Instr(0xA7, ZP0, LAX, 3), Instr(0xA8, IMP, TAY, 2), Instr(0xA9, IMM, LDA, 2), Instr(0xAA, IMP, TAX, 2), Instr(0xAB, IMM, LAX, 2), Instr(0xAC, ABS, LDY, 4), Instr(0xAD, ABS, LDA, 4), Instr(0xAE, ABS, LDX, 4), Instr(0xAF, ABS, LAX, 4),
    Instr(0xB0, REL, BCS, 2), Instr(0xB1, IDY, LDA, 5), Instr(0xB2, IMP, XXX, 2), Instr(0xB3, IDY, LAX, 5), Instr(0xB4, ZPX, LDY, 4), Instr(0xB5, ZPX, LDA, 4), Instr(0xB6, ZPY, LDX, 4), Instr(0xB7, ZPY, LAX, 4), Instr(0xB8, IMP, CLV, 2), Instr(0xB9, ABY, LDA, 4), Instr(0xBA, IMP, TSX, 2), Instr(0xBB, ABY, LAS, 4), Instr(0xBC, ABX, LDY, 4), Instr(0xBD, ABX, LDA, 4), Instr(0xBE, ABY, LDX, 4), Instr(0xBF, ABY, LAX, 4),
    Instr(0xC0, IMM, CPY, 2), Instr(0xC1, IDX, CMP, 6), Instr(0xC2, IMM, SKB, 2), Instr(0xC3, IDX, DCP, 8), Instr(0xC4, ZP0, CPY, 3), Instr(0xC5, ZP0, CMP, 3), Instr(0xC6, ZP0, DEC, 5), Instr(0xC7, ZP0, DCP, 5), Instr(0xC8, IMP, INY, 2), Instr(0xC9, IMM, CMP, 2), Instr(0xCA, IMP, DEX, 2), Instr(0xCB, IMM, AXS, 2), Instr(0xCC, ABS, CPY, 4), Instr(0xCD, ABS, CMP, 4), Instr(0xCE, ABS, DEC, 6), Instr(0xCF, ABS, DCP, 6),
    Instr(0xD0, REL, BNE, 2), Instr(0xD1, IDY, CMP, 5), Instr(0xD2, IMP, XXX, 2), Instr(0xD3, IDY, DCP, 8), Instr(0xD4, ZPX, NOP, 4), Instr(0xD5, ZPX, CMP, 4), Instr(0xD6, ZPX, DEC, 6), Instr(0xD7, ZPX, DCP, 6), Instr(0xD8, IMP, CLD, 2), Instr(0xD9, ABY, CMP, 4), Instr(0xDA, IMP, NOP, 2), Instr(0xDB, ABY, DCP, 7), Instr(0xDC, ABX, IGN, 4), Instr(0xDD, ABX, CMP, 4), Instr(0xDE, ABX, DEC, 7), Instr(0xDF, ABX, DCP, 7),
    Instr(0xE0, IMM, CPX, 2), Instr(0xE1, IDX, SBC, 6), Instr(0xE2, IMM, SKB, 2), Instr(0xE3, IDX, ISB, 8), Instr(0xE4, ZP0, CPX, 3), Instr(0xE5, ZP0, SBC, 3), Instr(0xE6, ZP0, INC, 5), Instr(0xE7, ZP0, ISB, 5), Instr(0xE8, IMP, INX, 2), Instr(0xE9, IMM, SBC, 2), Instr(0xEA, IMP, NOP, 2), Instr(0xEB, IMM, SBC, 2), Instr(0xEC, ABS, CPX, 4), Instr(0xED, ABS, SBC, 4), Instr(0xEE, ABS, INC, 6), Instr(0xEF, ABS, ISB, 6),
    Instr(0xF0, REL, BEQ, 2), Instr(0xF1, IDY, SBC, 5), Instr(0xF2, IMP, XXX, 2), Instr(0xF3, IDY, ISB, 8), Instr(0xF4, ZPX, NOP, 4), Instr(0xF5, ZPX, SBC, 4), Instr(0xF6, ZPX, INC, 6), Instr(0xF7, ZPX, ISB, 6), Instr(0xF8, IMP, SED, 2), Instr(0xF9, ABY, SBC, 4), Instr(0xFA, IMP, NOP, 2), Instr(0xFB, ABY, ISB, 7), Instr(0xFC, ABX, IGN, 4), Instr(0xFD, ABX, SBC, 4), Instr(0xFE, ABX, INC, 7), Instr(0xFF, ABX, ISB, 7),
];

/// CPU instructions
impl<M> Cpu<M>
where
    M: Memory,
{
    /// Storage opcodes

    /// LDA: Load A with M
    fn lda(&mut self) -> u8 {
        self.fetch_data();
        self.acc = self.fetched_data;
        self.set_flags_zn(self.acc);
        return 1;
    }
    /// LDX: Load X with M
    fn ldx(&mut self) -> u8 {
        self.fetch_data();
        self.x = self.fetched_data;
        self.set_flags_zn(self.x);
        return 1;
    }
    /// LDY: Load Y with M
    fn ldy(&mut self) -> u8 {
        self.fetch_data();
        self.y = self.fetched_data;
        self.set_flags_zn(self.y);
        return 1;
    }
    /// STA: Store A into M
    fn sta(&mut self) -> u8 {
        self.write(self.abs_addr, self.acc);
        return 0;
    }
    /// STX: Store X into M
    fn stx(&mut self) -> u8 {
        self.write(self.abs_addr, self.x);
        return 0;
    }
    /// STY: Store Y into M
    fn sty(&mut self) -> u8 {
        self.write(self.abs_addr, self.y);
        return 0;
    }
    /// TAX: Transfer A to X
    fn tax(&mut self) -> u8 {
        self.x = self.acc;
        self.set_flags_zn(self.x);
        return 0;
    }
    /// TAY: Transfer A to Y
    fn tay(&mut self) -> u8 {
        self.y = self.acc;
        self.set_flags_zn(self.y);
        return 0;
    }
    /// TSX: Transfer Stack Pointer to X
    fn tsx(&mut self) -> u8 {
        self.x = self.sp;
        self.set_flags_zn(self.x);
        return 0;
    }
    /// TXA: Transfer X to A
    fn txa(&mut self) -> u8 {
        self.acc = self.x;
        self.set_flags_zn(self.acc);
        return 0;
    }
    /// TXS: Transfer X to Stack Pointer
    fn txs(&mut self) -> u8 {
        self.sp = self.x;
        return 0;
    }
    /// TYA: Transfer Y to A
    fn tya(&mut self) -> u8 {
        self.acc = self.y;
        self.set_flags_zn(self.acc);
        return 0;
    }

    /// Arithmetic opcodes

    /// ADC: Add M to A with Carry
    fn adc(&mut self) -> u8 {
        self.fetch_data();
        let a = self.acc;
        let (x1, o1) = self.fetched_data.overflowing_add(a);
        let (x2, o2) = x1.overflowing_add(self.get_flag(C));
        self.acc = x2;
        self.set_flag(C, o1 | o2);
        self.set_flag(
            V,
            (a ^ self.fetched_data) & 0x80 == 0 && (a ^ self.acc) & 0x80 != 0,
        );
        self.set_flags_zn(self.acc);
        return 1;
    }
    /// SBC: Subtract M from A with Carry
    fn sbc(&mut self) -> u8 {
        self.fetch_data();
        let a = self.acc;
        let (x1, o1) = a.overflowing_sub(self.fetched_data);
        let (x2, o2) = x1.overflowing_sub(1 - self.get_flag(C));
        self.acc = x2;
        self.set_flag(C, !(o1 | o2));
        self.set_flag(
            V,
            (a ^ self.fetched_data) & 0x80 != 0 && (a ^ self.acc) & 0x80 != 0,
        );
        self.set_flags_zn(self.acc);
        return 1;
    }
    /// DEC: Decrement M by One
    fn dec(&mut self) -> u8 {
        self.fetch_data();
        self.write_fetched(self.fetched_data); // dummy write
        let val = self.fetched_data.wrapping_sub(1);
        self.write_fetched(val);
        self.set_flags_zn(val);
        return 0;
    }
    /// DEX: Decrement X by One
    fn dex(&mut self) -> u8 {
        self.x = self.x.wrapping_sub(1);
        self.set_flags_zn(self.x);
        return 0;
    }
    /// DEY: Decrement Y by One
    fn dey(&mut self) -> u8 {
        self.y = self.y.wrapping_sub(1);
        self.set_flags_zn(self.y);
        return 0;
    }
    /// INC: Increment M by One
    fn inc(&mut self) -> u8 {
        self.fetch_data();
        self.write_fetched(self.fetched_data); // dummy write
        let val = self.fetched_data.wrapping_add(1);
        self.set_flags_zn(val);
        self.write_fetched(val);
        return 0;
    }
    /// INX: Increment X by One
    fn inx(&mut self) -> u8 {
        self.x = self.x.wrapping_add(1);
        self.set_flags_zn(self.x);
        return 0;
    }
    /// INY: Increment Y by One
    fn iny(&mut self) -> u8 {
        self.y = self.y.wrapping_add(1);
        self.set_flags_zn(self.y);
        return 0;
    }

    /// Bitwise opcodes

    /// AND: "And" M with A
    fn and(&mut self) -> u8 {
        self.fetch_data();
        self.acc &= self.fetched_data;
        self.set_flags_zn(self.acc);
        return 1;
    }
    /// ASL: Shift Left One Bit (M or A)
    fn asl(&mut self) -> u8 {
        self.fetch_data();
        self.write_fetched(self.fetched_data); // dummy write
        self.set_flag(C, (self.fetched_data >> 7) & 1 > 0);
        let val = self.fetched_data.wrapping_shl(1);
        self.set_flags_zn(val);
        self.write_fetched(val);
        return 0;
    }
    /// BIT: Test Bits in M with A (Affects N, V, and Z)
    fn bit(&mut self) -> u8 {
        self.fetch_data();
        let val = self.acc & self.fetched_data;
        self.set_flag(Z, val == 0);
        self.set_flag(N, self.fetched_data & (1 << 7) > 0);
        self.set_flag(V, self.fetched_data & (1 << 6) > 0);
        return 0;
    }
    /// EOR: "Exclusive-Or" M with A
    fn eor(&mut self) -> u8 {
        self.fetch_data();
        self.acc ^= self.fetched_data;
        self.set_flags_zn(self.acc);
        return 1;
    }
    /// LSR: Shift Right One Bit (M or A)
    fn lsr(&mut self) -> u8 {
        self.fetch_data();
        self.write_fetched(self.fetched_data); // dummy write
        self.set_flag(C, self.fetched_data & 1 > 0);
        let val = self.fetched_data.wrapping_shr(1);
        self.set_flags_zn(val);
        self.write_fetched(val);
        return 0;
    }
    /// ORA: "OR" M with A
    fn ora(&mut self) -> u8 {
        self.fetch_data();
        self.acc |= self.fetched_data;
        self.set_flags_zn(self.acc);
        return 1;
    }
    /// ROL: Rotate One Bit Left (M or A)
    fn rol(&mut self) -> u8 {
        self.fetch_data();
        self.write_fetched(self.fetched_data); // dummy write
        let old_c = self.get_flag(C);
        self.set_flag(C, (self.fetched_data >> 7) & 1 > 0);
        let val = (self.fetched_data << 1) | old_c;
        self.set_flags_zn(val);
        self.write_fetched(val);
        return 0;
    }
    /// ROR: Rotate One Bit Right (M or A)
    fn ror(&mut self) -> u8 {
        self.fetch_data();
        self.write_fetched(self.fetched_data); // dummy write
        let mut ret = self.fetched_data.rotate_right(1);
        if self.get_flag(C) == 1 {
            ret |= 1 << 7;
        } else {
            ret &= !(1 << 7);
        }
        self.set_flag(C, self.fetched_data & 1 > 0);
        self.set_flags_zn(ret);
        self.write_fetched(ret);
        return 0;
    }

    /// Branch opcodes

    /// Utility function used by all branch instructions
    fn branch(&mut self) {
        self.cycle_count = self.cycle_count.wrapping_add(1);
        self.abs_addr = self.pc.wrapping_add(self.rel_addr);
        if self.pages_differ(self.abs_addr, self.pc) {
            self.cycle_count = self.cycle_count.wrapping_add(1);
        }
        self.pc = self.abs_addr;
    }
    /// BCC: Branch on Carry Clear
    fn bcc(&mut self) -> u8 {
        if self.get_flag(C) == 0 {
            self.branch();
        }
        return 0;
    }
    /// BCS: Branch on Carry Set
    fn bcs(&mut self) -> u8 {
        if self.get_flag(C) == 1 {
            self.branch();
        }
        return 0;
    }
    /// BEQ: Branch on Result Zero
    fn beq(&mut self) -> u8 {
        if self.get_flag(Z) == 1 {
            self.branch();
        }
        return 0;
    }
    /// BMI: Branch on Result Negative
    fn bmi(&mut self) -> u8 {
        if self.get_flag(N) == 1 {
            self.branch();
        }
        return 0;
    }
    /// BNE: Branch on Result Not Zero
    fn bne(&mut self) -> u8 {
        if self.get_flag(Z) == 0 {
            self.branch();
        }
        return 0;
    }
    /// BPL: Branch on Result Positive
    fn bpl(&mut self) -> u8 {
        if self.get_flag(N) == 0 {
            self.branch();
        }
        return 0;
    }
    /// BVC: Branch on Overflow Clear
    fn bvc(&mut self) -> u8 {
        if self.get_flag(V) == 0 {
            self.branch();
        }
        return 0;
    }
    /// BVS: Branch on Overflow Set
    fn bvs(&mut self) -> u8 {
        if self.get_flag(V) == 1 {
            self.branch();
        }
        return 0;
    }

    /// Jump opcodes

    /// JMP: Jump to Location
    fn jmp(&mut self) -> u8 {
        self.pc = self.abs_addr;
        return 0;
    }
    /// JSR: Jump to Location Save Return addr
    fn jsr(&mut self) -> u8 {
        self.push_stackw(self.pc.wrapping_sub(1));
        self.pc = self.abs_addr;
        return 0;
    }
    /// RTI: Return from Interrupt
    fn rti(&mut self) -> u8 {
        self.status = self.pop_stackb();
        self.status &= !(U as u8);
        self.status &= !(B as u8);
        self.pc = self.pop_stackw();
        return 0;
    }
    /// RTS: Return from Subroutine
    fn rts(&mut self) -> u8 {
        self.pc = self.pop_stackw().wrapping_add(1);
        return 0;
    }

    ///  Register opcodes

    /// CLC: Clear Carry Flag
    fn clc(&mut self) -> u8 {
        self.set_flag(C, false);
        return 0;
    }
    /// SEC: Set Carry Flag
    fn sec(&mut self) -> u8 {
        self.set_flag(C, true);
        return 0;
    }
    /// CLD: Clear Decimal Mode
    fn cld(&mut self) -> u8 {
        self.set_flag(D, false);
        return 0;
    }
    /// SED: Set Decimal Mode
    fn sed(&mut self) -> u8 {
        self.set_flag(D, true);
        return 0;
    }
    /// CLI: Clear Interrupt Disable Bit
    fn cli(&mut self) -> u8 {
        self.set_flag(I, false);
        return 0;
    }
    /// SEI: Set Interrupt Disable Status
    fn sei(&mut self) -> u8 {
        self.set_flag(I, true);
        return 0;
    }
    /// CLV: Clear Overflow Flag
    fn clv(&mut self) -> u8 {
        self.set_flag(V, false);
        return 0;
    }

    /// Compare opcodes

    /// Utility function used by all compare instructions
    fn compare(&mut self, a: u8, b: u8) {
        let result = a.wrapping_sub(b);
        self.set_flags_zn(result);
        self.set_flag(C, a >= b);
    }
    /// CMP: Compare M and A
    fn cmp(&mut self) -> u8 {
        self.fetch_data();
        self.compare(self.acc, self.fetched_data);
        return 1;
    }
    /// CPX: Compare M and X
    fn cpx(&mut self) -> u8 {
        self.fetch_data();
        self.compare(self.x, self.fetched_data);
        return 0;
    }
    /// CPY: Compare M and Y
    fn cpy(&mut self) -> u8 {
        self.fetch_data();
        self.compare(self.y, self.fetched_data);
        return 0;
    }

    /// Stack opcodes

    /// PHP: Push Processor Status on Stack
    fn php(&mut self) -> u8 {
        self.push_stackb(self.status | U as u8 | B as u8);
        self.set_flag(B, false);
        self.set_flag(U, false);
        return 0;
    }
    /// PLP: Pull Processor Status from Stack
    fn plp(&mut self) -> u8 {
        self.status = (self.pop_stackb() | U as u8) & !(B as u8);
        return 0;
    }
    /// PHA: Push A on Stack
    fn pha(&mut self) -> u8 {
        self.push_stackb(self.acc);
        return 0;
    }
    /// PLA: Pull A from Stack
    fn pla(&mut self) -> u8 {
        self.acc = self.pop_stackb();
        self.set_flags_zn(self.acc);
        return 0;
    }

    /// System opcodes

    /// BRK: Force Break Interrupt
    fn brk(&mut self) -> u8 {
        self.set_flag(I, true);
        self.push_stackw(self.pc.wrapping_add(1));
        self.set_flag(B, true);
        self.php();
        self.pc = self.readw(IRQ_ADDR);
        return 0;
    }
    /// NOP: No Operation
    fn nop(&mut self) -> u8 {
        return 0;
    }

    /// Unofficial opcodes

    /// SKB: Like NOP but issues a fetch
    fn skb(&mut self) -> u8 {
        self.fetch_data();
        return 0;
    }

    /// IGN: Like NOP but issues a fetch
    fn ign(&mut self) -> u8 {
        self.fetch_data();
        // Certain NOP instructions can take an extra cycle
        return match self.instr.opcode() {
            0x1C | 0x3C | 0x5C | 0x7C | 0xDC | 0xFC => 1,
            _ => 0,
        };
    }

    /// XXX: Captures all unimplemented opcodes
    fn xxx(&mut self) -> u8 {
        panic!(
            "Invalid opcode ${:02X} {{{:?}}} encountered!",
            self.instr.opcode(),
            self.instr.addr_mode()
        );
    }
    /// ISC/ISB: Shortcut for INC then SBC
    fn isb(&mut self) -> u8 {
        self.inc();
        self.sbc();
        return 0;
    }
    /// DCP: Shortcut for DEC then CMP
    fn dcp(&mut self) -> u8 {
        self.dec();
        self.cmp();
        return 0;
    }
    /// AXS: A & X into X
    fn axs(&mut self) -> u8 {
        self.fetch_data();
        self.set_flag(C, self.x <= self.fetched_data);
        self.x = (self.acc & self.x).wrapping_sub(self.fetched_data);
        self.set_flags_zn(self.x);
        return 0;
    }
    /// LAS: Shortcut for LDA then TSX
    fn las(&mut self) -> u8 {
        self.lda();
        self.tsx();
        return 1;
    }
    /// LAX: Shortcut for LDA then TAX
    fn lax(&mut self) -> u8 {
        self.lda();
        self.tax();
        return 1;
    }
    /// AHX: TODO
    fn ahx(&mut self) -> u8 {
        eprintln!("ahx not implemented");
        return 0;
    }
    /// SAX: AND A with X
    fn sax(&mut self) -> u8 {
        let val = self.acc & self.x;
        self.write_fetched(val);
        return 0;
    }
    /// XAA: TODO
    fn xaa(&mut self) -> u8 {
        eprintln!("xaa not implemented");
        return 0;
    }
    /// SHX: TODO
    fn shx(&mut self) -> u8 {
        eprintln!("shx not implemented");
        return 0;
    }
    /// RRA: Shortcut for ROR then ADC
    fn rra(&mut self) -> u8 {
        self.ror();
        self.adc();
        return 0;
    }
    /// TAS: Shortcut for STA then TXS
    fn tas(&mut self) -> u8 {
        self.sta();
        self.txs();
        return 0;
    }
    /// SHY: TODO
    fn shy(&mut self) -> u8 {
        eprintln!("shy not implemented");
        return 0;
    }
    /// ARR: Shortcut for AND #imm then ROR, but sets flags differently
    /// C is bit 6 and V is bit 6 xor bit 5
    /// TODO doesn't pass tests
    fn arr(&mut self) -> u8 {
        self.and();
        self.ror();
        return 0;
    }
    /// SRA: Shortcut for LSR then EOR
    fn sre(&mut self) -> u8 {
        self.lsr();
        self.eor();
        return 0;
    }
    /// ALR/ASR: Shortcut for AND #imm then LSR
    /// TODO doesn't pass tests
    fn alr(&mut self) -> u8 {
        self.and();
        self.lsr();
        return 0;
    }
    /// RLA: Shortcut for ROL then AND
    fn rla(&mut self) -> u8 {
        self.rol();
        self.and();
        return 0;
    }
    /// ANC/AAC: AND #imm but puts bit 7 into carry as if ASL was executed
    fn anc(&mut self) -> u8 {
        let ret = self.and();
        self.set_flag(C, (self.acc >> 7) & 1 > 0);
        return ret;
    }
    /// SLO: Shortcut for ASL then ORA
    fn slo(&mut self) -> u8 {
        self.asl();
        self.ora();
        return 0;
    }
}

impl<M> fmt::Debug for Cpu<M>
where
    M: Memory,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), fmt::Error> {
        write!(
            f,
            "Cpu {{ {:04X} A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X} CYC:{} rel_addr:{} }}",
            self.pc,
            self.acc,
            self.x,
            self.y,
            self.status,
            self.sp,
            self.cycle_count,
            self.rel_addr
        )
    }
}
impl fmt::Debug for Instr {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), fmt::Error> {
        let mut op = self.op();
        let unofficial = match self.op() {
            XXX | ISB | DCP | AXS | LAS | LAX | AHX | SAX | XAA | SHX | RRA | TAS | SHY | ARR
            | SRE | ALR | RLA | ANC | SLO => "*",
            NOP if self.opcode() != 0xEA => "*", // 0xEA is the only official NOP
            SKB | IGN => {
                op = NOP;
                "*"
            }
            SBC if self.opcode() == 0xEB => "*",
            _ => "",
        };
        write!(f, "{:1}{:?}", unofficial, op)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;
    use crate::mapper;
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    const TEST_ROM: &str = "tests/cpu/nestest.nes";
    const TEST_PC: u16 = 49156;

    #[test]
    fn test_cpu_new() {
        let rom = PathBuf::from(TEST_ROM);
        let mapper = mapper::load_rom(rom).expect("loaded mapper");
        let input = Rc::new(RefCell::new(Input::new()));
        let mut cpu_memory = MemoryMap::init(input);
        cpu_memory.load_mapper(mapper);
        let c = Cpu::init(cpu_memory);
        assert_eq!(c.cycle_count, 7);
        assert_eq!(c.pc, TEST_PC);
        assert_eq!(c.sp, POWER_ON_SP);
        assert_eq!(c.acc, 0);
        assert_eq!(c.x, 0);
        assert_eq!(c.y, 0);
        assert_eq!(c.status, POWER_ON_STATUS);
    }

    #[test]
    fn test_cpu_reset() {
        let rom = PathBuf::from(TEST_ROM);
        let mapper = mapper::load_rom(rom).expect("loaded mapper");
        let input = Rc::new(RefCell::new(Input::new()));
        let mut cpu_memory = MemoryMap::init(input);
        cpu_memory.load_mapper(mapper);
        let mut c = Cpu::init(cpu_memory);
        c.reset();
        assert_eq!(c.pc, TEST_PC);
        assert_eq!(c.sp, POWER_ON_SP - 3);
        assert_eq!(c.status, POWER_ON_STATUS);
        assert_eq!(c.cycle_count, 7);
    }
}
