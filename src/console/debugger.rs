use crate::console::cpu::{Cpu, Interrupt};

pub struct Debugger {
    enabled: bool,         // Whether debugger is enabled at all or not
    tracing: bool,         // Whether we want to print each CPU instruction
    breakpoint: u64,       // A specific CPU instruction step to break at
    current_step: u64,     // Current CPU instruction we're at
    steps: u64,            // Number of CPU instructions to step through
    break_type: BreakType, // Type of breakpoint
    run_last: bool,
    last_cmd: String,
}
#[derive(PartialEq, Eq, Debug)]
enum BreakType {
    Unset,
    Step,
    NMI,
    IRQ,
}
use BreakType::*;

impl Debugger {
    const B_USAGE: &'static str = "b <step>  Set a breakpoint on a given CPU step";
    const S_USAGE: &'static str = "s [steps] Step CPU [steps] (defaults to 1)";
    const P_USAGE: &'static str = "p [obj]   Print debug output of an object in memory.
           Options for obj:
               cpu      : Top-level details of the CPU status
               cpu_mem  : HEX output of memory sorted by memory map
               ppu      : Top-level details of the PPU status
               ppu_vram : HEX output of VRAM memory sorted by memory map
               apu      : Top-level details of the APU status
               cart     : Top-level details of the cartridge information
               cart_prg : HEX output of cartridge PRG-ROM and PRG-RAM
               cart_chr : HEX output of cartridge CHR-ROM and CHR-RAM";

    pub fn new() -> Self {
        Self {
            enabled: false,
            tracing: true,
            breakpoint: 0u64,
            current_step: 0u64,
            steps: 0u64,
            break_type: Unset,
            run_last: false,
            last_cmd: String::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn start(&mut self) {
        self.enabled = true;
        self.steps = 1;
        self.break_type = Step;
    }

    pub fn stop(&mut self) {
        self.enabled = false;
        self.steps = 0;
        self.break_type = Unset;
    }

    pub fn on_step(&mut self, cpu: &mut Cpu, opcode: u8, num_args: u8, disasm: String) {
        if self.tracing && (self.break_type == Step || cpu.interrupt != Interrupt::None) {
            cpu.print_instruction(opcode, num_args, disasm);
        }
        self.current_step = cpu.step;
        if self.enabled && self.break_type == Step {
            if self.steps > 0 {
                self.steps -= 1;
                if self.steps == 0 {
                    self.prompt(cpu);
                }
                return;
            } else if self.breakpoint > 0 && self.breakpoint == self.current_step {
                self.prompt(cpu);
                self.breakpoint = 0;
            }
        }
    }

    pub fn on_nmi(&mut self, cpu: &Cpu) {
        self.current_step = cpu.step;
        if self.enabled && self.break_type == NMI {
            eprintln!("DEBUG - VBLANK");
            self.prompt(cpu);
        }
    }

    pub fn on_irq(&mut self, cpu: &Cpu) {
        self.current_step = cpu.step;
        if self.enabled && self.break_type == IRQ {
            eprintln!("DEBUG - SCANLINE");
            self.prompt(cpu);
        }
    }

    fn prompt(&mut self, cpu: &Cpu) {
        loop {
            eprint!("debugger (step: {}) > ", self.current_step);
            let mut input = String::new();
            match std::io::stdin().read_line(&mut input) {
                Ok(bytes) => {
                    match input.trim() {
                        "" => {
                            // Ctrl-D was pressed
                            if bytes == 0 {
                                self.enabled = false;
                            }
                            // Enter was pressed - use last command TODO
                        }
                        "h" => self.usage(),
                        "q" => {
                            self.enabled = false;
                            break;
                        }
                        "c" => {
                            if self.breakpoint == 0 {
                                self.break_type = Unset;
                                self.enabled = false;
                            }
                            break;
                        }
                        "nmi" => {
                            self.break_type = NMI;
                            break;
                        }
                        "irq" => {
                            self.break_type = IRQ;
                            break;
                        }
                        cmd => match cmd.chars().next().unwrap() {
                            'b' => {
                                self.break_type = Step;
                                self.set_breakpoint(cmd);
                            }
                            'c' => {
                                self.break_type = Step;
                                self.set_breakpoint(cmd);
                                break;
                            }
                            's' => {
                                self.break_type = Step;
                                self.set_steps(cmd);
                                break;
                            }
                            'p' => {
                                self.print_obj(cpu, cmd);
                            }
                            _ => {
                                eprintln!("unknown command {:?}", cmd);
                            }
                        },
                    }
                }
                Err(x) => eprintln!("error reading input: {}", x),
            }
        }
    }

    fn usage(&mut self) {
        eprintln!(
            "List of commands:
    h         This help
    q         Disable debugger
    {}
    {}
    c [step]  Continue CPU execution until [step] or the next breakpoint (if any)
    {}
    nmi       Step until the next NMI (Vertical Blank)
    irq       Step until the next IRQ (Horizontal Blank/Scanline)
    <Enter>   Repeat the last command
",
            Self::B_USAGE,
            Self::S_USAGE,
            Self::P_USAGE,
        );
    }

    fn set_breakpoint(&mut self, cmd: &str) {
        let bp = self.extract_num(cmd);
        if let Ok(bp) = bp {
            self.breakpoint = bp;
        } else {
            eprintln!("{}", Self::B_USAGE);
        }
    }

    fn set_steps(&mut self, cmd: &str) {
        let steps = self.extract_num(cmd);
        if let Ok(steps) = steps {
            self.steps = steps;
        } else {
            eprintln!("{}", Self::S_USAGE);
        }
    }

    fn extract_num(&mut self, cmd: &str) -> Result<u64, std::num::ParseIntError> {
        if cmd.len() > 2 {
            let (_, num) = cmd.split_at(2);
            num.parse::<u64>()
        } else {
            Ok(1)
        }
    }

    fn print_obj(&mut self, cpu: &Cpu, cmd: &str) {
        if cmd.len() > 2 {
            let (_, obj) = cmd.split_at(2);
            match obj {
                "cpu" => eprintln!("not implemented yet"),
                "cpu_mem" => eprintln!("not implemented yet"),
                "ppu" => eprintln!("not implemented yet"),
                "ppu_vram" => {
                    Self::hexdump(&cpu.mem.ppu.vram.nametable.0);
                }
                "apu" => eprintln!("not implemented yet"),
                "cart" => eprintln!("{:?}", cpu.mem.mapper),
                "cart_prg" => eprintln!("not implemented yet"),
                "cart_chr" => eprintln!("not implemented yet"),
                _ => {
                    eprintln!("invalid obj: {:?}", obj);
                }
            }
        } else {
            eprintln!("{}", Self::P_USAGE);
        }
    }

    fn hexdump(data: &[u8]) {
        use std::cmp;

        let mut addr = 0;
        let len = data.len();
        let mut last_line_same = false;
        let mut last_line = String::with_capacity(80);
        while addr <= len {
            let end = cmp::min(addr + 16, len);
            let line_data = &data[addr..end];
            let line_len = line_data.len();

            let mut line = String::with_capacity(80);
            for byte in 0..line_len {
                line.push_str(&format!(" {:02X}", line_data[byte]));
            }

            if line_len % 16 > 0 {
                let words_left = (16 - line_len) / 2;
                for _ in 0..3 * words_left {
                    line.push_str(" ");
                }
            }

            if line_len > 0 {
                line.push_str("  |");
                for c in line_data {
                    if (*c as char).is_ascii() && !(*c as char).is_control() {
                        line.push_str(&format!("{}", (*c as char)));
                    } else {
                        line.push_str(".");
                    }
                }
                line.push_str("|");
            }
            if last_line == line {
                if last_line_same == false {
                    last_line_same = true;
                    eprintln!("*");
                }
            } else {
                last_line_same = false;
                eprintln!("{:08x} {}", addr, line);
            }
            last_line = line;

            addr += 16;
        }
    }
}

impl Default for Debugger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn test_hexdump() {
        let rom = std::path::PathBuf::from("roms/legend_of_zelda.nes");
        let mut rom_file = std::fs::File::open(&rom).expect("valid file");
        let mut data = Vec::new();
        rom_file.read_to_end(&mut data).expect("read data");
        Debugger::hexdump(&data);
    }
}
