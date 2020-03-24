use crate::{
    common::{Clocked, Powered},
    serialization::Savable,
    NesResult,
};
use std::io::{Read, Write};

#[derive(Clone)]
pub struct Sequencer {
    pub step: usize,
    pub length: usize,
}

impl Sequencer {
    pub(super) fn new(length: usize) -> Self {
        Self { step: 1, length }
    }
}

impl Clocked for Sequencer {
    fn clock(&mut self) -> usize {
        let clock = self.step;
        self.step += 1;
        if self.step > self.length {
            self.step = 1;
        }
        clock as usize
    }
}

impl Powered for Sequencer {
    fn reset(&mut self) {
        self.step = 1;
    }
}

impl Savable for Sequencer {
    fn save(&self, fh: &mut dyn Write) -> NesResult<()> {
        self.step.save(fh)?;
        self.length.save(fh)?;
        Ok(())
    }
    fn load(&mut self, fh: &mut dyn Read) -> NesResult<()> {
        self.step.load(fh)?;
        self.length.load(fh)?;
        Ok(())
    }
}