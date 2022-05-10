use enum_dispatch::enum_dispatch;
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};

pub const CONFIG_DIR: &str = ".config/tetanes";
pub const SAVE_DIR: &str = "save";
pub const SRAM_DIR: &str = "sram";

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NesFormat {
    Ntsc,
    Pal,
    Dendy,
}

impl Default for NesFormat {
    fn default() -> Self {
        Self::Ntsc
    }
}

impl AsRef<str> for NesFormat {
    fn as_ref(&self) -> &str {
        match self {
            Self::Ntsc => "NTSC",
            Self::Pal => "PAL",
            Self::Dendy => "Dendy",
        }
    }
}

impl From<usize> for NesFormat {
    fn from(value: usize) -> Self {
        match value {
            1 => Self::Pal,
            2 => Self::Dendy,
            _ => Self::Ntsc,
        }
    }
}

#[enum_dispatch(Mapper)]
pub trait Powered {
    fn power_on(&mut self) {}
    fn power_off(&mut self) {}
    fn reset(&mut self) {}
    fn power_cycle(&mut self) {
        self.reset();
    }
}

#[enum_dispatch(Mapper)]
pub trait Clocked {
    fn clock(&mut self) -> usize {
        0
    }
}

#[macro_export]
macro_rules! hashmap {
    { $($key:expr => $value:expr),* $(,)? } => {{
        let mut m = ::std::collections::HashMap::new();
        $(
            m.insert($key, $value);
        )*
        m
    }};
    ($hm:ident, { $($key:expr => $value:expr),* $(,)? } ) => ({
        $(
            $hm.insert($key, $value);
        )*
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("./"))
        .join(CONFIG_DIR)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn config_path<P: AsRef<Path>>(path: P) -> PathBuf {
    config_dir().join(path)
}

pub fn hexdump(data: &[u8], addr_offset: usize) {
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
        for byte in line_data.iter() {
            line.push_str(&format!(" {:02X}", byte));
        }

        if line_len % 16 > 0 {
            let words_left = (16 - line_len) / 2;
            for _ in 0..3 * words_left {
                line.push(' ');
            }
        }

        if line_len > 0 {
            line.push_str("  |");
            for c in line_data {
                if (*c as char).is_ascii() && !(*c as char).is_control() {
                    line.push_str(&format!("{}", (*c as char)));
                } else {
                    line.push('.');
                }
            }
            line.push('|');
        }
        if last_line == line {
            if !last_line_same {
                last_line_same = true;
                println!("*");
            }
        } else {
            last_line_same = false;
            println!("{:08x} {}", addr + addr_offset, line);
        }
        last_line = line;

        addr += 16;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::{
        control_deck::ControlDeck,
        input::GamepadSlot,
        ppu::{VideoFilter, RENDER_HEIGHT, RENDER_WIDTH},
    };
    use pix_engine::prelude::{Image, PixelFormat};
    use std::{
        collections::hash_map::DefaultHasher,
        fs::{self, File},
        hash::{Hash, Hasher},
        io::BufReader,
        path::{Path, PathBuf},
    };

    #[macro_export]
    macro_rules! test_roms {
        ($dir:expr, { $( ($test:ident, $run_frames:expr, $hash:expr $(, $ignore:expr)? $(,)?) ),* $(,)? }) => {$(
            $(#[ignore = $ignore])?
            #[test]
            fn $test() {
                $crate::common::tests::test_rom(concat!($dir, "/", stringify!($test), ".nes"), $run_frames, $hash);
            }
        )*};
    }

    #[macro_export]
    macro_rules! test_roms_adv {
        ($dir:expr, { $( ($test:ident, $run_frames:expr, $fn:expr $(, $ignore:expr)? $(,)?) ),* $(,)? }) => {$(
            $(#[ignore = $ignore])?
            #[test]
            fn $test() {
                $crate::common::tests::test_rom_advanced(concat!($dir, "/", stringify!($test), ".nes"), $run_frames, $fn);
            }
        )*};
    }

    pub(crate) const SLOT1: GamepadSlot = GamepadSlot::One;
    pub(crate) const RESULT_DIR: &str = "test_results";
    pub(crate) const TEST_DIR: &str = "test_roms";

    pub(crate) fn load<P: AsRef<Path>>(path: P) -> ControlDeck {
        let path = path.as_ref();
        let mut rom = BufReader::new(File::open(path).unwrap());
        let mut deck = ControlDeck::default();
        deck.load_rom(&path.to_string_lossy(), &mut rom).unwrap();
        deck.set_filter(VideoFilter::None);
        if std::env::var("RUST_LOG").is_ok() {
            pretty_env_logger::init();
            deck.cpu_mut().debugging = true;
        }
        deck
    }

    pub(crate) fn compare(expected: u64, deck: &mut ControlDeck, test: &str) {
        let mut hasher = DefaultHasher::new();
        let frame = deck.frame_buffer();
        frame.hash(&mut hasher);
        let actual = hasher.finish();
        let pass_path = PathBuf::from(RESULT_DIR).join("pass");
        let fail_path = PathBuf::from(RESULT_DIR).join("fail");

        if !pass_path.exists() {
            fs::create_dir_all(&pass_path).expect("created pass test results dir");
        }
        if !fail_path.exists() {
            fs::create_dir(&fail_path).expect("created fail test results dir");
        }

        let result_path = if expected == actual {
            pass_path
        } else {
            fail_path
        };
        let screenshot_path = result_path.join(PathBuf::from(test)).with_extension("png");
        Image::from_bytes(RENDER_WIDTH, RENDER_HEIGHT, frame, PixelFormat::Rgba)
            .expect("valid frame")
            .save(&screenshot_path)
            .expect("result screenshot");

        assert_eq!(expected, actual, "mismatched {:?}", screenshot_path);
    }

    pub(crate) fn test_rom<P: AsRef<Path>>(rom: P, run_frames: i32, expected: u64) {
        let rom = rom.as_ref();
        let mut deck = load(PathBuf::from(TEST_DIR).join(rom));
        for _ in 0..=run_frames {
            deck.clock_frame();
            deck.clear_audio_samples();
        }
        let test = rom.file_stem().expect("valid test file").to_string_lossy();
        compare(expected, &mut deck, &test);
    }

    pub(crate) fn test_rom_advanced<P, F>(rom: P, run_frames: i32, f: F)
    where
        P: AsRef<Path>,
        F: Fn(i32, &mut ControlDeck),
    {
        let rom = rom.as_ref();
        let mut deck = load(PathBuf::from(TEST_DIR).join(rom));
        for frame in 0..=run_frames {
            f(frame, &mut deck);
            deck.clock_frame();
            deck.clear_audio_samples();
        }
    }
}
