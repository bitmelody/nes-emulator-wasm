use std::{
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};
use structopt::StructOpt;
use tetanes::{cart::Cart, mem::RamState, NesResult};

fn main() -> NesResult<()> {
    let opt = Opt::from_args();
    let path = opt
        .path
        .unwrap_or_else(|| env::current_dir().unwrap_or_default());
    let board = opt.board.map(|b| b.to_lowercase());
    if path.is_dir() {
        let paths: Vec<PathBuf> = path
            .read_dir()
            .unwrap_or_else(|err| panic!("unable read directory {path:?}: {err}"))
            .filter_map(Result::ok)
            .filter(|f| f.path().extension() == Some(OsStr::new("nes")))
            .map(|f| f.path())
            .collect();
        let mut boards: Vec<String> = paths
            .iter()
            .map(get_mapper)
            .filter_map(Result::ok)
            .filter(|b| match &board {
                Some(board) => b.to_lowercase().contains(board),
                None => true,
            })
            .collect();
        boards.sort();
        for board in &boards {
            println!("{board}");
        }
    } else if path.is_file() {
        println!("{}", get_mapper(&path)?);
    }
    Ok(())
}

fn get_mapper<P: AsRef<Path>>(path: P) -> NesResult<String> {
    let cart = Cart::from_path(path, RamState::default())?;
    Ok(format!("{:<50} {:?}", cart.mapper_board(), cart.name()))
}

#[derive(StructOpt, Debug)]
#[must_use]
struct Opt {
    #[structopt(
        help = "The NES ROM or a directory containing `.nes` ROM files. [default: current directory]"
    )]
    path: Option<PathBuf>,
    #[structopt(help = "The NES Mapper Board to filter by.")]
    board: Option<String>,
}
