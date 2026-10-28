#![forbid(unsafe_code)]
//! `machine` — machine models by architecture (QEMU's `hw/<arch>/`): SoC
//! containers, board machines and the per-arch boot paths that give
//! `-kernel` its meaning. A machine is selected by name (`-M`); adding one
//! is a module under `src/<arch>/` plus one registry arm below — the
//! front-end never grows machine code (ADR 0021/0023/0024).

pub mod arm;
pub mod i386;
pub mod manifest;

/// One machine type the front-end can select (QEMU's `MODULE_INFO` metadata).
pub struct MachineInfo {
    /// The `-M` spelling of this machine.
    pub name: &'static str,
    /// One-line description, as `-M help` prints it.
    pub summary: &'static str,
    /// CPU architectures the machine's engines cover.
    pub architectures: &'static [&'static str],
}

/// The `-M help` listing, in print order.
pub fn machines() -> Vec<MachineInfo> {
    vec![i386::pc::info(), arm::stm32f103::info()]
}
