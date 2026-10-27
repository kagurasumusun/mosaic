//! `midi` contract: MIDI 1.0 UART (31250 baud, current-loop opto-isolated).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MidiError {
    NoReceiver,
    Overflow,
    BadStatus,
}

#[derive(Clone, Copy, Debug)]
pub struct MidiMessage {
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

pub trait MidiOutput {
    fn send(&mut self, ctx: &mut dyn Ctx, m: &MidiMessage) -> Result<(), MidiError>;
}

pub trait MidiInput {
    fn on_message(&mut self, ctx: &mut dyn Ctx, m: &MidiMessage) -> Result<(), MidiError>;
}
