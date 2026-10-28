#![forbid(unsafe_code)]
//! The system front-end: the QEMU-shaped entry point of Mosaic.
//!
//! `mosaic -M pc -cdrom os.iso -m 64M -serial - -display none` selects a
//! machine from the registry, attaches real drive images the way a PC BIOS
//! numbers them, and boots whatever the images actually contain. There is no
//! synthetic boot path in this crate: every run needs a real image, exactly
//! like selecting a CD in a system emulator.
//!
//! This crate is QEMU's `system/vl.c` and nothing more: it parses options,
//! lists machines and translates a [`SystemConfig`] into the selected
//! machine's own configuration type. The machines themselves live in the
//! `machine` crate by architecture (`machine/src/i386/`, `machine/src/arm/`,
//! ... — QEMU's `hw/<arch>`), and each new machine is one module there plus
//! one dispatch arm below and one entry in its registry, the way QEMU's
//! `hw/<arch>` machines register `MODULE_INFO` without `vl.c` growing
//! machine code (ADR 0021/0023/0024).

#[cfg(feature = "unicorn")]
pub mod report_json;

pub use mosaic_machine::arm::stm32f103::Stm32F103Report;
pub use mosaic_machine::arm::virt64::Virt64Report;
#[cfg(feature = "unicorn")]
pub use mosaic_machine::i386::pc::{run_pc_machine, PcBootOptions, PcBootReport, PcMachineConfig};
pub use mosaic_machine::MachineInfo;

/// A kernel the operator asked the machine to boot directly, bypassing every
/// loader in the attached images (QEMU's `-kernel`/`-initrd`/`-append`).
#[derive(Clone, Debug)]
pub struct DirectKernel {
    /// Kernel image bytes. What "a kernel image" means is each machine's
    /// boot protocol to decide (`bzImage` on the PC, a raw vector-table
    /// image on Cortex-M).
    pub image: Vec<u8>,
    /// Optional initramfs bytes.
    pub initrd: Option<Vec<u8>>,
    /// Kernel command line.
    pub cmdline: String,
}

/// One machine type the front-end knows how to build, the way QEMU's `-M`
/// names a board. The machine's implementation is a module in the `machine`
/// crate; an arm here only translates options and calls into it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineKind {
    /// `board:ibm/pc`: BIOS software-interrupt firmware interim, PIC/PIT/RTC
    /// port space, VGA text console. Boots El Torito CDs, boot sectors and
    /// `bzImage` kernels through the documented i386 boot protocol.
    Pc,
    /// `soc:st/stm32f103`: the RM0008 memory map on a Cortex-M3 core.
    /// `-kernel` loads a raw image and boots it through its own vector
    /// table; the machine has no drives and one fixed CPU.
    Stm32f103,
    /// `soc:arm/virt64`: the AArch64 `virt` map on a Cortex-A53 core.
    /// `-kernel` loads an arm64 `Image` and boots it through the AArch64
    /// boot protocol; GICv3, PL011 and virtio-mmio are wired.
    Virt64,
}

impl MachineKind {
    /// The `-M` spelling of this machine.
    pub fn name(self) -> &'static str {
        match self {
            MachineKind::Pc => mosaic_machine::i386::pc::MACHINE_NAME,
            MachineKind::Stm32f103 => mosaic_machine::arm::stm32f103::MACHINE_NAME,
            MachineKind::Virt64 => mosaic_machine::arm::virt64::MACHINE_NAME,
        }
    }

    /// One-line description, as `mosaic -M help` prints it.
    pub fn summary(self) -> &'static str {
        match self {
            MachineKind::Pc => mosaic_machine::i386::pc::MACHINE_SUMMARY,
            MachineKind::Stm32f103 => mosaic_machine::arm::stm32f103::MACHINE_SUMMARY,
            MachineKind::Virt64 => mosaic_machine::arm::virt64::MACHINE_SUMMARY,
        }
    }

    /// CPU architectures the machine's engines cover, as `-M help` prints it.
    pub fn architectures(self) -> &'static [&'static str] {
        match self {
            MachineKind::Pc => mosaic_machine::i386::pc::ARCHITECTURES,
            MachineKind::Stm32f103 => mosaic_machine::arm::stm32f103::ARCHITECTURES,
            MachineKind::Virt64 => mosaic_machine::arm::virt64::ARCHITECTURES,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "pc" => Some(MachineKind::Pc),
            "stm32f103" => Some(MachineKind::Stm32f103),
            "virt64" => Some(MachineKind::Virt64),
            _ => None,
        }
    }
}

/// All machines, in `-M help` order. A test pins this against the machine
/// crate's registry so the listing cannot drift from the real catalogue.
pub fn machines() -> Vec<MachineKind> {
    vec![MachineKind::Pc, MachineKind::Stm32f103, MachineKind::Virt64]
}

/// The machine registry's own listing (`-M help` data, QEMU's `MODULE_INFO`
/// aggregation), for callers that want names without the dispatch enum.
pub fn machine_registry() -> Vec<MachineInfo> {
    mosaic_machine::machines()
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
    /// QEMU `-kernel`: boot a kernel image (with optional initramfs and
    /// command line) directly, bypassing every boot loader in the image.
    DirectKernel(DirectKernel),
}

/// Run controls every machine understands (QEMU's `-icount` role). Machines
/// translate these into their own engine budgets; PC-specific extras (trace,
/// watch, dumps) ride along until the report format grows them per machine.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Instructions per scheduling quantum.
    pub quantum: u64,
    /// Total instruction budget.
    pub instruction_limit: u64,
    /// Record the last n instructions (diagnostics).
    pub trace_instructions: usize,
    /// Record stores into these guest regions.
    pub watch_writes: Vec<(u64, u64)>,
    /// Copy these guest regions into the report.
    pub dumps: Vec<(u64, usize)>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            quantum: 50_000,
            instruction_limit: 200_000_000,
            trace_instructions: 0,
            watch_writes: Vec::new(),
            dumps: Vec::new(),
        }
    }
}

/// Everything one run needs, after the CLI resolved file paths to bytes.
#[derive(Clone, Debug)]
pub struct SystemConfig {
    pub machine: MachineKind,
    /// Total guest RAM in KiB (`-m`). Machines with a fixed memory map
    /// (MCU-class boards like `stm32f103`) ignore it.
    pub ram_kib: u64,
    /// The microarchitecture the CPU impersonates (`-cpu`). At the `lib`
    /// boundary this stays machine-neutral; each machine maps it to its own
    /// engine's model list.
    pub cpu_model: CpuModel,
    pub drives: Vec<DriveSpec>,
    pub boot: BootSource,
    pub options: RunOptions,
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

/// The full machine-specific payload of one run.
#[derive(Clone, Debug)]
pub enum MachineReport {
    /// The `pc` machine's report. Gated on the Unicorn backend, which is the
    /// only engine the machine runs on today.
    #[cfg(feature = "unicorn")]
    Pc(PcBootReport),
    Stm32f103(Stm32F103Report),
    Virt64(Virt64Report),
}

impl MachineReport {
    /// Serial bytes the guest emitted, whatever machine produced them.
    pub fn serial(&self) -> &[u8] {
        match self {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => &report.serial,
            MachineReport::Stm32f103(report) => &report.output,
            MachineReport::Virt64(report) => &report.output,
        }
    }

    /// Text console lines at the end of the run.
    pub fn screen(&self) -> Vec<String> {
        match self {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => report.screen.clone(),
            MachineReport::Stm32f103(_) => Vec::new(),
            MachineReport::Virt64(_) => Vec::new(),
        }
    }

    /// Every output channel joined, for pattern assertions that do not care
    /// which console a guest chose.
    pub fn text(&self) -> String {
        match self {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => report.text(),
            MachineReport::Stm32f103(report) => {
                let mut text = String::from_utf8_lossy(&report.output).into_owned();
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text
            }
            MachineReport::Virt64(report) => {
                let mut text = String::from_utf8_lossy(&report.output).into_owned();
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text
            }
        }
    }

    /// The run's own stop classification.
    pub fn stop_reason(&self) -> String {
        match self {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => report.stop_reason.clone(),
            MachineReport::Stm32f103(report) => {
                if report.instructions == 0 {
                    "no instruction executed".into()
                } else {
                    "instruction budget exhausted".into()
                }
            }
            MachineReport::Virt64(report) => report.stop_reason.clone(),
        }
    }

    /// Instructions the guest executed.
    pub fn instructions(&self) -> u64 {
        match self {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => report.instructions,
            MachineReport::Stm32f103(report) => report.instructions,
            MachineReport::Virt64(report) => report.instructions,
        }
    }

    /// Machine-specific failure fields, formatted for humans and failure
    /// messages, so callers never reach into one machine's report shape.
    pub fn diagnosis(&self) -> String {
        match self {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => format!(
                "eip {:#x} (unhandled {:?}, unclaimed {:?}, unmapped {:?})",
                report.eip, report.unhandled_interrupt, report.unclaimed_interrupt, report.unmapped
            ),
            MachineReport::Stm32f103(report) => format!(
                "{} instructions, {} events, pin edges {:?}",
                report.instructions, report.events, report.pin_edges
            ),
            MachineReport::Virt64(report) => format!(
                "{} instructions, {} events, stop reason {}",
                report.instructions, report.events, report.stop_reason
            ),
        }
    }

    /// Whether this run is a failure (engine fault, bus fault, or an
    /// interrupt no machine claimed), as opposed to a normal stop.
    pub fn failed(&self) -> bool {
        match self {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => {
                report.engine_error.is_some()
                    || report.bus_fault.is_some()
                    || report.unclaimed_interrupt.is_some()
            }
            MachineReport::Stm32f103(_) => false,
            MachineReport::Virt64(report) => report.stop_reason == "fault",
        }
    }
}

/// The result of one run, machine-neutral on the surface: the fields every
/// machine fills in, plus the full machine-specific payload.
#[derive(Clone, Debug)]
pub struct SystemReport {
    pub machine: &'static str,
    pub detail: MachineReport,
}

impl SystemReport {
    /// Serial bytes the guest emitted, whatever machine produced them.
    pub fn serial(&self) -> &[u8] {
        self.detail.serial()
    }

    /// Text console lines at the end of the run.
    pub fn screen(&self) -> &[String] {
        match &self.detail {
            #[cfg(feature = "unicorn")]
            MachineReport::Pc(report) => &report.screen,
            MachineReport::Stm32f103(_) => &[],
            MachineReport::Virt64(_) => &[],
        }
    }

    /// Every output channel joined, for pattern assertions that do not care
    /// which console a guest chose.
    pub fn text(&self) -> String {
        self.detail.text()
    }

    /// The run's own stop classification.
    pub fn stop_reason(&self) -> String {
        self.detail.stop_reason()
    }

    /// Instructions the guest executed.
    pub fn instructions(&self) -> u64 {
        self.detail.instructions()
    }

    /// Machine-specific failure fields, formatted.
    pub fn diagnosis(&self) -> String {
        self.detail.diagnosis()
    }

    /// Whether this run is a failure rather than a normal stop.
    pub fn failed(&self) -> bool {
        self.detail.failed()
    }

    /// The PC report, when the machine that ran is `pc` (Unicorn backend).
    #[cfg(feature = "unicorn")]
    pub fn pc(&self) -> Option<&PcBootReport> {
        match &self.detail {
            MachineReport::Pc(report) => Some(report),
            _ => None,
        }
    }
}

/// Run one machine with the attached images. This is the whole front-end: no
/// hidden fixtures, no fallbacks — a missing image is an error, exactly like a
/// system emulator refusing to open a file.
pub fn run(config: &SystemConfig) -> Result<SystemReport, String> {
    match config.machine {
        MachineKind::Pc => run_pc(config),
        MachineKind::Stm32f103 => run_stm32f103(config),
        MachineKind::Virt64 => run_virt64(config),
    }
}

fn run_pc(config: &SystemConfig) -> Result<SystemReport, String> {
    #[cfg(feature = "unicorn")]
    {
        let machine = to_pc_config(config)?;
        let report = run_pc_machine(&machine, &to_pc_options(config))?;
        Ok(SystemReport {
            machine: config.machine.name(),
            detail: MachineReport::Pc(report),
        })
    }
    #[cfg(not(feature = "unicorn"))]
    {
        let _ = config;
        Err("rebuild with --features unicorn: the PC machine needs the Unicorn engine".into())
    }
}

fn run_stm32f103(config: &SystemConfig) -> Result<SystemReport, String> {
    #[cfg(feature = "unicorn")]
    {
        if !config.drives.is_empty() {
            return Err("the stm32f103 machine has no drives; boot it with -kernel".into());
        }
        let kernel = match &config.boot {
            BootSource::DirectKernel(kernel) => kernel,
            BootSource::Order(_) => {
                return Err("the stm32f103 machine boots with -kernel only".into())
            }
        };
        if kernel.initrd.is_some() {
            return Err("the stm32f103 machine has no initramfs concept".into());
        }
        let options = mosaic_machine::arm::stm32f103::Stm32f103Options {
            instruction_limit: config.options.instruction_limit,
            quantum: config.options.quantum,
        };
        let report = mosaic_machine::arm::stm32f103::run(&kernel.image, &options)?;
        Ok(SystemReport {
            machine: config.machine.name(),
            detail: MachineReport::Stm32f103(report),
        })
    }
    #[cfg(not(feature = "unicorn"))]
    {
        let _ = config;
        Err(
            "rebuild with --features unicorn: the stm32f103 machine needs the Unicorn engine"
                .into(),
        )
    }
}

fn run_virt64(config: &SystemConfig) -> Result<SystemReport, String> {
    #[cfg(feature = "unicorn")]
    {
        if !config.drives.is_empty() {
            return Err("the virt64 machine has no drives yet; boot it with -kernel".into());
        }
        let kernel = match &config.boot {
            BootSource::DirectKernel(kernel) => kernel,
            BootSource::Order(_) => {
                return Err("the virt64 machine boots with -kernel only".into())
            }
        };
        if kernel.initrd.is_some() {
            return Err("the virt64 machine has no initramfs concept yet".into());
        }
        let options = mosaic_machine::arm::virt64::Virt64Options {
            instruction_limit: config.options.instruction_limit,
            quantum: config.options.quantum,
            ram_bytes: config.ram_kib * 1024,
        };
        let report = mosaic_machine::arm::virt64::run(&kernel.image, &options)?;
        Ok(SystemReport {
            machine: config.machine.name(),
            detail: MachineReport::Virt64(report),
        })
    }
    #[cfg(not(feature = "unicorn"))]
    {
        let _ = config;
        Err("rebuild with --features unicorn: the virt64 machine needs the Unicorn engine".into())
    }
}

#[cfg(feature = "unicorn")]
fn to_pc_options(config: &SystemConfig) -> PcBootOptions {
    PcBootOptions {
        quantum: config.options.quantum,
        instruction_limit: config.options.instruction_limit,
        trace_instructions: config.options.trace_instructions,
        watch_writes: config.options.watch_writes.clone(),
        dumps: config.options.dumps.clone(),
    }
}

#[cfg(feature = "unicorn")]
fn to_pc_config(config: &SystemConfig) -> Result<PcMachineConfig, String> {
    let cpu_model = match config.cpu_model {
        CpuModel::Default => mosaic_accel_unicorn::X86CpuModel::Qemu64,
        CpuModel::Haswell => mosaic_accel_unicorn::X86CpuModel::Haswell,
        CpuModel::Skylake => mosaic_accel_unicorn::X86CpuModel::SkylakeClient,
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
            machine.with_kernel(mosaic_machine::i386::pc::DirectKernel {
                image: kernel.image.clone(),
                initrd: kernel.initrd.clone(),
                cmdline: kernel.cmdline.clone(),
            });
        }
    }
    Ok(machine)
}

/// The registry and the dispatch table cannot drift apart: every registered
/// machine name resolves through [`MachineKind`], and every kind is listed
/// the way `-M help` prints it.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dispatch_covers_the_whole_registry() {
        let registry = machine_registry();
        assert!(!registry.is_empty(), "the machine registry is empty");
        let listed = machines();
        assert_eq!(registry.len(), listed.len());
        for info in &registry {
            let kind = MachineKind::try_from(info.name)
                .unwrap_or_else(|error| panic!("registry machine {error}"));
            assert_eq!(kind.name(), info.name);
            assert_eq!(kind.summary(), info.summary);
            assert_eq!(kind.architectures(), info.architectures);
        }
    }
}
