//! Various utility functions for the UI and Console

use crate::console::{Image, SCREEN_HEIGHT, SCREEN_WIDTH};
use chrono::prelude::*;
use dirs;
use failure::{format_err, Error};
use image::{png, ColorType};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Alias for Result<T, failure::Error>
pub type Result<T> = std::result::Result<T, Error>;

const CONFIG_DIR: &str = ".rustynes";
const SAVE_FILE_MAGIC: [u8; 9] = *b"RUSTYNES\x1a";
const VERSION: [u8; 6] = *b"v0.2.0";

/// Searches for valid NES rom files ending in `.nes`
///
/// If rom_path is a `.nes` file, uses that
/// If no arg[1], searches current directory for `.nes` files
pub fn find_roms<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>> {
    use std::ffi::OsStr;
    let path = path.as_ref();
    let mut roms = Vec::new();
    if path.is_dir() {
        path.read_dir()
            .map_err(|e| format_err!("unable to read directory {:?}: {}", path, e))?
            .filter_map(|f| f.ok())
            .filter(|f| f.path().extension() == Some(OsStr::new("nes")))
            .for_each(|f| roms.push(f.path()));
    } else if path.is_file() {
        roms.push(path.to_path_buf());
    } else {
        Err(format_err!("invalid path: {:?}", path))?;
    }
    if roms.is_empty() {
        Err(format_err!("no rom files found or specified"))?;
    }
    Ok(roms)
}

/// Returns the path where battery-backed Save RAM files are stored
///
/// # Arguments
///
/// * `path` - An object that implements AsRef<Path> that holds the path to the currently
/// running ROM
///
/// # Errors
///
/// Panics if path is not a valid path
pub fn sram_path<P: AsRef<Path>>(path: &P) -> Result<PathBuf> {
    let save_name = path.as_ref().file_stem().and_then(|s| s.to_str()).unwrap();
    let mut path = home_dir().unwrap_or_else(|| PathBuf::from("./"));
    path.push(CONFIG_DIR);
    path.push("sram");
    path.push(save_name);
    path.set_extension("dat");
    Ok(path)
}

/// Returns the path where Save states are stored
///
/// # Arguments
///
/// * `path` - An object that implements AsRef<Path> that holds the path to the currently
/// running ROM
///
/// # Errors
///
/// Panics if path is not a valid path
pub fn save_path<P: AsRef<Path>>(path: &P, slot: u8) -> Result<PathBuf> {
    let save_name = path.as_ref().file_stem().and_then(|s| s.to_str()).unwrap();
    let mut path = home_dir().unwrap_or_else(|| PathBuf::from("./"));
    path.push(CONFIG_DIR);
    path.push("save");
    path.push(save_name);
    path.push(format!("{}", slot));
    path.set_extension("dat");
    Ok(path)
}

/// Returns the path where ROM thumbnails have been downloaded to
///
/// # Arguments
///
/// * `path` - An object that implements AsRef<Path> that holds the path to the currently
/// running ROM
///
/// # Errors
///
/// Panics if path is not a valid path
pub fn thumbnail_path<P: AsRef<Path>>(path: &P) -> Result<PathBuf> {
    let filehash = hash_file(path)?;
    let mut path = home_dir().unwrap_or_else(|| PathBuf::from("./"));
    path.push(CONFIG_DIR);
    path.push("thumbnail");
    path.push(filehash);
    path.set_extension("png");
    Ok(path)
}

/// Returns a SHA256 hash of the first 255 bytes of a file to uniquely identify it
///
/// # Arguments
///
/// * `path` - An object that implements AsRef<Path> that holds the path to the currently
/// running ROM
///
/// # Errors
///
/// Panics if path is not a valid path or if there are permissions issues reading the file
pub fn hash_file<P: AsRef<Path>>(path: &P) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; 255];
    file.read_exact(&mut buf)?;
    Ok(format!("{:x}", Sha256::digest(&buf)))
}

/// Returns the users current HOME directory (if one exists)
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir().and_then(|d| Some(d.to_path_buf()))
}

/// Takes a screenshot and saves it to the current directory as a `.png` file
///
/// # Arguments
///
/// * `pixels` - An array of pixel data to save in `.png` format
///
/// # Errors
///
/// It's possible for this method to fail, but instead of erroring the program,
/// it'll simply log the error out to STDERR
pub fn screenshot(pixels: &Image) {
    let datetime: DateTime<Local> = Local::now();
    let mut png_path = PathBuf::from(format!(
        "screenshot_{}",
        datetime.format("%Y-%m-%dT%H-%M-%S").to_string()
    ));
    png_path.set_extension("png");
    create_png(&png_path, pixels);
}

/// Creates a '.png' file
///
/// # Arguments
///
/// * `png_path` - An object that implements AsRef<Path> for the location to save the `.png`
/// file
/// * `pixels` - An array of pixel data to save in `.png` format
///
/// # Errors
///
/// It's possible for this method to fail, but instead of erroring the program,
/// it'll simply log the error out to STDERR
pub fn create_png<P: AsRef<Path>>(png_path: &P, pixels: &Image) {
    let png_path = png_path.as_ref();
    let png_file = fs::File::create(&png_path);
    if png_file.is_err() {
        eprintln!(
            "failed to create png file {:?}: {}",
            png_path.display(),
            png_file.err().unwrap(),
        );
        return;
    }
    let png = png::PNGEncoder::new(png_file.unwrap());
    let encode = png.encode(
        pixels,
        SCREEN_WIDTH as u32,
        SCREEN_HEIGHT as u32,
        ColorType::RGB(8),
    );
    if encode.is_err() {
        eprintln!(
            "failed to save screenshot {:?}: {}",
            png_path.display(),
            encode.err().unwrap(),
        );
    }
    eprintln!("{}", png_path.display());
}

pub fn write_save_header(fh: &mut Write, save_path: &PathBuf) -> Result<()> {
    let mut header: Vec<u8> = Vec::new();
    header.extend(&SAVE_FILE_MAGIC.to_vec());
    header.extend(&VERSION.len().to_be_bytes());
    header.extend(&VERSION.to_vec());
    fh.write_all(&header)
        .map_err(|e| format_err!("failed to write save file {:?}: {}", save_path.display(), e))?;
    Ok(())
}

pub fn validate_save_header(fh: &mut Read, save_path: &PathBuf) -> Result<()> {
    let mut magic = [0u8; 9];
    fh.read_exact(&mut magic)?;
    if magic != SAVE_FILE_MAGIC {
        Err(format_err!(
            "invalid save file format {:?}",
            save_path.display()
        ))?;
    }
    let mut version_len = [0u8; 8];
    fh.read_exact(&mut version_len)?;
    let mut version = vec![0; usize::from_be_bytes(version_len)];
    fh.read_exact(&mut version)?;
    if version != VERSION {
        Err(format_err!(
            "invalid save file version {:?}. current: {}, save file: {}",
            save_path.display(),
            std::str::from_utf8(&VERSION)?,
            std::str::from_utf8(&version)?,
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_roms() {
        let rom_tests = &[
            // (Test name, Path, Error)
            // CWD with no `.nes` files
            (
                "CWD with no nes files",
                "./",
                "no rom files found or specified",
            ),
            // Directory with no `.nes` files
            (
                "Dir with no nes files",
                "src/",
                "no rom files found or specified",
            ),
            (
                "invalid directory",
                "invalid/",
                "invalid path: \"invalid/\"",
            ),
        ];
        for test in rom_tests {
            let roms = find_roms(test.1);
            assert!(roms.is_err(), "invalid path {}", test.0);
            assert_eq!(
                roms.err().unwrap().to_string(),
                test.2,
                "error matches {}",
                test.0
            );
        }
    }
}
