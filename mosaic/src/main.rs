#![forbid(unsafe_code)]
//! `mosaic` — the QEMU-shaped command line: pick a machine, attach
//! real images, boot them. See `--help` for the spelling of every option.

#[cfg(feature = "unicorn")]
use mosaic::{run, BootSource, DriveSpec, MachineKind, MediaKind, RunOptions, SystemConfig};

const USAGE: &str = "\
mosaic — Mosaic system emulator front-end

usage: mosaic [options]

machine selection:
   -M, --machine <name>    machine type; '-M help' lists them (default: pc)
   -m, --memory <size>     guest RAM: 64M, 512M, 65536K (default: 64M;
                           machines with a fixed memory map ignore it)

drives (real images only; nothing here synthesises one):
   -fda <file>             floppy image (BIOS drive 0x00)
   -fdb <file>             second floppy (BIOS drive 0x01)
   -hda <file>             first hard disk (BIOS drive 0x80)
   -hdb <file>             second hard disk (BIOS drive 0x81)
   -cdrom <file>           CD-ROM image (BIOS drive 0x9F)

boot:
   -boot <order>           boot order over 'c' disk, 'd' cdrom, 'a' floppy
                           (default: dca — CD first, like a fresh machine)
   -kernel <file>          boot a kernel image directly, bypassing the image's
                           own loaders. bzImage on the PC, a raw vector-table
                           image on Cortex-M machines. '<iso>!<inner-path>'
                           extracts from an ISO.
   -initrd <file>          initramfs for -kernel (same ISO syntax)
   -append <cmdline>       kernel command line

run control:
   --quantum <n>           instructions per scheduling quantum (default 50000)
   --instruction-limit <n> total instruction budget (default 200000000)
   --no-reboot             a guest reset request stops the run (always on)

output:
   -serial <file>          serial output to file ('stdio' or '-' = stdout,
                           'null' = discard; default: none)
   -display <mode>         'none' (default) or 'text' (print VGA screen at exit)
   --screen-dump <file>    write the final VGA text screen to a file
   --report <file>         machine-readable JSON run report ('-' = stdout)
   --trace <n>             record the last n instructions (diagnostics)
   --watch <base>:<len>    record stores into a guest region (repeatable)
   --dump <base>:<len>     copy a guest region into the report (repeatable)
   -q, --quiet             suppress the human-readable summary
   -h, --help              this text

examples:
   mosaic -M pc -cdrom os.iso -m 64M -serial stdio
   mosaic -cdrom os.iso -kernel os.iso!/boot/vmlinuz -append 'console=ttyS0'
   mosaic -fda boot.img -boot a -display text
";

#[cfg(feature = "unicorn")]
struct CommandLine {
    machine: String,
    ram_mib: u64,
    cpu: mosaic::CpuModel,
    drives: Vec<(MediaKind, String)>,
    boot_order: Option<String>,
    kernel: Option<String>,
    initrd: Option<String>,
    append: Option<String>,
    serial: Option<String>,
    display: Option<String>,
    screen_dump: Option<String>,
    report: Option<String>,
    quiet: bool,
    options: RunOptions,
}

fn fail_usage(message: &str) -> ! {
    eprintln!("mosaic: {message}\ntry 'mosaic --help'");
    std::process::exit(2);
}

#[cfg(feature = "unicorn")]
fn parse_size_bytes(text: &str, what: &str) -> u64 {
    let value = text.trim();
    let (digits, scale) = match value.chars().last() {
        Some('K' | 'k') => (&value[..value.len() - 1], 1024u64),
        Some('M' | 'm') => (&value[..value.len() - 1], 1024 * 1024),
        Some('G' | 'g') => (&value[..value.len() - 1], 1024 * 1024 * 1024),
        _ => (value, 1),
    };
    let Ok(bytes) = digits.trim().parse::<u64>() else {
        fail_usage(&format!("--{what}: cannot parse size '{text}'"));
    };
    bytes * scale
}

#[cfg(feature = "unicorn")]
fn parse_number(text: &str, what: &str) -> u64 {
    let value = text.trim();
    let parsed = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else {
        value.parse::<u64>().ok()
    };
    parsed.unwrap_or_else(|| fail_usage(&format!("--{what}: cannot parse number '{text}'")))
}

#[cfg(feature = "unicorn")]
fn parse_region(text: &str, what: &str) -> (u64, u64) {
    let Some((base, length)) = text.split_once(':') else {
        fail_usage(&format!("--{what}: expected <base>:<length>, got '{text}'"));
    };
    (parse_number(base, what), parse_number(length, what).max(1))
}

#[cfg(feature = "unicorn")]
fn parse_args() -> CommandLine {
    let mut command = CommandLine {
        machine: "pc".into(),
        ram_mib: 64,
        cpu: mosaic::CpuModel::Haswell,
        drives: Vec::new(),
        boot_order: None,
        kernel: None,
        initrd: None,
        append: None,
        serial: None,
        display: None,
        screen_dump: None,
        report: None,
        quiet: false,
        options: RunOptions::default(),
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) => (name.to_string(), Some(value.to_string())),
            None => (argument.clone(), None),
        };
        let mut value = |what: &str| -> String {
            if let Some(value) = inline.clone() {
                return value;
            }
            arguments
                .next()
                .unwrap_or_else(|| fail_usage(&format!("{what} needs a value")))
        };
        match name.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-M" | "--machine" => {
                let machine = value("machine");
                if machine == "help" {
                    println!("machines:");
                    for machine in mosaic::machines() {
                        println!(
                            "{:<12} {} [{}]",
                            machine.name(),
                            machine.summary(),
                            machine.architectures().join(" ")
                        );
                    }
                    std::process::exit(0);
                }
                if MachineKind::try_from(machine.as_str()).is_err() {
                    eprintln!("mosaic: unknown machine '{machine}' (try -M help)");
                    std::process::exit(2);
                }
                command.machine = machine;
            }
            "-m" | "--memory" => {
                let text = value("memory");
                command.ram_mib = parse_size_bytes(&text, "memory") / (1024 * 1024);
                if command.ram_mib == 0 {
                    fail_usage("--memory: below one mebibyte");
                }
            }
            "-fda" => command.drives.push((MediaKind::Floppy, value("-fda"))),
            "-fdb" => command.drives.push((MediaKind::Floppy, value("-fdb"))),
            "-hda" => command.drives.push((MediaKind::HardDisk, value("-hda"))),
            "-hdb" => command.drives.push((MediaKind::HardDisk, value("-hdb"))),
            "-cdrom" => command.drives.push((MediaKind::Cdrom, value("-cdrom"))),
            "-boot" => command.boot_order = Some(value("-boot")),
            "-cpu" => {
                let cpu = value("-cpu");
                command.cpu = match cpu.as_str() {
                    "help" => {
                        println!("cpus (pc): default (qemu64-class), haswell (BMI2/AVX2), skylake");
                        std::process::exit(0);
                    }
                    "default" | "qemu64" => mosaic::CpuModel::Default,
                    "haswell" => mosaic::CpuModel::Haswell,
                    "skylake" => mosaic::CpuModel::Skylake,
                    other => fail_usage(&format!("unknown cpu '{other}' (try -cpu help)")),
                };
            }
            "-kernel" => command.kernel = Some(value("-kernel")),
            "-initrd" => command.initrd = Some(value("-initrd")),
            "-append" => command.append = Some(value("-append")),
            "-serial" => command.serial = Some(value("-serial")),
            "-display" => command.display = Some(value("-display")),
            "--screen-dump" => command.screen_dump = Some(value("--screen-dump")),
            "--report" => command.report = Some(value("--report")),
            "-q" | "--quiet" => command.quiet = true,
            "--no-reboot" | "-no-reboot" => {}
            "--quantum" => {
                command.options.quantum = parse_number(&value("--quantum"), "quantum").max(1)
            }
            "--instruction-limit" => {
                command.options.instruction_limit =
                    parse_number(&value("--instruction-limit"), "instruction-limit").max(1)
            }
            "--trace" => {
                command.options.trace_instructions =
                    parse_number(&value("--trace"), "trace").max(1) as usize
            }
            "--watch" => {
                let (base, length) = parse_region(&value("--watch"), "watch");
                command.options.watch_writes.push((base, length));
            }
            "--dump" => {
                let (base, length) = parse_region(&value("--dump"), "dump");
                command.options.dumps.push((base, length as usize));
            }
            other => fail_usage(&format!("unknown option {other}")),
        }
    }
    if command.drives.is_empty() && command.kernel.is_none() {
        fail_usage(
            "no boot source: attach an image (-cdrom, -hda, -fda) or name a kernel (-kernel)",
        );
    }
    if command.kernel.is_none() && command.initrd.is_some() {
        fail_usage("-initrd needs -kernel");
    }
    command
}

/// Read a whole file, with the error text the front-end owes the operator.
#[cfg(feature = "unicorn")]
fn read_image(path: &str, what: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{what} '{path}': {error}"))
}

/// `-kernel`/`-initrd` operands name either a plain file or `<iso>!<inner>`
/// for a file inside an ISO9660 image — the same trick a workflow uses when
/// it boots the kernel out of the distribution CD it just downloaded.
#[cfg(feature = "unicorn")]
fn read_kernel_operand(text: &str, what: &str) -> Result<Vec<u8>, String> {
    if let Some((iso_path, inner)) = text.split_once('!') {
        let iso = read_image(iso_path, what)?;
        let file = mosaic_block::iso9660::iso_file(&iso, inner)
            .ok_or_else(|| format!("{what}: '{inner}' is not a file in {iso_path}"))?;
        return file
            .read(&iso)
            .ok_or_else(|| format!("{what}: '{inner}' runs past the end of {iso_path}"));
    }
    read_image(text, what)
}

#[cfg(not(feature = "unicorn"))]
fn main() {
    eprintln!("mosaic: rebuild with --features unicorn");
    std::process::exit(1);
}

#[cfg(feature = "unicorn")]
fn main() -> std::process::ExitCode {
    let command = parse_args();
    let Ok(machine) = MachineKind::try_from(command.machine.as_str()) else {
        eprintln!(
            "mosaic: unknown machine '{}' (try -M help)",
            command.machine
        );
        return std::process::ExitCode::from(2);
    };
    let mut drives = Vec::new();
    for (media, path) in &command.drives {
        let data = match read_image(path, "drive") {
            Ok(data) => data,
            Err(error) => {
                eprintln!("mosaic: {error}");
                return std::process::ExitCode::FAILURE;
            }
        };
        drives.push(DriveSpec {
            media: *media,
            data,
        });
    }
    let boot = if let Some(kernel) = &command.kernel {
        let image = match read_kernel_operand(kernel, "-kernel") {
            Ok(image) => image,
            Err(error) => {
                eprintln!("mosaic: {error}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let initrd = match &command.initrd {
            Some(path) => match read_kernel_operand(path, "-initrd") {
                Ok(data) => Some(data),
                Err(error) => {
                    eprintln!("mosaic: {error}");
                    return std::process::ExitCode::FAILURE;
                }
            },
            None => None,
        };
        BootSource::DirectKernel(mosaic::DirectKernel {
            image,
            initrd,
            cmdline: command.append.clone().unwrap_or_default(),
        })
    } else {
        BootSource::Order(command.boot_order.clone().unwrap_or_else(|| "dca".into()))
    };
    let config = SystemConfig {
        machine,
        // `-m` counts the whole machine RAM; the PC splits it into the low
        // megabyte and extended memory itself.
        ram_kib: command.ram_mib * 1024,
        cpu_model: command.cpu,
        drives,
        boot,
        options: command.options.clone(),
    };
    let report = match run(&config) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("mosaic: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };

    summarise(&command, &report)
}

#[cfg(feature = "unicorn")]
fn summarise(command: &CommandLine, report: &mosaic::SystemReport) -> std::process::ExitCode {
    let serial_text = String::from_utf8_lossy(report.serial());
    if let Some(destination) = &command.serial {
        match destination.as_str() {
            "stdio" | "-" => {
                print!("{serial_text}");
            }
            "null" | "none" => {}
            path => {
                if let Err(error) = std::fs::write(path, report.serial()) {
                    eprintln!("mosaic: -serial '{path}': {error}");
                }
            }
        }
    }
    if command.display.as_deref() == Some("text") {
        for line in report.screen() {
            println!("{line}");
        }
    }
    if let Some(path) = &command.screen_dump {
        let text = report.screen().join("\n");
        if let Err(error) = std::fs::write(path, format!("{text}\n")) {
            eprintln!("mosaic: --screen-dump '{path}': {error}");
        }
    }
    if let Some(destination) = &command.report {
        let Some(pc) = report.pc() else {
            eprintln!("mosaic: --report is only implemented for the pc machine");
            return std::process::ExitCode::FAILURE;
        };
        let json = mosaic::report_json::pc_boot_report(pc, 64 * 1024, 64);
        match destination.as_str() {
            "-" => println!("{json}"),
            path => {
                if let Err(error) = std::fs::write(path, json) {
                    eprintln!("mosaic: --report '{path}': {error}");
                }
            }
        }
    }
    if !command.quiet {
        eprintln!(
            "mosaic: machine {} stopped: {} after {} instructions",
            report.machine,
            report.stop_reason(),
            report.instructions()
        );
        if report.failed() {
            eprintln!("mosaic: {}", report.diagnosis());
        }
    }
    // A run that exhausted its budget, or a guest that parked itself in a
    // loop waiting for hardware this machine does not model yet, is a normal
    // stop: the report is the product. Machine-reported faults are failures.
    if report.failed() {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
