use pix_engine::*;
use std::time::Duration;

struct App {
    paused: bool,
}

impl App {
    fn new() -> Self {
        Self { paused: false }
    }
}

impl State for App {
    fn on_start(&mut self, data: &mut StateData) -> bool {
        true
    }
    fn on_update(&mut self, elapsed: Duration, data: &mut StateData) -> bool {
        true
    }
    fn on_stop(&mut self, _data: &mut StateData) -> bool {
        true
    }
}

pub fn main() {
    let app = App::new();

    let mut engine = PixEngine::new("Asteroids", app, 800, 600).fullscreen(false);
    engine.run().unwrap();
}
