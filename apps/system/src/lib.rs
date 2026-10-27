#![forbid(unsafe_code)]
//! The system front-end: the QEMU-shaped entry point of Mosaic.
//!
//! `mosaic-system -M pc -cdrom os.iso -m 64M -serial - -display none` selects a
//! machine from the registry, attaches real drive images the way a PC BIOS
//! numbers them, and boots whatever the images actually contain. There is no
//! synthetic boot path in this crate: every run needs a real image, exactly
//! like selecting a CD in a system emulator.
//!
//! This crate is QEMU's `softmmu/vl.c` and nothing more: it parses options,
//! lists machines and translates a [`SystemConfig`] into the selected
//! machine's own configuration type. The machines themselves are catalog
//! board models (ADR 0023) — `pc` lives in `catalog/board/ibm/pc`
//! (`mosaic-board-ibm-pc-glue`) — and each new machine (`virt-64`, `goldfish`,
//! `versatile-pb`, ...) is a new board crate plus one arm in the dispatch
//! below, the way QEMU's `hw/<arch>` machines register `MODULE_INFO` without
//! `vl.c` growing machine code.

pub mod report_json;

#[cfg(feature = "unicorn")]
pub use mosaic_board_ibm_pc_glue::{run_pc_machine, PcBootOptions, PcBootReport, PcMachineConfig};

/// A kernel the operator asked the machine to boot directly, bypassing every
/// loader in the attached images (QEMU's `-kernel`/`-initrd`/`-append`).
#[derive(Clone, Debug)]
pub struct DirectKernel {
    /// `bzImage` bytes.
    pub image: Vec<u8>,
    /// Optional initramfs bytes.
    pub initrd: Option<Vec<u8>>,
    /// Kernel command line.
    pub cmdline: String,
}

/// One machine type the front-end knows how to build, the way QEMU's `-M`
/// names a board. The machine's implementation is a catalog board crate;
/// an arm here only translates options and calls into it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineKind {
    /// `board:ibm/pc`: BIOS software-interrupt firmware interim, PIC/PIT/RTC
    /// port space, VGA text console. Boots El Torito CDs, boot sectors and
    /// `bzImage` kernels through the documented i386 boot protocol.
    Pc,
    // `Virt64` (AArch64 `virt`) is planned: pl011 + GICv2 + direct kernel
    // boot. It registers here once a real image boots on it, not before.
}

impl MachineKind {
    /// The `-M` spelling of this machine.
    pub fn name(self) -> &'static str {
        match self {
            MachineKind::Pc => mosaic_board_ibm_pc_glue::MACHINE_NAME,
        }
    }

    /// One-line description, as `mosaic-system -M help` prints it.
    pub fn summary(self) -> &'static str {
        match self {
            MachineKind::Pc => mosaic_board_ibm_pc_glue::MACHINE_SUMMARY,
        }
    }

    /// CPU architectures the machine's engines cover, as `-M help` prints it.
    pub fn architectures(self) -> &'static [&'static str] {
        match self {
            MachineKind::Pc => mosaic_board_ibm_pc_glue::ARCHITECTURES,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "pc" => Some(MachineKind::Pc),
            _ => None,
        }
    }
}

/// All machines, in `-M help` order.
pub fn machines() -> Vec<MachineKind> {
    vec![MachineKind::Pc]
}

/// A parse failure that carries the whole machine list, so `-M help` is one
/// error message away for any typo.
#[derive(Debug)]
pub struct UnknownMachine {
    pub name: String,
}

impl std::fmt::Display for UnknownMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown machine '{}'", self.name)
    }
}

impl TryFrom<&str> for MachineKind {
    type Error = UnknownMachine;

    fn try_from(name: &str) -> Result<Self, UnknownMachine> {
        MachineKind::from_name(name).ok_or(UnknownMachine { name: name.into() })
    }
}

/// The media kind of one attached drive, as the CLI spelled it. The machine
/// decides what BIOS drive number and geometry each kind receives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Floppy,
    HardDisk,
    Cdrom,
}

/// One drive image: bytes plus the role the user attached it as.
#[derive(Clone, Debug)]
pub struct DriveSpec {
    pub media: MediaKind,
    pub data: Vec<u8>,
}

/// How the machine picks what to execute first.
#[derive(Clone, Debug)]
pub enum BootSource {
    /// QEMU `-boot` order: `'c'` first hard disk, `'d'` first CD-ROM,
    /// `'a'` first floppy. The first device with a bootable image wins.
    Order(String),
    /// QEMU `-kernel`: boot a `bzImage` (with optional initramfs and command
    /// line) directly, bypassing every boot loader in the image.
    DirectKernel(DirectKernel),
}

/// Everything one run needs, after the CLI resolved file paths to bytes.
#[derive(Clone, Debug)]
pub struct SystemConfig {
    pub machine: MachineKind,
    /// Total guest RAM in KiB (`-m`).
    pub ram_kib: u64,
    /// The microarchitecture the CPU impersonates (`-cpu`). At the `lib`
    /// boundary this stays machine-neutral; each machine maps it to its own
    /// engine's model list.
    pub cpu_model: CpuModel,
    pub drives: Vec<DriveSpec>,
    pub boot: BootSource,
    #[cfg(feature = "unicorn")]
    pub options: PcBootOptions,
}

/// The `-cpu` a run asked for, machine-neutral. Machines translate this into
/// their own engine's vocabulary and may report `CapabilityMissing` for one
/// they cannot impersonate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuModel {
    /// The engine default: good for 16-bit/32-bit era guests, but a kernel
    /// built for `x86-64-v2` faults on its first BMI2 instruction.
    Default,
    /// A core with BMI2/AVX2 (`x86-64-v2` baseline and above).
    Haswell,
    /// A newer client core, for guests probing `x86-64-v3`-era features.
    Skylake,
}

/// The result of one run, machine-neutral on the surface: the fields every
/// machine fills in. PC fills all of them from its own report today.
#[derive(Clone, Debug)]
pub struct SystemReport {
    pub machine: &'static str,
    #[cfg(feature = "unicorn")]
    pub pc: PcBootReport,
}

impl SystemReport {
    /// Serial bytes the guest emitted, whatever machine produced them.
    #[cfg(feature = "unicorn")]
    pub fn serial(&self) -> &[u8] {
        &self.pc.serial
    }

    /// Text console lines at the end of the run.
    #[cfg(feature = "unicorn")]
    pub fn screen(&self) -> &[String] {
        &self.pc.screen
    }

    /// Every output channel joined, for pattern assertions that do not care
    /// which console a guest chose.
    #[cfg(feature = "unicorn")]
    pub fn text(&self) -> String {
        self.pc.text()
    }

    /// The run's own stop classification.
    #[cfg(feature = "unicorn")]
    pub fn stop_reason(&self) -> &str {
        &self.pc.stop_reason
    }

    /// Instructions the guest executed.
    #[cfg(feature = "unicorn")]
    pub fn instructions(&self) -> u64 {
        self.pc.instructions
    }
}

/// Run one machine with the attached images. This is the whole front-end: no
/// hidden fixtures, no fallbacks — a missing image is an error, exactly like a
/// system emulator refusing to open a file.
pub fn run(config: &SystemConfig) -> Result<SystemReport, String> {
    match config.machine {
        MachineKind::Pc => {
            #[cfg(feature = "unicorn")]
            {
                let machine = to_pc_config(config)?;
                let pc = run_pc_machine(&machine, &config.options)?;
                Ok(SystemReport {
                    machine: config.machine.name(),
                    pc,
                })
            }
            #[cfg(not(feature = "unicorn"))]
            {
                let _ = config;
                Err(
                    "rebuild with --features unicorn: the PC machine needs the Unicorn engine"
                        .into(),
                )
            }
        }
    }
}

#[cfg(feature = "unicorn")]
fn to_pc_config(config: &SystemConfig) -> Result<PcMachineConfig, String> {
    let cpu_model = match config.cpu_model {
        CpuModel::Default => mosaic_engine_unicorn::X86CpuModel::Qemu64,
        CpuModel::Haswell => mosaic_engine_unicorn::X86CpuModel::Haswell,
        CpuModel::Skylake => mosaic_engine_unicorn::X86CpuModel::SkylakeClient,
    };
    let mut machine = PcMachineConfig::new(config.ram_kib)
        .with_cpu_model(cpu_model)
        .clone();
    for drive in &config.drives {
        match drive.media {
            MediaKind::Floppy => {
                machine.attach_floppy(drive.data.clone());
            }
            MediaKind::HardDisk => {
                machine.attach_hard_disk(drive.data.clone());
            }
            MediaKind::Cdrom => {
                machine.attach_cdrom(drive.data.clone())?;
            }
        }
    }
    match &config.boot {
        BootSource::Order(order) => {
            machine.with_boot_order(order.clone());
        }
        BootSource::DirectKernel(kernel) => {
            machine.with_kernel(mosaic_board_ibm_pc_glue::DirectKernel {
                image: kernel.image.clone(),
                initrd: kernel.initrd.clone(),
                cmdline: kernel.cmdline.clone(),
            });
        }
    }
    Ok(machine)
}
