//! Fabric-mediated leaf parts — placed by what they *are* (ADR 0031).
//!
//! A part never appears in an address space of its own: a board attaches
//! it to a fabric (`conn` wiring) and the controller on the other end of
//! the fabric fronts it. Which bus a part speaks is a `bindings` attribute
//! in its manifest, not its directory: a part's home is its role — sensor,
//! memory, expander, power, motion, instrument.
//!
//! A multi-bus part (BME280-type) is one model with several binds, not
//! several models; the same chip is never registered twice.

pub mod adc;
pub mod audio;
pub mod can;
pub mod dac;
pub mod display;
pub mod ec;
pub mod expander;
pub mod fan;
pub mod i3c;
pub mod input;
pub mod instrument;
pub mod led;
pub mod memory;
pub mod motion;
pub mod mux;
pub mod net;
pub mod nfc;
pub mod power;
pub mod rtc;
pub mod security;
pub mod sensor;
pub mod serial;
pub mod storage;
