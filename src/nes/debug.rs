use crate::{
    cpu::StatusRegs,
    mapper::Mapper,
    memory::MemRead,
    nes::{Mode, Nes, View},
    ppu::{vram::NT_START, RENDER_HEIGHT, RENDER_WIDTH},
};
use pix_engine::prelude::*;

const PALETTE_HEIGHT: u32 = 64;

impl Nes {
    pub(crate) fn toggle_cpu_debugger(&mut self, s: &mut PixState) -> PixResult<()> {
        match self.cpu_debugger {
            None => {
                let (w, h) = s.dimensions()?;
                let window_id = s
                    .window()
                    .with_dimensions(w, h)
                    .with_title("CPU Debugger")
                    .position(10, 10)
                    .resizable()
                    .build()?;
                self.cpu_debugger = Some(View::new(window_id, None));
                self.mode = Mode::Debugging;
            }
            Some(debugger) => {
                s.close_window(debugger.window_id)?;
                self.cpu_debugger = None;
                if self.control_deck.is_running() {
                    self.mode = Mode::Playing;
                } else {
                    self.mode = Mode::Paused;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn render_cpu_debugger(&mut self, s: &mut PixState) -> PixResult<()> {
        if let Some(view) = self.cpu_debugger {
            s.with_window(view.window_id, |s: &mut PixState| {
                s.clear()?;
                s.no_stroke();

                {
                    let cpu = self.control_deck.cpu();

                    s.text("Status: ")?;
                    use StatusRegs::{B, C, D, I, N, U, V};
                    s.push();
                    for status in &[N, V, U, B, D, I, C] {
                        s.same_line(None);
                        s.fill(if cpu.status & *status as u8 > 0 {
                            Color::RED
                        } else {
                            Color::GREEN
                        });
                        s.text(&format!("{:?}", status))?;
                    }
                    s.pop();

                    s.text(&format!("Cycles: {:8}", cpu.cycle_count))?;
                    // TODO: Total running time

                    s.spacing()?;
                    s.text(&format!(
                        "PC: ${:04X}           A: ${:02X} [{:03}]",
                        cpu.pc, cpu.acc, cpu.acc
                    ))?;
                    s.text(&format!(
                        "X:  ${:02X} [{:03}]   Y: ${:02X} [{:03}]",
                        cpu.x, cpu.x, cpu.y, cpu.y
                    ))?;

                    s.spacing()?;
                    s.text(&format!("Stack: $01{:02X}", cpu.sp))?;
                    let bytes_per_row = 8;
                    for (i, offset) in (0xE0..=0xFF).rev().enumerate() {
                        let val = cpu.peek(0x0100 | offset);
                        s.text(&format!("{:02X} ", val))?;
                        if i % bytes_per_row < bytes_per_row - 1 {
                            s.same_line(None);
                        }
                    }
                }

                {
                    let ppu = self.control_deck.ppu();

                    s.text(&format!("VRAM Addr: ${:04X}", ppu.read_ppuaddr()))?;
                    s.text(&format!("OAM Addr:  ${:02X}", ppu.read_oamaddr()))?;
                    s.text(&format!(
                        "PPU Cycle: {:3}  Scanline: {:3}",
                        ppu.cycle,
                        i32::from(ppu.scanline) - 1
                    ))?;

                    s.spacing()?;
                    let m = s.mouse_pos() / self.config.scale as i32;
                    let mx = (m.x() as f32 * 7.0 / 8.0) as u32;
                    s.text(&format!("Mouse: {:3}, {:3}", mx, m.y()))?;
                }

                s.spacing()?;
                let disasm = self
                    .control_deck
                    .disasm(self.control_deck.pc(), self.control_deck.pc() + 20);
                for instr in &disasm {
                    s.text(&instr)?;
                }

                Ok(())
            })?;
        }
        Ok(())
    }

    pub(crate) fn toggle_ppu_debugger(&mut self, s: &mut PixState) -> PixResult<()> {
        match self.ppu_debugger {
            None => {
                let w = 4 * RENDER_WIDTH;
                let h = 3 * RENDER_HEIGHT;
                let window_id = s
                    .window()
                    .with_dimensions(w, h)
                    .with_title("PPU Debugger")
                    .position(10, 10)
                    .resizable()
                    .build()?;
                s.with_window(window_id, |s: &mut PixState| {
                    let texture_id = s.create_texture(w, h, PixelFormat::Rgba)?;
                    self.ppu_debugger = Some(View::new(window_id, Some(texture_id)));
                    Ok(())
                })?;
                self.control_deck.ppu_mut().update_debug();
                self.control_deck.ppu_mut().set_debugging(true);
            }
            Some(debugger) => {
                s.close_window(debugger.window_id)?;
                self.ppu_debugger = None;
                self.control_deck.ppu_mut().set_debugging(false);
            }
        }
        Ok(())
    }

    pub(crate) fn render_ppu_debugger(&mut self, s: &mut PixState) -> PixResult<()> {
        if let Some(view) = self.ppu_debugger {
            if let Some(texture_id) = view.texture_id {
                s.with_window(view.window_id, |s: &mut PixState| {
                    s.clear()?;

                    let width = RENDER_WIDTH as i32;
                    let height = RENDER_HEIGHT as i32;
                    let m = s.mouse_pos();

                    // Nametables

                    let nametables = &self.control_deck.ppu().nametables;
                    let nametable1 = rect![0, 0, width, height];
                    let nametable2 = rect![width, 0, width, height];
                    let nametable3 = rect![0, height, width, height];
                    let nametable4 = rect![width, height, width, height];
                    let nametable_src = rect![0, 0, 2 * width, 2 * height];
                    let nametable_pitch = 4 * width as usize;

                    s.update_texture(texture_id, nametable1, &nametables[0], nametable_pitch)?;
                    s.update_texture(texture_id, nametable2, &nametables[1], nametable_pitch)?;
                    s.update_texture(texture_id, nametable3, &nametables[2], nametable_pitch)?;
                    s.update_texture(texture_id, nametable4, &nametables[3], nametable_pitch)?;
                    s.texture(texture_id, nametable_src, nametable_src)?;

                    // Scanline
                    let scanline = self.scanline as i32;
                    s.push();
                    s.stroke(Color::WHITE);
                    s.stroke_weight(2);
                    s.line([0, scanline, 2 * width, scanline])?;
                    s.line([0, scanline + height, 2 * width, scanline + height])?;
                    s.pop();

                    // Nametable Info

                    s.set_cursor_pos([s.cursor_pos().x(), nametable3.bottom() + 4]);

                    s.text(&format!("Scanline: {}", self.scanline))?;
                    let mirroring = self.control_deck.mapper().mirroring();
                    s.text(&format!("Mirroring: {:?}", mirroring))?;

                    if rect![0, 0, 2 * width, 2 * height].contains_point(m) {
                        let nt_addr =
                            NT_START as i32 + (m.x() / width) * 0x0400 + (m.y() / height) * 0x0800;
                        let ppu_addr = nt_addr + ((((m.y() / 8) % 30) << 5) | ((m.x() / 8) % 32));
                        let tile_id = self
                            .control_deck
                            .ppu()
                            .nametable_ids
                            .get((ppu_addr - NT_START as i32) as usize)
                            .unwrap_or(&0x00);
                        s.text(&format!("Tile ID: ${:02X}", tile_id))?;
                        s.text(&format!("(X, Y): ({}, {})", m.x() % width, m.y() % height))?;
                        s.text(&format!("PPU Addr: ${:04X}", ppu_addr))?;
                    } else {
                        s.text("Tile ID: $00")?;
                        s.text("(X, Y): (0, 0)")?;
                        s.text("PPU Addr: $0000")?;
                    }

                    // Pattern Tables

                    let patterns = &self.control_deck.ppu().pattern_tables;
                    let pattern_x = nametable_src.right() + 8;
                    let pattern_w = width / 2;
                    let pattern_h = height / 2;
                    let pattern_left = rect![pattern_x, 0, pattern_w, pattern_h];
                    let pattern_right = rect![pattern_x + pattern_w, 0, pattern_w, pattern_h];
                    let pattern_src = rect![pattern_x, 0, 2 * pattern_w, pattern_h];
                    let pattern_dst = rect![pattern_x, 0, 2 * width, height];
                    let pattern_pitch = 4 * pattern_w as usize;
                    s.update_texture(texture_id, pattern_left, &patterns[0], pattern_pitch)?;
                    s.update_texture(texture_id, pattern_right, &patterns[1], pattern_pitch)?;
                    s.texture(texture_id, pattern_src, pattern_dst)?;

                    // Palette

                    let palette = &self.control_deck.ppu().palette;
                    let palette_w = 16;
                    let palette_h = 2;
                    let palette_src = rect![0, pattern_src.bottom(), palette_w, palette_h];
                    let palette_dst = rect![
                        pattern_x,
                        pattern_dst.bottom(),
                        2 * width,
                        PALETTE_HEIGHT as i32
                    ];
                    let palette_pitch = 4 * palette_w as usize;
                    s.update_texture(texture_id, palette_src, &palette, palette_pitch)?;
                    s.texture(texture_id, palette_src, palette_dst)?;

                    // Borders

                    s.push();

                    s.stroke(Color::DIM_GRAY);
                    s.no_fill();
                    s.stroke_weight(2);

                    s.rect(nametable1)?;
                    s.rect(nametable2)?;
                    s.rect(nametable3)?;
                    s.rect(nametable4)?;
                    s.rect(pattern_dst)?;
                    s.line([
                        pattern_dst.center().x(),
                        pattern_dst.top(),
                        pattern_dst.center().x(),
                        pattern_dst.bottom(),
                    ])?;

                    s.pop();

                    // PPU Address Info

                    s.set_cursor_pos([s.cursor_pos().x(), palette_dst.bottom() + 4]);
                    s.set_column_offset(pattern_x);

                    if pattern_dst.contains_point(m) {
                        let tile = (m.y() / 16) << 4 | ((m.x() / 16) % 16);
                        s.text(&format!("Tile: ${:02X}", tile))?;
                    } else {
                        s.text("Tile: $00")?;
                    }

                    if palette_dst.contains_point(m) {
                        let py = m.y().saturating_sub(height + 2) / 32;
                        let px = m.x() / 32;
                        let palette = self
                            .control_deck
                            .ppu()
                            .palette_ids
                            .get((py * 16 + px) as usize)
                            .unwrap_or(&0x00);
                        s.text(&format!("Palette: ${:02X}", palette))?;
                    } else {
                        s.text("Palette: $00")?;
                    }

                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    pub(crate) fn toggle_apu_debugger(&mut self, s: &mut PixState) -> PixResult<()> {
        match self.apu_debugger {
            None => {
                // let window_id = s
                //     .window()
                //     .with_dimensions(w, h)
                //     .with_title("APU Debugger")
                //     .position(10, 10)
                //     .build()?;
                // self.apu_debugger = Some(View::new(window_id, Some(texture_id)));
            }
            Some(debugger) => {
                s.close_window(debugger.window_id)?;
                self.apu_debugger = None;
            }
        }
        Ok(())
    }
}
