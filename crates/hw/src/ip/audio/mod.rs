//! Audio interface IP blocks (QEMU `hw/audio/`): sound cards and
//! on-SoC audio interfaces. Codec and amplifier chips live in
//! `crate::part::audio`.
pub mod ac97;
pub mod ad1980;
pub mod alc1220;
pub mod alc889;
pub mod alc892;
pub mod catalog;
pub mod cs4281;
pub mod es1370;
pub mod es1371;
pub mod hda;
pub mod live;
pub mod opl3;
pub mod pl041;
pub mod sb128;
pub mod sb16;
pub mod sbawe32;
