#![forbid(unsafe_code)]
//! Machine-readable run report, for the boot-test harness and for scripting.
//!
//! Hand-rolled JSON emission (no external dependencies in this crate): the
//! report is a flat structure of strings, numbers and string arrays, and the
//! emitter below keeps it that way by construction.

#[cfg(feature = "unicorn")]
use crate::PcBootReport;

/// Escape one string into a JSON string literal (without the quotes).
fn escape_into(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
}

fn quoted(out: &mut String, text: &str) {
    out.push('"');
    escape_into(out, text);
    out.push('"');
}

fn string_field(out: &mut String, name: &str, value: &str, first: bool) {
    if !first {
        out.push(',');
    }
    quoted(out, name);
    out.push(':');
    quoted(out, value);
}

fn number_field(out: &mut String, name: &str, value: u64, first: bool) {
    if !first {
        out.push(',');
    }
    quoted(out, name);
    out.push(':');
    out.push_str(&value.to_string());
}

fn string_array_field(out: &mut String, name: &str, values: &[String], first: bool) {
    if !first {
        out.push(',');
    }
    quoted(out, name);
    out.push_str(":[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        quoted(out, value);
    }
    out.push(']');
}

fn bytes_field(out: &mut String, name: &str, bytes: &[u8], first: bool) {
    if !first {
        out.push(',');
    }
    quoted(out, name);
    out.push(':');
    // Serial consoles are text in practice; lossy conversion keeps the JSON
    // valid whatever the guest emitted, and a byte count records the rest.
    out.push('"');
    let text = String::from_utf8_lossy(bytes);
    escape_into(out, &text);
    out.push('"');
}

/// Render a PC run as one JSON object. `serial_tail`/`screen_tail` bound the
/// largest fields so a runaway guest cannot produce a gigabyte report; the
/// numbers and stop diagnostics are always complete.
#[cfg(feature = "unicorn")]
pub fn pc_boot_report(report: &PcBootReport, serial_tail: usize, screen_tail: usize) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(2048);
    out.push('{');
    string_field(&mut out, "machine", "pc", true);
    number_field(&mut out, "instructions", report.instructions, false);
    string_field(&mut out, "stop_reason", &report.stop_reason, false);
    number_field(&mut out, "eip", report.eip, false);
    number_field(&mut out, "cs", report.cs, false);
    number_field(&mut out, "cr0", report.cr0, false);
    let screen = if report.screen.len() > screen_tail {
        &report.screen[report.screen.len() - screen_tail..]
    } else {
        &report.screen
    };
    string_array_field(&mut out, "screen", screen, false);
    let serial = if report.serial.len() > serial_tail {
        &report.serial[report.serial.len() - serial_tail..]
    } else {
        &report.serial
    };
    bytes_field(&mut out, "serial", serial, false);
    if let Some(vector) = report.unhandled_interrupt {
        number_field(&mut out, "unhandled_interrupt", u64::from(vector), false);
    }
    if let Some(vector) = report.unclaimed_interrupt {
        number_field(&mut out, "unclaimed_interrupt", u64::from(vector), false);
    }
    if let Some((vector, eip, cr2)) = report.exception {
        if !out.ends_with('{') {
            out.push(',');
        }
        let _ = write!(
            out,
            "\"exception\":{{\"vector\":{vector},\"eip\":{eip},\"cr2\":{cr2}}}"
        );
    }
    if let Some((is_write, address, size)) = report.unmapped {
        if !out.ends_with('{') {
            out.push(',');
        }
        let _ = write!(
            out,
            "\"unmapped\":{{\"write\":{is_write},\"address\":{address},\"size\":{size}}}"
        );
    }
    if let Some(error) = &report.engine_error {
        string_field(&mut out, "engine_error", error, false);
    }
    if !report.last_instruction.is_empty() {
        let hex: String = report
            .last_instruction
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join("");
        string_field(&mut out, "last_instruction", &hex, false);
    }
    if let Some(fault) = &report.bus_fault {
        string_field(&mut out, "bus_fault", fault, false);
    }
    if let Some(kernel) = &report.kernel {
        if !out.ends_with('{') {
            out.push(',');
        }
        out.push_str("\"kernel\":{");
        string_field(&mut out, "version", &kernel.version, true);
        string_field(&mut out, "protocol", &kernel.protocol, false);
        number_field(
            &mut out,
            "setup_sects",
            u64::from(kernel.setup_sects),
            false,
        );
        number_field(
            &mut out,
            "payload_bytes",
            kernel.payload_bytes as u64,
            false,
        );
        number_field(&mut out, "initrd_bytes", kernel.initrd_bytes as u64, false);
        let _ = write!(&mut out, ",\"loaded_high\":{}", kernel.loaded_high);
        out.push('}');
    }
    if !report.disk_reads.is_empty() {
        if !out.ends_with('{') {
            out.push(',');
        }
        out.push_str("\"disk_reads\":[");
        for (index, read) in report.disk_reads.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"drive\":{},\"lba\":{},\"blocks\":{},\"buffer\":{}}}",
                read.drive, read.lba, read.blocks, read.buffer
            );
        }
        out.push(']');
    }
    if !report.interrupts.is_empty() {
        if !out.ends_with('{') {
            out.push(',');
        }
        out.push_str("\"interrupts\":{");
        for (index, (vector, count)) in report.interrupts.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "\"{vector:#04x}\":{count}");
        }
        out.push('}');
    }
    out.push('}');
    out
}
