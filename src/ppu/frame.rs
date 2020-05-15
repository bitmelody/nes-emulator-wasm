use super::{Sprite, RENDER_HEIGHT, RENDER_SIZE, RENDER_WIDTH};
use crate::{common::Powered, serialization::Savable, NesResult};
use std::{
    f32::consts::PI,
    io::{Read, Write},
};

#[derive(Clone)]
pub(super) struct Frame {
    num: u32,
    pub(super) parity: bool,
    // Shift registers
    pub(super) tile_lo: u8,
    pub(super) tile_hi: u8,
    // Tile data - stored in cycles 0 mod 8
    pub(super) nametable: u16,
    pub(super) attribute: u8,
    pub(super) tile_data: u64,
    // Sprite data
    pub(super) sprite_count: u8,
    pub(super) sprite_zero_on_line: bool,
    pub(super) sprites: [Sprite; 8], // Each frame can only hold 8 sprites at a time
    prev_pixel: u32,
    palette: Vec<Vec<Vec<u32>>>,
    pub(super) pixels: Vec<u8>,
}

impl Frame {
    pub(super) fn new() -> Self {
        let mut frame = Self {
            num: 0,
            parity: false,
            nametable: 0,
            attribute: 0,
            tile_lo: 0,
            tile_hi: 0,
            tile_data: 0,
            sprite_count: 0,
            sprite_zero_on_line: false,
            sprites: [Sprite::new(); 8],
            prev_pixel: 0xFFFF,
            palette: vec![vec![vec![0; 512]; 64]; 3],
            pixels: vec![0; RENDER_SIZE],
        };
        frame.generate_ntsc_palette();
        frame
    }

    pub(super) fn increment(&mut self) {
        self.num += 1;
        self.parity = !self.parity;
    }

    pub(super) fn put_pixel(&mut self, x: u32, y: u32, red: u8, green: u8, blue: u8) {
        if x >= RENDER_WIDTH || y >= RENDER_HEIGHT {
            return;
        }
        let idx = 4 * (x + y * RENDER_WIDTH) as usize;
        self.pixels[idx] = red;
        self.pixels[idx + 1] = green;
        self.pixels[idx + 2] = blue;
        self.pixels[idx + 3] = 255;
    }

    // Amazing implementation Bisqwit! Much faster than my original, but boy what a pain
    // to translate it to Rust
    // Source: https://bisqwit.iki.fi/jutut/kuvat/programming_examples/nesemu1/nesemu1.cc
    // http://wiki.nesdev.com/w/index.php/NTSC_video
    //
    // Note: Because blending relies on previous x pixel, we shift everything to the
    // left and render an extra pixel column on the right
    pub(super) fn put_ntsc_pixel(&mut self, x: u32, y: u32, mut pixel: u32, ppu_cycle: u32) {
        if x > RENDER_WIDTH || y >= RENDER_HEIGHT {
            return;
        }
        if x == RENDER_WIDTH {
            pixel = self.prev_pixel;
        }
        let color =
            self.palette[ppu_cycle as usize][(self.prev_pixel % 64) as usize][pixel as usize];
        self.prev_pixel = pixel;
        let red = (color >> 16 & 0xFF) as u8;
        let green = (color >> 8 & 0xFF) as u8;
        let blue = (color & 0xFF) as u8;
        self.put_pixel(x.saturating_sub(1), y, red, green, blue);
    }

    // NOTE: There's lot's to clean up here -- too many magic numbers and duplication but
    // I'm afraid to touch it now that it works
    // Source: https://bisqwit.iki.fi/jutut/kuvat/programming_examples/nesemu1/nesemu1.cc
    // http://wiki.nesdev.com/w/index.php/NTSC_video
    fn generate_ntsc_palette(&mut self) {
        // Calculate the luma and chroma by emulating the relevant circuits:
        const VOLTAGES: [i32; 16] = [
            -6, -69, 26, -59, 29, -55, 73, -40, 68, -17, 125, 11, 68, 33, 125, 78,
        ];
        // Helper functions for converting YIQ to RGB
        let gammafix = |color: f32| {
            if color < 0.0 {
                0.0
            } else {
                color.powf(2.2 / 1.8)
            }
        };
        let clamp = |color| {
            if color > 255.0 {
                255
            } else {
                color as u32
            }
        };
        let yiq_divider = (9 * 10u32.pow(6)) as f32;
        for palette_offset in 0..3 {
            for channel in 0..3 {
                for color0 in 0..512 {
                    let emphasis = color0 / 64;

                    for color1 in 0..64usize {
                        let mut y = 0;
                        let mut i = 0;
                        let mut q = 0;
                        // 12 samples of NTSC signal constitute a color.
                        for sample in 0..12 {
                            let noise = (sample + palette_offset * 4) % 12;
                            // Sample either the previous or the current pixel.
                            // Use pixel=color0 to disable artifacts.
                            let pixel = if noise < 5 - channel * 2 {
                                color0
                            } else {
                                color1
                            };

                            // Decode the color index.
                            let chroma = pixel % 16;
                            let luma = if chroma < 0xE { (pixel / 4) & 12 } else { 4 }; // Forces luma to 0, 4, 8, or 12 for easy lookup
                                                                                        // NES NTSC modulator (square wave between up to four voltage levels):
                            let limit = if (chroma + 8 + sample) % 12 < 6 {
                                12
                            } else {
                                0
                            };
                            let high = if chroma > limit { 1 } else { 0 };
                            // TODO: This doesn't quite work yet - green is swapped with blue
                            // and blue emphasis is more of a darker gray
                            let emp_effect = if (152_278 >> (sample / 2 * 3)) & emphasis > 0 {
                                0
                            } else {
                                2
                            };
                            let level = 40 + VOLTAGES[(high + emp_effect + luma) as usize];
                            // Ideal TV NTSC demodulator:
                            let (sin, cos) = (PI * sample as f32 / 6.0).sin_cos();
                            y += level;
                            i += level * (cos * 5909.0) as i32;
                            q += level * (sin * 5909.0) as i32;
                        }
                        // Store color at subpixel precision
                        let y = y as f32 / 1980.0;
                        let i = i as f32;
                        let q = q as f32;
                        match channel {
                            2 => {
                                let rgb = y + i * 0.947 / yiq_divider + q * 0.624 / yiq_divider;
                                self.palette[palette_offset][color1][color0] +=
                                    0x10000 * clamp(255.0 * gammafix(rgb));
                            }
                            1 => {
                                let rgb = y + i * -0.275 / yiq_divider + q * -0.636 / yiq_divider;
                                self.palette[palette_offset][color1][color0] +=
                                    0x00100 * clamp(255.0 * gammafix(rgb));
                            }
                            0 => {
                                let rgb = y + i * -1.109 / yiq_divider + q * 1.709 / yiq_divider;
                                self.palette[palette_offset][color1][color0] +=
                                    clamp(255.0 * gammafix(rgb));
                            }
                            _ => (), // invalid channel
                        }
                    }
                }
            }
        }
    }
}

impl Powered for Frame {
    fn reset(&mut self) {
        self.num = 0;
        self.parity = false;
    }
    fn power_cycle(&mut self) {
        self.reset();
    }
}

impl Savable for Frame {
    fn save<F: Write>(&self, fh: &mut F) -> NesResult<()> {
        self.num.save(fh)?;
        self.parity.save(fh)?;
        self.tile_lo.save(fh)?;
        self.tile_hi.save(fh)?;
        self.nametable.save(fh)?;
        self.attribute.save(fh)?;
        self.tile_data.save(fh)?;
        self.sprite_count.save(fh)?;
        self.sprite_zero_on_line.save(fh)?;
        self.sprites.save(fh)?;
        self.prev_pixel.save(fh)?;
        self.pixels.save(fh)?;
        // Ignore palette
        Ok(())
    }
    fn load<F: Read>(&mut self, fh: &mut F) -> NesResult<()> {
        self.num.load(fh)?;
        self.parity.load(fh)?;
        self.tile_lo.load(fh)?;
        self.tile_hi.load(fh)?;
        self.nametable.load(fh)?;
        self.attribute.load(fh)?;
        self.tile_data.load(fh)?;
        self.sprite_count.load(fh)?;
        self.sprite_zero_on_line.load(fh)?;
        self.sprites.load(fh)?;
        self.prev_pixel.load(fh)?;
        self.pixels.load(fh)?;
        Ok(())
    }
}
