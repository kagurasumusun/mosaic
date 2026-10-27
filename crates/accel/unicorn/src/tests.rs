//! Unit tests for the Unicorn adapter: register maps, the x86 interrupt
//! entry machinery, memory binding and engine budgets.

use super::*;
use std::sync::{Arc, Mutex};

use mosaic_isa_arm::Register;

/// IA-32 fixture: 64 KiB of RAM at physical 0 plus a port recorder.
fn x86_engine(ports: Option<Box<dyn PortBus>>) -> UnicornX86 {
    let mut engine = UnicornX86::new().unwrap();
    engine
        .map_memory(Addr(0), 0x1_0000, Permissions::RWX)
        .unwrap();
    if let Some(ports) = ports {
        engine.attach_ports(ports).unwrap();
    }
    engine
}

#[test]
fn x86_unclaimed_exception_stops_the_quantum_and_names_the_vector() {
    // Unicorn never delivers an exception through the guest's IDT: it calls
    // UC_HOOK_INTR and moves on. With no service attached the engine must
    // stop instead of silently skipping the fault, and the report has to
    // say which vector it was.
    let mut engine = UnicornX86Boot::new().unwrap();
    engine
        .map_memory(Addr(0), 0x10_0000, Permissions::RWX)
        .unwrap();
    // xor ecx,ecx ; div ecx  -> #DE (vector 0)
    engine
        .write_memory(Addr(0x500), &[0x31, 0xC9, 0xF7, 0xF1])
        .unwrap();
    engine
        .reg_write(mosaic_isa_x86::i386::Register::Eip.into(), 0x500)
        .unwrap();
    let outcome = engine.run(NonZeroU64::new(4).unwrap());
    assert_eq!(outcome.reason, StopReason::CapabilityMissing);
    // No gate table exists yet, so there is nowhere to deliver `#DE`: the
    // quantum stops and names the vector instead of skipping the fault.
    assert_eq!(engine.take_unclaimed_interrupt(), Some(0));
    assert_eq!(engine.take_exception(), Some((0, 0x502, 0)));
}

#[test]
fn x86_boot_core_resumes_across_a_real_mode_far_jump() {
    use mosaic_isa_x86::i386::Register;
    // `MODE_16`'s `uc_emu_start` rewrites the start address as a 16-bit IP
    // (`ip = begin - cs * 16`), so a context parked anywhere but the
    // segment base cannot be resumed. The boot core runs on the 32-bit mode
    // word instead and keeps real-mode semantics, so the same far jump
    // resumes on the next instruction (ADR 0020).
    let mut engine = UnicornX86Boot::new().unwrap();
    engine
        .map_memory(Addr(0), 0x10_0000, Permissions::RWX)
        .unwrap();
    // At 0x500: jmp far 9020:0000.
    engine
        .write_memory(Addr(0x500), &[0xEA, 0x00, 0x00, 0x20, 0x90])
        .unwrap();
    // Setup entry: jmp +0x6a, then `mov ax,0x1234 ; mov bx,0x5678`.
    engine.write_memory(Addr(0x90_200), &[0xEB, 0x6A]).unwrap();
    engine
        .write_memory(Addr(0x90_26C), &[0xB8, 0x34, 0x12, 0xBB, 0x78, 0x56])
        .unwrap();
    engine.reg_write(Register::Cs.into(), 0).unwrap();
    engine.reg_write(Register::Eip.into(), 0x500).unwrap();
    let mut eips = Vec::new();
    for _ in 0..4 {
        engine.run(NonZeroU64::new(1).unwrap());
        eips.push((
            engine.reg_read(Register::Cs.into()).unwrap(),
            engine.reg_read(Register::Eip.into()).unwrap(),
        ));
    }
    assert_eq!(
        eips,
        vec![(0x9020, 0), (0x9020, 0x6C), (0x9020, 0x6F), (0x9020, 0x72)],
        "each quantum must retire exactly one instruction and resume in the segment"
    );
    assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x1234);
    assert_eq!(engine.reg_read(Register::Ebx.into()).unwrap(), 0x5678);
}

#[derive(Default)]
struct RecordingPorts {
    writes: Vec<(u16, usize, u32)>,
    reads: Vec<u16>,
    input: u32,
}

impl PortBus for RecordingPorts {
    fn read(&mut self, port: u16, _size: usize) -> Result<u32, BusFault> {
        self.reads.push(port);
        Ok(self.input)
    }

    fn write(&mut self, port: u16, size: usize, value: u32) -> Result<(), BusFault> {
        self.writes.push((port, size, value));
        Ok(())
    }
}

struct RecordingInterrupts {
    seen: Vec<u8>,
}

impl InterruptService for RecordingInterrupts {
    fn service(&mut self, vector: u8, cpu: &mut dyn SoftwareInterruptCpu) -> bool {
        self.seen.push(vector);
        // Report a value through AX, as a BIOS service would.
        let _ = cpu.reg_write(RegId(0), 0xB10C);
        true
    }
}

#[test]
fn x86_software_interrupt_service_answers_and_resumes_after_the_int() {
    use mosaic_isa_x86::i386::Register;
    let mut engine = x86_engine(None);
    engine
        .attach_interrupt_service(Box::new(RecordingInterrupts { seen: Vec::new() }))
        .unwrap();
    // int 0x10 ; mov ah, 0x4C ; hlt
    let program = [0xCD, 0x10, 0xB4, 0x4C, 0xF4];
    engine.write_memory(Addr(0x100), &program).unwrap();
    engine.reg_write(Register::Cs.into(), 0).unwrap();
    engine.reg_write(Register::Eip.into(), 0x100).unwrap();
    engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
    engine.run(NonZeroU64::new(2).unwrap());
    // The service wrote AX = 0xB10C and the guest then executed
    // `mov ah, 0x4C`, so AX ends as 0x4C0C: the service ran, and the CPU
    // resumed after the two-byte INT encoding instead of re-executing it.
    assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x4C0C);
    assert!(
        engine.reg_read(Register::Eip.into()).unwrap() >= 0x103,
        "EIP must be past the INT instruction"
    );
}

#[test]
fn x86_real_mode_interrupt_pushes_a_16_bit_frame_from_the_ivt() {
    use mosaic_isa_x86::i386::{eflags, Register};
    let mut engine = x86_engine(None);
    // Handler at 0000:0100 spins in place.
    engine.write_memory(Addr(0x100), &[0xEB, 0xFE]).unwrap();
    // IVT entry for vector 0x20 -> 0000:0100.
    let mut entry = Vec::new();
    entry.extend_from_slice(&0x0100u16.to_le_bytes());
    entry.extend_from_slice(&0x0000u16.to_le_bytes());
    engine.write_memory(Addr(0x20 * 4), &entry).unwrap();
    // Interrupted context at 0000:0200 with IF set.
    engine.reg_write(Register::Cs.into(), 0).unwrap();
    engine.reg_write(Register::Eip.into(), 0x200).unwrap();
    engine.reg_write(Register::Ss.into(), 0).unwrap();
    engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
    engine
        .reg_write(Register::Eflags.into(), u64::from(eflags::IF))
        .unwrap();

    engine.queue_interrupt(0x20);
    assert_eq!(engine.pending_interrupts(), 1);
    engine.run(NonZeroU64::new(1).unwrap());

    assert_eq!(engine.pending_interrupts(), 0);
    assert_eq!(engine.reg_read(Register::Eip.into()).unwrap(), 0x100);
    // Frame: IP, CS, FLAGS at growing addresses from the new SP.
    let sp = engine.reg_read(Register::Esp.into()).unwrap();
    assert_eq!(sp, 0x8000 - 6);
    let mut frame = [0u8; 6];
    engine.read_memory(Addr(sp), &mut frame).unwrap();
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x0200);
    assert_eq!(u16::from_le_bytes([frame[2], frame[3]]), 0x0000);
    // The saved copy keeps the interrupted IF; the live EFLAGS loses it.
    assert_ne!(
        u16::from_le_bytes([frame[4], frame[5]]) & eflags::IF as u16,
        0,
        "the saved frame keeps the interrupted IF for IRET"
    );
    assert_eq!(
        engine.reg_read(Register::Eflags.into()).unwrap() & u64::from(eflags::IF),
        0,
        "real-mode entry clears IF"
    );
}

#[test]
fn x86_exception_enters_the_real_mode_ivt_handler() {
    use mosaic_isa_x86::i386::Register;
    // Unicorn reports `#DE` to `UC_HOOK_INTR` and then continues without
    // entering any handler, so a real-mode guest's own IVT vector is the
    // adapter's job. The frame must be the one `iret` expects.
    let mut engine = x86_engine(None);
    // 0000:1000 -> `mov bx,0xDEAD ; hlt`.
    engine
        .write_memory(Addr(0x1000), &[0xBB, 0xAD, 0xDE, 0xF4])
        .unwrap();
    let mut entry = Vec::new();
    entry.extend_from_slice(&0x1000u16.to_le_bytes());
    entry.extend_from_slice(&0x0000u16.to_le_bytes());
    engine.write_memory(Addr(0), &entry).unwrap();
    // xor ecx,ecx ; div ecx -> #DE at 0000:0502.
    engine
        .write_memory(Addr(0x500), &[0x31, 0xC9, 0xF7, 0xF1])
        .unwrap();
    engine.reg_write(Register::Cs.into(), 0).unwrap();
    engine.reg_write(Register::Eip.into(), 0x500).unwrap();
    engine.reg_write(Register::Ss.into(), 0).unwrap();
    engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
    engine.run(NonZeroU64::new(4).unwrap());

    assert_eq!(
        engine.take_exception(),
        Some((0, 0x502, 0)),
        "the fault is reported where it happened"
    );
    assert_eq!(engine.take_unclaimed_interrupt(), None);
    let handler_eip = engine.reg_read(Register::Eip.into()).unwrap();
    assert!(
        (0x1000..0x1010).contains(&handler_eip),
        "the IVT handler runs, EIP {handler_eip:#x}"
    );
    assert_eq!(engine.reg_read(Register::Cs.into()).unwrap(), 0);
    assert_eq!(
        engine.reg_read(Register::Esp.into()).unwrap(),
        0x8000 - 6,
        "a real-mode frame is three 16-bit words"
    );
    let mut frame = [0u8; 6];
    engine.read_memory(Addr(0x8000 - 6), &mut frame).unwrap();
    assert_eq!(
        u16::from_le_bytes([frame[0], frame[1]]),
        0x502,
        "IP points at the faulting division"
    );
    assert_eq!(u16::from_le_bytes([frame[2], frame[3]]), 0);
    // The handler runs inside the same quantum: `mov bx,0xDEAD`.
    assert_eq!(engine.reg_read(Register::Ebx.into()).unwrap(), 0xDEAD);
}

#[test]
fn x86_page_fault_enters_the_idt_gate_with_an_error_code() {
    use mosaic_isa_x86::i386::Register;
    // The WP-bit probe every Linux x86 kernel runs is a deliberately
    // read-only page: it proves the CPU honours `CR0.WP` by faulting on a
    // supervisor write. The kernel expects the hardware to push
    // `error code / EIP / CS / EFLAGS` and vector through `IDT[14]`, and
    // the error code has to say "present, write" or the handler takes the
    // wrong branch.
    let mut engine = UnicornX86Boot::new().unwrap();
    engine
        .map_memory(Addr(0), 0x10_0000, Permissions::RWX)
        .unwrap();

    // GDT: null, 0x08 flat 32-bit code, 0x10 flat 32-bit data.
    let mut gdt = Vec::new();
    gdt.extend_from_slice(&[0u8; 8]);
    gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x9A, 0xCF, 0x00]);
    gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x92, 0xCF, 0x00]);
    engine.write_memory(Addr(0x2000), &gdt).unwrap();
    let mut gdtr = Vec::new();
    gdtr.extend_from_slice(&((gdt.len() - 1) as u16).to_le_bytes());
    gdtr.extend_from_slice(&0x2000u32.to_le_bytes());
    engine.write_memory(Addr(0x2100), &gdtr).unwrap();

    // IDT with one gate: vector 14 -> 0x08:0x6000, 32-bit interrupt gate.
    let mut idt = vec![0u8; 14 * 8];
    idt.extend_from_slice(&[0x00, 0x60, 0x08, 0x00, 0x00, 0x8E, 0x00, 0x00]);
    engine.write_memory(Addr(0x2200), &idt).unwrap();
    let mut idtr = Vec::new();
    idtr.extend_from_slice(&((idt.len() - 1) as u16).to_le_bytes());
    idtr.extend_from_slice(&0x2200u32.to_le_bytes());
    engine.write_memory(Addr(0x2110), &idtr).unwrap();

    // Identity map the first 4 MiB through 4 KiB pages; page 0x90 is
    // present but read-only, which is the page the guest will write.
    let mut page_table = Vec::with_capacity(4096);
    for index in 0..1024u32 {
        let entry = if index == 0x90 {
            0x90_0001u32
        } else {
            (index << 12) | 3
        };
        page_table.extend_from_slice(&entry.to_le_bytes());
    }
    engine.write_memory(Addr(0x4000), &page_table).unwrap();
    engine
        .write_memory(Addr(0x3000), &(0x4000u32 | 3).to_le_bytes())
        .unwrap();

    // Handler at 0x6000: `mov eax,0x0BAD ; hlt`. It must not share a page
    // with anything else the test patches: a handler written over the page
    // tables silently rewrites the identity map.
    engine
        .write_memory(Addr(0x6000), &[0xB8, 0xAD, 0x0B, 0x00, 0x00, 0xF4])
        .unwrap();

    // Real-mode prologue: the default operand size is 16 bits until a
    // 32-bit code segment is loaded, so the `CR0` write is spelled with an
    // operand-size prefix and the far jump with a 32-bit offset.
    let mut code = Vec::new();
    // lgdt [0x2100]
    code.extend_from_slice(&[0x0F, 0x01, 0x16, 0x00, 0x21]);
    // or eax, CR0.PG | CR0.WP | CR0.PE
    code.extend_from_slice(&[0x66, 0x0D, 0x01, 0x00, 0x01, 0x80]);
    // mov cr0, eax
    code.extend_from_slice(&[0x0F, 0x22, 0xC0]);
    let far_jump_at = code.len();
    code.extend_from_slice(&[0x66, 0xEA, 0, 0, 0, 0, 0x08, 0x00]);
    let entry_at = code.len();
    let entry = 0x1000u32 + entry_at as u32;
    code[far_jump_at + 2..far_jump_at + 6].copy_from_slice(&entry.to_le_bytes());
    // mov ax,0x10 ; mov ds,ax ; mov ss,ax ; mov esp,0x80000 ;
    // lidt [0x2110] ; mov byte [0x90000],1 ; hlt
    code.extend_from_slice(&[0x66, 0xB8, 0x10, 0x00]);
    code.extend_from_slice(&[0x8E, 0xD8]);
    code.extend_from_slice(&[0x8E, 0xD0]);
    code.extend_from_slice(&[0xBC, 0x00, 0x00, 0x08, 0x00]);
    code.extend_from_slice(&[0x0F, 0x01, 0x1D, 0x10, 0x21, 0x00, 0x00]);
    let store_at = 0x1000u32 + code.len() as u32;
    code.extend_from_slice(&[0xC6, 0x05, 0x00, 0x00, 0x09, 0x00, 0x01]);
    code.extend_from_slice(&[0xF4]);
    engine.write_memory(Addr(0x1000), &code).unwrap();

    // CR3 lives at the ISA's register 18; the page directory is above the
    // low page tables so a mistake shows up as a fault, not a silent pass.
    // Unicorn leaves `CR4.PAE` set on this backend; a 32-bit guest that
    // installs two-level tables has to clear it, exactly as a non-PAE
    // kernel does in its own `CR4` write.
    engine.reg_write(RegId(19), 0).unwrap();
    engine.reg_write(RegId(18), 0x3000).unwrap();
    engine.reg_write(Register::Cs.into(), 0).unwrap();
    engine.reg_write(Register::Eip.into(), 0x1000).unwrap();
    engine.run(NonZeroU64::new(64).unwrap());

    assert_eq!(
        engine.take_exception(),
        Some((14, u64::from(store_at), 0x9_0000)),
        "#PF is reported with the faulting instruction and CR2"
    );
    let handler_eip = engine.reg_read(Register::Eip.into()).unwrap();
    assert!(
        (0x6000..0x6010).contains(&handler_eip),
        "the guest's own #PF handler runs, EIP {handler_eip:#x}"
    );
    assert_eq!(engine.reg_read(Register::Cs.into()).unwrap(), 0x08);
    assert_eq!(
        engine.reg_read(Register::Esp.into()).unwrap(),
        0x80000 - 16,
        "error code + three word frame"
    );
    let mut frame = [0u8; 16];
    engine.read_memory(Addr(0x80000 - 16), &mut frame).unwrap();
    let word =
        |at: usize| u32::from_le_bytes([frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]);
    assert_eq!(word(0), 3, "error code: present | write");
    assert_eq!(word(4), store_at, "EIP points at the faulting store");
    assert_eq!(word(8), 0x08, "CS of the faulting context");
    assert_ne!(word(12) & 0x2, 0, "the frame carries a real EFLAGS image");
    assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x0BAD);
}

#[test]
fn x86_masked_interrupt_stays_queued_until_if_is_set() {
    use mosaic_isa_x86::i386::Register;
    let mut engine = x86_engine(None);
    engine.reg_write(Register::Cs.into(), 0).unwrap();
    engine.reg_write(Register::Eip.into(), 0x200).unwrap();
    engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
    engine.reg_write(Register::Eflags.into(), 0).unwrap();
    engine.queue_interrupt(0x20);
    engine.run(NonZeroU64::new(4).unwrap());
    assert_eq!(engine.pending_interrupts(), 1);
    assert_eq!(
        engine.reg_read(Register::Esp.into()).unwrap(),
        0x8000,
        "a masked request must not touch the stack"
    );
}

#[test]
fn x86_port_io_reaches_the_port_bus_in_both_directions() {
    use mosaic_isa_x86::i386::Register;
    let mut engine = x86_engine(Some(Box::new(RecordingPorts {
        input: 0x5A,
        ..RecordingPorts::default()
    })));
    // mov dx, 0x3F8 ; mov al, 0x41 ; out dx, al ; in al, dx ; hlt
    let program = [0xBA, 0xF8, 0x03, 0xB0, 0x41, 0xEE, 0xEC, 0xF4];
    engine.write_memory(Addr(0x100), &program).unwrap();
    engine.reg_write(Register::Cs.into(), 0).unwrap();
    engine.reg_write(Register::Eip.into(), 0x100).unwrap();
    let outcome = engine.run(NonZeroU64::new(5).unwrap());
    assert_eq!(outcome.reason, StopReason::BudgetExhausted, "{outcome:?}");
    // The recorder is owned by the engine now; assert through the guest AL.
    assert_eq!(engine.reg_read(Register::Eax.into()).unwrap(), 0x5A);
}

#[test]
fn x86_protected_mode_interrupt_uses_the_gate_and_masks_if() {
    use mosaic_isa_x86::i386::{eflags, Register};
    let mut engine = UnicornX86::protected_mode().unwrap();
    engine
        .map_memory(Addr(0), 0x1_0000, Permissions::RWX)
        .unwrap();
    // GDT at 0x1000: null, flat code (0x08), flat data (0x10).
    let mut gdt = Vec::new();
    gdt.extend_from_slice(&[0u8; 8]);
    gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x9A, 0xCF, 0x00]);
    gdt.extend_from_slice(&[0xFF, 0xFF, 0x00, 0x00, 0x00, 0x92, 0xCF, 0x00]);
    engine.write_memory(Addr(0x1000), &gdt).unwrap();
    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&((gdt.len() as u16) - 1).to_le_bytes());
    pseudo.extend_from_slice(&0x1000u32.to_le_bytes());
    engine.write_memory(Addr(0x400), &pseudo).unwrap();
    // IDT at 0x2000, gate 0x20 -> 0x0008:0x0300, type 0x8E (interrupt gate).
    let mut idt = vec![0u8; 8 * 0x21];
    idt[0x20 * 8..0x20 * 8 + 8].copy_from_slice(&[0x00, 0x03, 0x08, 0x00, 0x00, 0x8E, 0x00, 0x00]);
    engine.write_memory(Addr(0x2000), &idt).unwrap();
    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&((idt.len() as u16) - 1).to_le_bytes());
    pseudo.extend_from_slice(&0x2000u32.to_le_bytes());
    engine.write_memory(Addr(0x410), &pseudo).unwrap();
    engine.write_memory(Addr(0x300), &[0xEB, 0xFE]).unwrap();
    // lgdt [0x400] ; lidt [0x410] ; jmp $
    let program = [
        0x0F, 0x01, 0x15, 0x00, 0x04, 0x00, 0x00, 0x0F, 0x01, 0x1D, 0x10, 0x04, 0x00, 0x00, 0xEB,
        0xFE,
    ];
    engine.write_memory(Addr(0x200), &program).unwrap();
    engine.reg_write(Register::Eip.into(), 0x200).unwrap();
    engine.reg_write(Register::Esp.into(), 0x8000).unwrap();
    engine
        .reg_write(Register::Eflags.into(), u64::from(eflags::IF))
        .unwrap();
    // Load the descriptor tables while the core is still un-protected, then
    // enter protected mode and reload the flat selectors from the GDT.
    engine.run(NonZeroU64::new(2).unwrap());
    engine.reg_write(Register::Cr0.into(), 1).unwrap();
    engine.reg_write(Register::Cs.into(), 0x08).unwrap();
    engine.reg_write(Register::Ds.into(), 0x10).unwrap();
    engine.reg_write(Register::Ss.into(), 0x10).unwrap();
    // Park the interrupted context on a two-byte spin instruction.
    engine.write_memory(Addr(0x220), &[0xEB, 0xFE]).unwrap();
    engine.reg_write(Register::Eip.into(), 0x220).unwrap();
    engine.run(NonZeroU64::new(1).unwrap());
    assert_eq!(engine.reg_read(Register::Eip.into()).unwrap(), 0x220);

    engine.queue_interrupt(0x20);
    engine.run(NonZeroU64::new(1).unwrap());
    assert_eq!(engine.pending_interrupts(), 0);
    assert_eq!(engine.reg_read(Register::Eip.into()).unwrap(), 0x300);
    assert_eq!(engine.reg_read(Register::Cs.into()).unwrap(), 0x08);
    let esp = engine.reg_read(Register::Esp.into()).unwrap();
    assert_eq!(esp, 0x8000 - 12, "32-bit gate pushes a 12-byte frame");
    let mut frame = [0u8; 12];
    engine.read_memory(Addr(esp), &mut frame).unwrap();
    assert_eq!(
        u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]),
        0x220,
        "return EIP is the interrupted instruction"
    );
    assert_eq!(
        u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]),
        0x08
    );
    assert_ne!(
        u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) & eflags::IF,
        0,
        "the saved frame keeps the interrupted IF so IRET restores it"
    );
    assert_eq!(
        engine.reg_read(Register::Eflags.into()).unwrap() & u64::from(eflags::IF),
        0,
        "the live EFLAGS loses IF"
    );
}

fn engine() -> UnicornArm {
    let mut engine = UnicornArm::new().unwrap();
    engine
        .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
        .unwrap();
    engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
    engine
}

#[test]
fn mclass_backend_executes_t32_from_reset() {
    // ADR 0012: the M-class construction (Cortex-M33 via UC_MODE_MCLASS,
    // T bit asserted through the R15 write path) runs T32 from reset,
    // which the A32 backend cannot offer. Listing: MOVS r0,#42 (0x202A) ;
    // MOVS r1,#1 (0x2101) ; ADDS r0,r0,r1 (0x1840) ;
    // STR r1,[r2,#0] (0x6011).
    let mut engine = UnicornCortexM3::new().unwrap();
    engine
        .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
        .unwrap();
    engine
        .write_memory(
            Addr(0x1000),
            &[0x2a, 0x20, 0x01, 0x21, 0x40, 0x18, 0x11, 0x60],
        )
        .unwrap();
    engine.reg_write(Register::R2.into(), 0x1100).unwrap();
    engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
    let outcome = engine.run(NonZeroU64::new(4).unwrap());
    assert_eq!(outcome.reason, StopReason::BudgetExhausted);
    assert_eq!(outcome.instruction_hooks, 4);
    assert_eq!(engine.reg_read(Register::R0.into()), Ok(43));
    assert_eq!(engine.reg_read(Register::Pc.into()), Ok(0x1008));
    let mut bytes = [0; 4];
    engine.read_memory(Addr(0x1100), &mut bytes).unwrap();
    assert_eq!(bytes[0], 1);
    // M-profile PC writes are halfword aligned; odd values stay invalid.
    engine.reg_write(Register::Pc.into(), 0x1002).unwrap();
    assert_eq!(
        engine.reg_write(Register::Pc.into(), 0x1003),
        Err(EngineError::InvalidValue)
    );
    // The A32 backend still rejects halfword targets outright.
    let mut a32 = UnicornArm::new().unwrap();
    a32.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
        .unwrap();
    assert_eq!(
        a32.reg_write(Register::Pc.into(), 0x1002),
        Err(EngineError::InvalidValue)
    );
}

#[test]
fn context_save_and_restore_preserves_registers() {
    let mut engine = engine();
    engine.reg_write(Register::R0.into(), 123).unwrap();
    engine.reg_write(Register::R1.into(), 456).unwrap();
    let ctx = engine.save_context().unwrap();
    engine.reg_write(Register::R0.into(), 999).unwrap();
    engine.reg_write(Register::R1.into(), 888).unwrap();
    engine.restore_context(&ctx).unwrap();
    assert_eq!(engine.reg_read(Register::R0.into()), Ok(123));
    assert_eq!(engine.reg_read(Register::R1.into()), Ok(456));
}

#[test]
fn x86_64_backend_executes_instructions() {
    use mosaic_isa_x86::x86_64::Register as X86Reg;
    let mut engine = UnicornX86_64::new().unwrap();
    engine
        .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
        .unwrap();
    // nop (0x90), nop (0x90), jmp $ (0xeb, 0xfe)
    engine
        .write_memory(Addr(0x1000), &[0x90, 0x90, 0xeb, 0xfe])
        .unwrap();
    engine.reg_write(X86Reg::Rip.into(), 0x1000).unwrap();
    let outcome = engine.run(NonZeroU64::new(2).unwrap());
    assert_eq!(outcome.reason, StopReason::BudgetExhausted);
    assert_eq!(engine.reg_read(X86Reg::Rip.into()), Ok(0x1002));
}

#[test]
fn arm64_backend_executes_instructions() {
    use mosaic_isa_aarch64::Register as Arm64Reg;
    let mut engine = UnicornArm64::new().unwrap();
    engine
        .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
        .unwrap();
    // nop (0xd503201f le), b . (0x14000000 le)
    engine
        .write_memory(
            Addr(0x1000),
            &[0x1f, 0x20, 0x03, 0xd5, 0x00, 0x00, 0x00, 0x14],
        )
        .unwrap();
    engine.reg_write(Arm64Reg::Pc.into(), 0x1000).unwrap();
    let outcome = engine.run(NonZeroU64::new(1).unwrap());
    assert_eq!(outcome.reason, StopReason::BudgetExhausted);
    assert_eq!(engine.reg_read(Arm64Reg::Pc.into()), Ok(0x1004));
}

#[test]
fn instruction_budget_and_resume_are_exact_for_a32() {
    let mut engine = engine();
    engine
        .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 1, 0x10, 0x80, 0xe2])
        .unwrap();
    for expected_pc in [0x1004, 0x1008] {
        let result = engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(result.reason, StopReason::BudgetExhausted);
        assert_eq!(result.instruction_hooks, 1);
        assert_eq!(engine.reg_read(Register::Pc.into()), Ok(expected_pc));
    }
    assert_eq!(engine.reg_read(Register::R0.into()), Ok(42));
    assert_eq!(engine.reg_read(Register::R1.into()), Ok(43));
}

#[test]
fn patching_code_invalidates_translated_blocks() {
    let mut engine = engine();
    engine
        .write_memory(Addr(0x1000), &[1, 0, 0xa0, 0xe3])
        .unwrap();
    engine.run(NonZeroU64::new(1).unwrap());
    assert_eq!(engine.reg_read(Register::R0.into()), Ok(1));
    engine
        .write_memory(Addr(0x1000), &[2, 0, 0xa0, 0xe3])
        .unwrap();
    engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
    engine.run(NonZeroU64::new(1).unwrap());
    assert_eq!(engine.reg_read(Register::R0.into()), Ok(2));
}

#[test]
fn guest_permissions_are_not_bypassed_by_loader_access() {
    let mut engine = UnicornArm::new().unwrap();
    engine
        .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RX)
        .unwrap();
    engine
        .map_memory(Addr(0x2000), PAGE_SIZE, Permissions::R)
        .unwrap();
    // MOV r0,#42; STR r0,[r2]. Loader may initialize RX; guest may not write R.
    engine
        .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 0, 0, 0x82, 0xe5])
        .unwrap();
    engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
    engine.reg_write(Register::R2.into(), 0x2000).unwrap();
    assert_eq!(
        engine.run(NonZeroU64::new(2).unwrap()).reason,
        StopReason::Fault
    );
    let mut data = [0; 4];
    engine.read_memory(Addr(0x2000), &mut data).unwrap();
    assert_eq!(data, [0; 4]);

    let mut no_execute = UnicornArm::new().unwrap();
    no_execute
        .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW)
        .unwrap();
    no_execute
        .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3])
        .unwrap();
    no_execute.reg_write(Register::Pc.into(), 0x1000).unwrap();
    assert_eq!(
        no_execute.run(NonZeroU64::new(1).unwrap()).reason,
        StopReason::Fault
    );
}

#[test]
fn an_infinite_guest_loop_stops_at_the_instruction_budget() {
    let mut engine = engine();
    engine
        .write_memory(Addr(0x1000), &[0xfe, 0xff, 0xff, 0xea])
        .unwrap(); // B .
    let outcome = engine.run(NonZeroU64::new(20).unwrap());
    assert_eq!(outcome.reason, StopReason::BudgetExhausted);
    assert_eq!(outcome.instruction_hooks, 20);
    assert_eq!(engine.reg_read(Register::Pc.into()), Ok(0x1000));
}

#[test]
fn mapped_pages_are_private_to_each_instance() {
    let mut first = engine();
    let mut second = engine();
    first.write_memory(Addr(0x1100), &[1, 2, 3, 4]).unwrap();
    second.write_memory(Addr(0x1100), &[5, 6, 7, 8]).unwrap();
    let mut bytes = [0; 4];
    first.read_memory(Addr(0x1100), &mut bytes).unwrap();
    assert_eq!(bytes, [1, 2, 3, 4]);
    second.read_memory(Addr(0x1100), &mut bytes).unwrap();
    assert_eq!(bytes, [5, 6, 7, 8]);
}

#[test]
fn shared_ram_is_coherent_across_engine_instances() {
    // Design §12.3 rule 1 verification. Unicorn 2.1.x maps host pointers
    // read-only towards the guest, so the shared page is mirrored per
    // engine and synchronised at deterministic boundaries (ADR 0010).
    let ram = SharedRam::new(PAGE_SIZE).unwrap();
    let mut first = UnicornArm::new().unwrap();
    let mut second = UnicornArm::new().unwrap();
    for core in [&mut first, &mut second] {
        core.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        core.map_shared_memory(Addr(0x10000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram))
            .unwrap();
    }
    // MOV r0,#0xAB; LDR r1,[pc,#4]; STRB r0,[r1]; B .; .word 0x00010000
    // The literal sits at +0x10 from the load, so the load uses +4.
    first
        .write_memory(
            Addr(0x1000),
            &[
                0xab, 0, 0xa0, 0xe3, 4, 0x10, 0x9f, 0xe5, 0, 0, 0xc1, 0xe5, 0xfe, 0xff, 0xff, 0xea,
                0, 0, 1, 0,
            ],
        )
        .unwrap();
    first.reg_write(Register::Pc.into(), 0x1000).unwrap();
    let outcome = first.run(NonZeroU64::new(3).unwrap());
    assert_eq!(outcome.reason, StopReason::BudgetExhausted);
    // The guest store lands in the first engine's mirror.
    let mut bytes = [0; 4];
    first.read_memory(Addr(0x10000), &mut bytes).unwrap();
    assert_eq!(bytes[0], 0xab);
    // Copying the mirror back publishes it to the authoritative buffer.
    first.sync_shared_out().unwrap();
    ram.read(0, &mut bytes).unwrap();
    assert_eq!(bytes[0], 0xab);
    // The next core sees it after the copy in.
    second.sync_shared_in().unwrap();
    second.read_memory(Addr(0x10000), &mut bytes).unwrap();
    assert_eq!(bytes[0], 0xab);
    // Host-side writes reach the guests at the next copy in.
    ram.write(8, &[9]).unwrap();
    second.sync_shared_in().unwrap();
    second.read_memory(Addr(0x10008), &mut bytes[..1]).unwrap();
    assert_eq!(bytes[0], 9);
    // A guest write in the second core is published on copy out.
    second.write_memory(Addr(0x10004), &[7]).unwrap();
    second.sync_shared_out().unwrap();
    ram.read(4, &mut bytes[..1]).unwrap();
    assert_eq!(bytes[0], 7);
}

#[test]
fn shared_mappings_validate_like_native_regions() {
    let ram = SharedRam::new(PAGE_SIZE).unwrap();
    let mut engine = UnicornArm::new().unwrap();
    assert!(matches!(
        SharedRam::new(0),
        Err(EngineError::InvalidMapping)
    ));
    // The buffer bounds the mapping size.
    assert_eq!(
        engine.map_shared_memory(
            Addr(0x1000),
            2 * PAGE_SIZE,
            Permissions::RW,
            Arc::clone(&ram)
        ),
        Err(EngineError::InvalidMapping)
    );
    engine
        .map_shared_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram))
        .unwrap();
    assert_eq!(
        engine.map_shared_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram)),
        Err(EngineError::Overlap)
    );
    assert_eq!(
        engine.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW),
        Err(EngineError::Overlap)
    );
    // Loader spans resolve against the shared mapping.
    assert_eq!(
        engine.write_memory(Addr(0x2000), &[1]),
        Err(EngineError::Unmapped)
    );
    assert_eq!(
        ram.write(PAGE_SIZE as usize, &[1]),
        Err(EngineError::InvalidMapping)
    );
    assert_eq!(
        ram.read(1, &mut [0u8; PAGE_SIZE as usize]),
        Err(EngineError::InvalidMapping)
    );
}

#[test]
fn mapping_and_register_errors_do_not_panic() {
    let mut engine = engine();
    assert_eq!(
        engine.map_memory(Addr(1), PAGE_SIZE, Permissions::RW),
        Err(EngineError::InvalidMapping)
    );
    assert_eq!(
        engine.map_memory(Addr(0x2000), 0, Permissions::RW),
        Err(EngineError::InvalidMapping)
    );
    assert_eq!(
        engine.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW),
        Err(EngineError::Overlap)
    );
    assert_eq!(
        engine.map_memory(Addr(0xffff_f000), 2 * PAGE_SIZE, Permissions::RW),
        Err(EngineError::InvalidMapping)
    );
    assert_eq!(
        engine.reg_read(RegId(99)),
        Err(EngineError::InvalidRegister)
    );
    assert_eq!(
        engine.reg_write(Register::R0.into(), 1 << 32),
        Err(EngineError::InvalidValue)
    );
    // A32 state requires 4-byte PC alignment; 2 mod 4 stays invalid.
    assert_eq!(
        engine.reg_write(Register::Pc.into(), 0x1002),
        Err(EngineError::InvalidValue)
    );
    assert_eq!(
        engine.write_memory(Addr(0x2000), &[0]),
        Err(EngineError::Unmapped)
    );
    assert_eq!(
        engine.read_memory(Addr(0x1fff), &mut [0; 2]),
        Err(EngineError::Unmapped)
    );
}

#[test]
fn faults_and_unsupported_budgets_do_not_report_success() {
    let mut engine = UnicornArm::new().unwrap();
    assert_eq!(
        engine.run(NonZeroU64::new(1).unwrap()).reason,
        StopReason::Fault
    );
    assert_eq!(
        engine.run(NonZeroU64::new(MAX_QUANTUM + 1).unwrap()),
        RunOutcome {
            reason: StopReason::CapabilityMissing,
            instruction_hooks: 0,
        }
    );
    assert!(!engine.caps().exclusive_monitor);
    assert!(!engine.caps().precise_exceptions);
}

#[test]
fn a_created_and_used_engine_can_move_between_threads_sequentially() {
    let mut engine = engine();
    engine
        .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 1, 0x10, 0x80, 0xe2])
        .unwrap();
    engine.run(NonZeroU64::new(1).unwrap());
    let mut engine = std::thread::spawn(move || {
        assert_eq!(
            engine.run(NonZeroU64::new(1).unwrap()).reason,
            StopReason::BudgetExhausted
        );
        engine
    })
    .join()
    .unwrap();
    assert_eq!(engine.reg_read(Register::R1.into()), Ok(43));
    engine.write_memory(Addr(0x1100), &[7]).unwrap();
}

#[derive(Default)]
struct BusRecord {
    writes: Vec<(u64, u8, u64)>,
    reads: Vec<(u64, u8)>,
}

struct FakeBus {
    record: Arc<Mutex<BusRecord>>,
    read_value: u64,
    fail_read: Option<BusFault>,
}

impl FakeBus {
    fn new(read_value: u64) -> (Self, Arc<Mutex<BusRecord>>) {
        let record = Arc::new(Mutex::new(BusRecord::default()));
        (
            Self {
                record: Arc::clone(&record),
                read_value,
                fail_read: None,
            },
            record,
        )
    }

    fn failing(record: Arc<Mutex<BusRecord>>, fault: BusFault) -> Self {
        Self {
            record,
            read_value: 0,
            fail_read: Some(fault),
        }
    }
}

impl EngineBus for FakeBus {
    fn read(&mut self, address: Addr, width: AccessSize) -> Result<u64, BusFault> {
        self.record
            .lock()
            .unwrap()
            .reads
            .push((address.0, width.bytes()));
        match self.fail_read {
            Some(fault) => Err(fault),
            None => Ok(self.read_value),
        }
    }

    fn write(&mut self, address: Addr, width: AccessSize, value: u64) -> Result<(), BusFault> {
        self.record
            .lock()
            .unwrap()
            .writes
            .push((address.0, width.bytes(), value));
        Ok(())
    }
}

#[test]
fn mmio_hooks_forward_guest_accesses_to_the_attached_bus() {
    let mut engine = engine();
    let (bus, record) = FakeBus::new(0x0bad_f00d);
    engine.attach_bus(Box::new(bus)).unwrap();
    engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
    // MOV r0,#0x55; STR r0,[r2]; LDR r1,[r2]; STRB r0,[r2,#4]; LDRB r3,[r2,#4]
    engine
        .write_memory(
            Addr(0x1000),
            &[
                0x55, 0, 0xa0, 0xe3, 0, 0, 0x82, 0xe5, 0, 0x10, 0x92, 0xe5, 4, 0, 0xc2, 0xe5, 4,
                0x30, 0xd2, 0xe5,
            ],
        )
        .unwrap();
    engine.reg_write(Register::R2.into(), 0x2000).unwrap();
    let outcome = engine.run(NonZeroU64::new(5).unwrap());
    assert_eq!(outcome.reason, StopReason::BudgetExhausted);
    assert_eq!(outcome.instruction_hooks, 5);
    assert_eq!(engine.reg_read(Register::R0.into()), Ok(0x55));
    assert_eq!(engine.reg_read(Register::R1.into()), Ok(0x0bad_f00d));
    // LDRB keeps the low byte of the bus value.
    assert_eq!(engine.reg_read(Register::R3.into()), Ok(0x0d));
    let record = record.lock().unwrap();
    assert_eq!(record.writes, vec![(0x2000, 4, 0x55), (0x2004, 1, 0x55)]);
    assert_eq!(record.reads, vec![(0x2000, 4), (0x2004, 1)]);
    assert!(engine.take_bus_fault().is_none());
}

#[test]
fn mmio_bus_fault_stops_emulation_and_is_reported_once() {
    let mut engine = engine();
    let record = Arc::new(Mutex::new(BusRecord::default()));
    let bus = FakeBus::failing(Arc::clone(&record), BusFault::Device);
    engine.attach_bus(Box::new(bus)).unwrap();
    engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
    // LDR r1,[r2] is the first instruction.
    engine
        .write_memory(Addr(0x1000), &[0, 0x10, 0x92, 0xe5])
        .unwrap();
    engine.reg_write(Register::R2.into(), 0x2000).unwrap();
    let outcome = engine.run(NonZeroU64::new(4).unwrap());
    assert_eq!(outcome.reason, StopReason::Fault);
    assert_eq!(
        engine.take_bus_fault(),
        Some(EngineBusFault::new(Addr(0x2000), 4, BusFault::Device))
    );
    assert!(engine.take_bus_fault().is_none());
    assert_eq!(record.lock().unwrap().reads, vec![(0x2000, 4)]);
}

#[test]
fn mmio_registration_rejects_misuse_and_stays_out_of_loader_spans() {
    let mut engine = UnicornArm::new().unwrap();
    assert_eq!(
        engine.map_mmio(Addr(0x2000), PAGE_SIZE),
        Err(EngineError::BusNotAttached)
    );
    let (bus, _) = FakeBus::new(0);
    engine.attach_bus(Box::new(bus)).unwrap();
    assert_eq!(
        engine.attach_bus(Box::new(FakeBus::new(0).0)),
        Err(EngineError::BusAlreadyAttached)
    );
    assert_eq!(
        engine.map_mmio(Addr(1), PAGE_SIZE),
        Err(EngineError::InvalidMapping)
    );
    assert_eq!(
        engine.map_mmio(Addr(0x2000), 0),
        Err(EngineError::InvalidMapping)
    );
    engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
    assert_eq!(
        engine.map_mmio(Addr(0x2000), PAGE_SIZE),
        Err(EngineError::Overlap)
    );
    assert_eq!(
        engine.map_memory(Addr(0x2000), PAGE_SIZE, Permissions::RW),
        Err(EngineError::Overlap)
    );
    // Callback regions are not reachable through the loader memory API.
    assert_eq!(
        engine.write_memory(Addr(0x2000), &[1]),
        Err(EngineError::Unmapped)
    );
}

#[test]
fn set_line_records_edges_and_rejects_unknown_arm_lines() {
    let mut engine = UnicornArm::new().unwrap();
    assert!(!engine.caps().line_injection);
    engine.set_line(ARM_IRQ, true).unwrap();
    engine.set_line(ARM_FIQ, true).unwrap();
    engine.set_line(ARM_IRQ, false).unwrap();
    assert_eq!(
        engine.irq_edges(),
        &[(ARM_IRQ, true), (ARM_FIQ, true), (ARM_IRQ, false)]
    );
    // The contract view exposes the same log.
    assert_eq!(CpuEngine::line_edges(&engine).len(), 3);
    assert_eq!(engine.irq_level(ARM_IRQ), Some(false));
    assert_eq!(
        engine.set_line(IrqLine(9), true),
        Err(EngineError::LineNotSupported)
    );
}

#[cfg(feature = "arch-riscv")]
mod riscv32 {
    use super::*;
    use mosaic_isa_riscv::Register as RvReg;

    fn engine() -> UnicornRiscV32 {
        let mut engine = UnicornRiscV32::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        engine.reg_write(RvReg::Pc.into(), 0x1000).unwrap();
        engine
    }

    #[test]
    fn instruction_budget_and_resume_are_exact_for_rv32i() {
        let mut engine = engine();
        // addi x5,x0,42; addi x6,x5,1; addi x0,x0,5 (x0 stays hardwired)
        engine
            .write_memory(
                Addr(0x1000),
                &[
                    0x93, 0x02, 0xa0, 0x02, 0x13, 0x83, 0x12, 0x00, 0x13, 0x00, 0x50, 0x00,
                ],
            )
            .unwrap();
        for expected_pc in [0x1004, 0x1008] {
            let result = engine.run(NonZeroU64::new(1).unwrap());
            assert_eq!(result.reason, StopReason::BudgetExhausted);
            assert_eq!(result.instruction_hooks, 1);
            assert_eq!(engine.reg_read(RvReg::Pc.into()), Ok(expected_pc));
        }
        assert_eq!(engine.reg_read(RvReg::X5.into()), Ok(42));
        assert_eq!(engine.reg_read(RvReg::X6.into()), Ok(43));
        let _ = engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(engine.reg_read(RvReg::X0.into()), Ok(0));
    }

    #[test]
    fn mmio_and_shared_paths_match_the_arm_adapter() {
        let mut engine = engine();
        let (bus, record) = FakeBus::new(0x0bad_f00d);
        engine.attach_bus(Box::new(bus)).unwrap();
        engine.map_mmio(Addr(0x2000), PAGE_SIZE).unwrap();
        let ram = SharedRam::new(PAGE_SIZE).unwrap();
        engine
            .map_shared_memory(Addr(0x10000), PAGE_SIZE, Permissions::RW, Arc::clone(&ram))
            .unwrap();
        // lw x6,0(x5); sw x6,0(x7): x5 = MMIO base, x7 = shared base.
        engine
            .write_memory(
                Addr(0x1000),
                &[0x03, 0xa3, 0x02, 0x00, 0x23, 0xa0, 0x63, 0x00],
            )
            .unwrap();
        engine.reg_write(RvReg::X5.into(), 0x2000).unwrap();
        engine.reg_write(RvReg::X7.into(), 0x10000).unwrap();
        let outcome = engine.run(NonZeroU64::new(2).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(engine.reg_read(RvReg::X6.into()), Ok(0x0bad_f00d));
        assert_eq!(record.lock().unwrap().reads, vec![(0x2000, 4)]);
        // The shared page is mirrored: the guest store lands in the
        // engine's copy and reaches the authoritative buffer on sync.
        let mut bytes = [0; 4];
        engine.read_memory(Addr(0x10000), &mut bytes).unwrap();
        assert_eq!(bytes, 0x0bad_f00du32.to_le_bytes());
        engine.sync_shared_out().unwrap();
        ram.read(0, &mut bytes).unwrap();
        assert_eq!(bytes, 0x0bad_f00du32.to_le_bytes());
    }

    #[test]
    fn lines_and_registers_reject_out_of_vocabulary_input() {
        let mut engine = UnicornRiscV32::new().unwrap();
        engine.set_line(RISCV_MEXT, true).unwrap();
        assert_eq!(CpuEngine::line_edges(&engine), &[(RISCV_MEXT, true)]);
        assert_eq!(
            engine.set_line(IrqLine(1), true),
            Err(EngineError::LineNotSupported)
        );
        assert_eq!(
            engine.reg_read(RegId(33)),
            Err(EngineError::InvalidRegister)
        );
        assert_eq!(
            engine.reg_write(RvReg::Pc.into(), 0x1002),
            Err(EngineError::InvalidValue)
        );
        assert_eq!(
            engine.reg_write(RvReg::X5.into(), 1u64 << 32),
            Err(EngineError::InvalidValue)
        );
    }

    #[test]
    fn the_register_table_matches_the_backend_enum() {
        for (index, register) in RISCV_GENERAL.iter().enumerate() {
            assert_eq!(
                *register as i32,
                RegisterRISCV::X0 as i32 + index as i32,
                "X{index} must stay contiguous in uc_riscv_reg"
            );
        }
    }
}
