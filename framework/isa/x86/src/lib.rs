#![no_std]
#![forbid(unsafe_code)]
//! x86 (IA-32) register and exception vocabulary. These values are Mosaic IDs,
//! not Unicorn IDs: the engine backend maps them per ISA (design §12.1).
//!
//! The integer register order is the hardware encoding order used by the
//! ModRM/opcode tables (EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI), so a
//! trace produced by this vocabulary lines up with an opcode listing.
//! Only IA-32 state that a 16/32-bit PC platform observes is named here;
//! long-mode and FPU/SIMD register files join with the first 64-bit slice.

use mosaic_core_types::RegId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Register {
    Eax,
    Ecx,
    Edx,
    Ebx,
    Esp,
    Ebp,
    Esi,
    Edi,
    /// Instruction pointer (`EIP`; `IP` in real mode).
    Eip,
    Eflags,
    Cs,
    Ds,
    Es,
    Fs,
    Gs,
    Ss,
    Cr0,
    Cr2,
    Cr3,
    Cr4,
}

impl From<Register> for RegId {
    fn from(reg: Register) -> Self {
        Self(reg as u32)
    }
}

/// `EFLAGS` bit positions (Intel SDM Vol.1 §3.4.3).
pub mod eflags {
    pub const CF: u32 = 1 << 0;
    pub const PF: u32 = 1 << 2;
    pub const AF: u32 = 1 << 4;
    pub const ZF: u32 = 1 << 6;
    pub const SF: u32 = 1 << 7;
    /// Trap flag. Hardware clears it on interrupt entry.
    pub const TF: u32 = 1 << 8;
    /// Interrupt enable flag. Hardware clears it on interrupt-gate entry.
    pub const IF: u32 = 1 << 9;
    pub const DF: u32 = 1 << 10;
    pub const OF: u32 = 1 << 11;
}

/// `CR0` bit positions (Intel SDM Vol.3 §2.5).
pub mod cr0 {
    /// Protection enable: 0 = real mode, 1 = protected mode.
    pub const PE: u32 = 1 << 0;
    pub const MP: u32 = 1 << 1;
    pub const EM: u32 = 1 << 2;
    pub const TS: u32 = 1 << 3;
    pub const NE: u32 = 1 << 5;
    pub const WP: u32 = 1 << 16;
    /// Paging enable.
    pub const PG: u32 = 1 << 31;
}

/// Architectural exception vectors below the external-interrupt range.
pub mod exception {
    pub const DIVIDE_ERROR: u8 = 0;
    pub const DEBUG: u8 = 1;
    pub const NMI: u8 = 2;
    pub const BREAKPOINT: u8 = 3;
    pub const OVERFLOW: u8 = 4;
    pub const BOUND_RANGE: u8 = 5;
    pub const INVALID_OPCODE: u8 = 6;
    pub const DEVICE_NOT_AVAILABLE: u8 = 7;
    pub const DOUBLE_FAULT: u8 = 8;
    pub const INVALID_TSS: u8 = 10;
    pub const SEGMENT_NOT_PRESENT: u8 = 11;
    pub const STACK_FAULT: u8 = 12;
    pub const GENERAL_PROTECTION: u8 = 13;
    pub const PAGE_FAULT: u8 = 14;
    pub const X87_FP: u8 = 16;
    pub const ALIGNMENT_CHECK: u8 = 17;
    pub const MACHINE_CHECK: u8 = 18;
    pub const SIMD_FP: u8 = 19;
}

/// ISA interrupt line numbers as wired on the standard AT bus (8259 inputs).
/// The vector a line raises is `MASTER_BASE + line` on the master PIC and
/// `SLAVE_BASE + line - 8` on the slave; the bases are programmed by the guest
/// at boot (the PC convention is 0x20/0x28).
pub mod isa_irq {
    pub const TIMER: u8 = 0;
    pub const KEYBOARD: u8 = 1;
    pub const CASCADE: u8 = 2;
    pub const COM2: u8 = 3;
    pub const COM1: u8 = 4;
    pub const LPT2: u8 = 5;
    pub const FLOPPY: u8 = 6;
    pub const LPT1: u8 = 7;
    pub const RTC: u8 = 8;
    pub const IDE_PRIMARY: u8 = 14;
    pub const IDE_SECONDARY: u8 = 15;
    pub const COUNT: u8 = 16;
}

/// Interrupt gate descriptor types (Intel SDM Vol.3 §6.11, Table 6-2).
pub mod gate {
    pub const TASK_32: u8 = 0x5;
    pub const INTERRUPT_16: u8 = 0x6;
    pub const TRAP_16: u8 = 0x7;
    pub const INTERRUPT_32: u8 = 0xE;
    pub const TRAP_32: u8 = 0xF;

    /// Hardware clears `IF` on entry through an interrupt gate only.
    pub const fn clears_if(gate_type: u8) -> bool {
        matches!(gate_type, INTERRUPT_16 | INTERRUPT_32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_ids_are_the_hardware_encoding_order() {
        assert_eq!(RegId::from(Register::Eax), RegId(0));
        assert_eq!(RegId::from(Register::Esp), RegId(4));
        assert_eq!(RegId::from(Register::Edi), RegId(7));
        assert_eq!(RegId::from(Register::Eip), RegId(8));
        assert_eq!(RegId::from(Register::Cr4), RegId(19));
    }

    #[test]
    fn isa_irq_lines_fit_between_the_two_pics() {
        assert_eq!(isa_irq::COUNT, 16);
        assert_eq!(isa_irq::RTC, 8);
        assert_eq!(isa_irq::IDE_PRIMARY, 14);
    }

    #[test]
    fn only_interrupt_gates_mask_further_interrupts() {
        assert!(gate::clears_if(gate::INTERRUPT_32));
        assert!(gate::clears_if(gate::INTERRUPT_16));
        assert!(!gate::clears_if(gate::TRAP_32));
        assert!(!gate::clears_if(gate::TRAP_16));
        assert!(!gate::clears_if(gate::TASK_32));
    }
}
