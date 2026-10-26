#![forbid(unsafe_code)]
//! Machine-level boot tests over real guest images — the Mosaic counterpart
//! of QEMU's functional tests.
//!
//! Policy (ADR 0022): every entry in `tests/images.toml` names a *real*
//! image — a committed fixture, a downloadable release, or an image the
//! operator supplies — and the same generic harness boots it on a machine
//! and asserts on what the guest actually printed. There are no synthetic
//! images and no per-OS bespoke test code: the catalog is data, the harness
//! is one loop, and an image that is not present is *skipped loudly* rather
//! than faked.
//!
//! Provisioning (mirrors QEMU's asset handling):
//!
//! * `committed` — lives in `tests/assets/`, in the repository.
//! * `download`  — `cargo xtask images fetch` places it in `test-images/`
//!                 after verifying its SHA-256 against this catalog.
//! * `manual`    — redistribution is restricted, so the operator provides it
//!                 in `test-images/` themselves (Windows media, for example).

pub mod sha256;

use std::path::{Path, PathBuf};

/// Where the catalog itself lives.
pub const CATALOG: &str = "images.toml";
/// Committed fixtures (checked into the repository).
pub const ASSETS_DIR: &str = "tests/assets";
/// Fetched or operator-supplied images (never committed).
pub const IMAGE_DIR: &str = "test-images";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSource {
    /// Committed under `tests/assets/`.
    Committed,
    /// Fetched into `test-images/` by `cargo xtask images fetch`.
    Download,
    /// Supplied by the operator into `test-images/` (license-restricted).
    Manual,
}

#[derive(Clone, Debug)]
pub enum BootPlan {
    /// Boot the image as a CD-ROM (`-cdrom`).
    Cdrom,
    /// Boot the image as a floppy (`-fda`, boot order `a`).
    Floppy,
    /// Boot the image as a hard disk (`-hda`, boot order `c`).
    HardDisk,
    /// Extract a kernel (and optional initramfs) from the ISO and boot it
    /// directly (`-kernel`/`-initrd`/`-append`).
    KernelFromIso {
        kernel: String,
        initrd: Option<String>,
        append: String,
    },
    /// The entry's own `file` is a kernel image (bzImage), optionally with
    /// an initramfs file beside it in the same image directory.
    KernelFiles {
        initrd_file: Option<String>,
        append: String,
    },
    /// No boot plan expressible against the machines that exist today; the
    /// entry stays in the catalog as a ledger row until one is.
    Deferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryStatus {
    /// The expectation patterns are a complete boot contract: a miss fails.
    Asserted,
    /// Bring-up ledger entry: the patterns are the milestone this machine
    /// has already reached on this image — still a real boot, asserted.
    BringUp,
    /// The machine this entry needs does not exist yet; the harness skips it
    /// with the recorded reason instead of pretending.
    MachinePending,
}

#[derive(Clone, Debug)]
pub struct ImageEntry {
    pub id: String,
    /// `linux` / `windows` / `android` / `rtos` / `other`.
    pub os: String,
    pub distro: String,
    pub machine: String,
    pub arch: String,
    pub source: ImageSource,
    pub file: String,
    pub url: Option<String>,
    /// Bytes expected, when the server publishes it (a cheap pre-check for
    /// a 700 MiB download).
    pub size: Option<u64>,
    /// SHA-256 of `file`, when it is known. A `download` entry without one
    /// is a catalog error: an unverifiable download is not a test asset.
    pub sha256: Option<String>,
    /// For `kernel-files` plans: initramfs file name, download URL and
    /// digest, beside the kernel in the same image directory.
    pub initrd_file: Option<String>,
    pub initrd_url: Option<String>,
    pub initrd_sha256: Option<String>,
    pub boot: BootPlan,
    pub budget: u64,
    /// Machine RAM in MiB (`-m`); defaults to 64 when the entry says nothing.
    pub ram_mib: Option<u64>,
    pub expect: Vec<String>,
    pub status: EntryStatus,
    pub note: String,
}

/// Absolute path of the workspace root, located from this crate's position.
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(1)
        .map(Path::to_path_buf)
        .expect("tests/ sits one level below the workspace root")
}

fn text(table: &toml::Table, key: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("images.toml: field '{key}' is missing or not a string"))
}

fn optional_text(table: &toml::Table, key: &str) -> Result<Option<String>, String> {
    match table.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_string()))
            .ok_or_else(|| format!("images.toml: field '{key}' is not a string")),
    }
}

/// A digest field is either absent (unknown yet) or exactly 64 hex digits.
fn optional_digest(table: &toml::Table, key: &str) -> Result<Option<String>, String> {
    match optional_text(table, key)? {
        None => Ok(None),
        Some(value) if value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(Some(value.to_ascii_lowercase()))
        }
        Some(value) => Err(format!(
            "images.toml: field '{key}' must be 64 hex digits, got '{value}'"
        )),
    }
}

/// Parse `tests/images.toml` into the catalog.
pub fn load_catalog() -> Result<Vec<ImageEntry>, String> {
    let path = workspace_root()
        .join(ASSETS_DIR)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(workspace_root)
        .join(CATALOG);
    let raw =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let parsed: toml::Value = raw
        .parse()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let images = parsed
        .get("image")
        .and_then(|images| images.as_array())
        .ok_or("images.toml: no [[image]] entries")?;
    let mut catalog = Vec::new();
    for image in images {
        let table = image
            .as_table()
            .ok_or("images.toml: an [[image]] entry is not a table")?;
        let source = match text(table, "source")?.as_str() {
            "committed" => ImageSource::Committed,
            "download" => ImageSource::Download,
            "manual" => ImageSource::Manual,
            other => return Err(format!("images.toml: unknown source '{other}'")),
        };
        let boot = match text(table, "boot")?.as_str() {
            "cdrom" => BootPlan::Cdrom,
            "floppy" => BootPlan::Floppy,
            "harddisk" => BootPlan::HardDisk,
            "kernel-from-iso" => BootPlan::KernelFromIso {
                kernel: text(table, "kernel")?,
                initrd: optional_text(table, "initrd")?,
                append: optional_text(table, "append")?.unwrap_or_default(),
            },
            "kernel-files" => BootPlan::KernelFiles {
                initrd_file: optional_text(table, "initrd-file")?,
                append: optional_text(table, "append")?.unwrap_or_default(),
            },
            "deferred" => BootPlan::Deferred,
            other => return Err(format!("images.toml: unknown boot plan '{other}'")),
        };
        let status = match text(table, "status")?.as_str() {
            "asserted" => EntryStatus::Asserted,
            "bring-up" => EntryStatus::BringUp,
            "machine-pending" => EntryStatus::MachinePending,
            other => return Err(format!("images.toml: unknown status '{other}'")),
        };
        let sha256 = optional_digest(table, "sha256")?;
        let url = optional_text(table, "url")?;
        if source == ImageSource::Download {
            if url.is_none() {
                return Err(format!(
                    "images.toml: {}: a download entry needs a url",
                    text(table, "id")?
                ));
            }
            if sha256.is_none() {
                return Err(format!(
                    "images.toml: {}: a download entry needs a pinned sha256",
                    text(table, "id")?
                ));
            }
        }
        if status != EntryStatus::MachinePending && matches!(boot, BootPlan::Deferred) {
            return Err(format!(
                "images.toml: {}: a deferred boot plan can only be machine-pending",
                text(table, "id")?
            ));
        }
        let expect = table
            .get("expect")
            .and_then(|expect| expect.as_array())
            .map(|patterns| {
                patterns
                    .iter()
                    .filter_map(|pattern| pattern.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        catalog.push(ImageEntry {
            id: text(table, "id")?,
            os: text(table, "os")?,
            distro: text(table, "distro")?,
            machine: text(table, "machine")?,
            arch: text(table, "arch")?,
            source,
            file: text(table, "file")?,
            url,
            size: table
                .get("size")
                .and_then(|size| size.as_integer())
                .map(|size| size as u64),
            sha256,
            initrd_file: optional_text(table, "initrd-file")?,
            initrd_url: optional_text(table, "initrd-url")?,
            initrd_sha256: optional_digest(table, "initrd-sha256")?,
            boot,
            budget: table
                .get("budget")
                .and_then(|budget| budget.as_integer())
                .ok_or("images.toml: field 'budget' is missing or not an integer")?
                as u64,
            ram_mib: table
                .get("ram-mib")
                .and_then(|ram| ram.as_integer())
                .map(|ram| ram as u64),
            expect,
            status,
            note: optional_text(table, "note")?.unwrap_or_default(),
        });
    }
    Ok(catalog)
}

/// Catalog entries for one OS family, in catalog order. A catalog that does
/// not parse fails the test run here, loudly: a silent empty list is how a
/// boot suite stops testing anything.
pub fn entries_for(os: &str) -> Vec<ImageEntry> {
    let catalog = load_catalog().expect("tests/images.toml must parse");
    catalog.into_iter().filter(|entry| entry.os == os).collect()
}

/// The resolved bytes of one entry, or why they are not available.
pub enum Resolved {
    /// The image is present, and its digest matches when one is pinned.
    Verified(Vec<u8>),
    /// The image is absent — a normal state for `download`/`manual` entries
    /// on a machine that never fetched them.
    Missing { entry: ImageEntry, path: PathBuf },
    /// The image is present but is not the image the catalog names. This is
    /// a hard error, not a skip: silently booting the wrong bytes is how a
    /// test suite starts lying.
    DigestMismatch {
        entry: ImageEntry,
        path: PathBuf,
        expected: String,
        actual: String,
    },
}

/// Path one image lives at, according to its provisioning kind.
pub fn image_path(entry: &ImageEntry) -> PathBuf {
    match entry.source {
        ImageSource::Committed => workspace_root().join(ASSETS_DIR).join(&entry.file),
        ImageSource::Download | ImageSource::Manual => {
            workspace_root().join(IMAGE_DIR).join(&entry.file)
        }
    }
}

/// Read one image and verify its digest against the catalog.
pub fn resolve(entry: &ImageEntry) -> Resolved {
    let path = image_path(entry);
    let Ok(bytes) = std::fs::read(&path) else {
        return Resolved::Missing {
            entry: entry.clone(),
            path,
        };
    };
    match &entry.sha256 {
        Some(expected) => {
            let actual = sha256::hex(&bytes);
            if actual == *expected {
                Resolved::Verified(bytes)
            } else {
                Resolved::DigestMismatch {
                    entry: entry.clone(),
                    path,
                    expected: expected.clone(),
                    actual,
                }
            }
        }
        None => Resolved::Verified(bytes),
    }
}

/// The library entry point each OS-family test calls: boot one real image
/// and assert the milestone the catalog recorded for it.
#[cfg(feature = "unicorn")]
pub fn boot_case(entry: &ImageEntry) {
    if let EntryStatus::MachinePending = entry.status {
        eprintln!(
            "skip {}: machine '{}' is not implemented yet: {}",
            entry.id, entry.machine, entry.note
        );
        return;
    }
    let data = match resolve(entry) {
        Resolved::Verified(bytes) => bytes,
        Resolved::Missing { path, .. } => {
            eprintln!(
                "skip {}: image {} is not present (fetch with `cargo xtask images fetch` or place it there)",
                entry.id,
                path.display()
            );
            return;
        }
        Resolved::DigestMismatch {
            path,
            expected,
            actual,
            ..
        } => panic!(
            "{}: {} has digest {actual}, catalog says {expected}",
            entry.id,
            path.display()
        ),
    };
    let report = run_entry(entry, &data)
        .unwrap_or_else(|error| panic!("{}: machine error: {error}", entry.id));
    check_entry(entry, &report);
}

/// Build the machine configuration one entry describes and run it.
#[cfg(feature = "unicorn")]
pub fn run_entry(
    entry: &ImageEntry,
    data: &[u8],
) -> Result<mosaic_apps_system::SystemReport, String> {
    use mosaic_apps_system::{BootSource, DriveSpec, MediaKind, SystemConfig};
    let mut config = SystemConfig {
        machine: mosaic_apps_system::MachineKind::try_from(entry.machine.as_str())
            .map_err(|error| error.to_string())?,
        ram_kib: entry.ram_mib.unwrap_or(64) * 1024,
        // Distribution kernels are built for x86-64-v2 and fault on BMI2
        // under the backend's qemu64-class default core.
        cpu_model: mosaic_apps_system::CpuModel::Haswell,
        drives: Vec::new(),
        boot: BootSource::Order("dca".into()),
        options: mosaic_apps_system::PcBootOptions {
            quantum: 50_000,
            instruction_limit: entry.budget,
            ..Default::default()
        },
    };
    match &entry.boot {
        BootPlan::Cdrom => {
            config.drives.push(DriveSpec {
                media: MediaKind::Cdrom,
                data: data.to_vec(),
            });
        }
        BootPlan::Floppy => {
            config.drives.push(DriveSpec {
                media: MediaKind::Floppy,
                data: data.to_vec(),
            });
            config.boot = BootSource::Order("a".into());
        }
        BootPlan::HardDisk => {
            config.drives.push(DriveSpec {
                media: MediaKind::HardDisk,
                data: data.to_vec(),
            });
            config.boot = BootSource::Order("c".into());
        }
        BootPlan::KernelFromIso {
            kernel,
            initrd,
            append,
        } => {
            let kernel_bytes = mosaic_apps_system::linux_boot::iso_file(data, kernel)
                .and_then(|file| file.read(data))
                .ok_or_else(|| format!("{kernel} is not in the ISO image"))?;
            let initrd_bytes = initrd
                .as_ref()
                .map(|path| {
                    mosaic_apps_system::linux_boot::iso_file(data, path)
                        .and_then(|file| file.read(data))
                        .ok_or_else(|| format!("{path} is not in the ISO image"))
                })
                .transpose()?;
            // The CD stays attached: the initramfs often needs modules from it.
            config.drives.push(DriveSpec {
                media: MediaKind::Cdrom,
                data: data.to_vec(),
            });
            config.boot = BootSource::DirectKernel(mosaic_apps_system::DirectKernel {
                image: kernel_bytes,
                initrd: initrd_bytes,
                cmdline: append.clone(),
            });
        }
        BootPlan::KernelFiles {
            initrd_file,
            append,
        } => {
            // `data` is the kernel itself; the initramfs is a file beside it.
            let initrd_bytes = initrd_file
                .as_ref()
                .map(|name| {
                    let path = workspace_root().join(IMAGE_DIR).join(name);
                    std::fs::read(&path)
                        .map_err(|error| format!("initramfs {}: {error}", path.display()))
                })
                .transpose()?;
            config.boot = BootSource::DirectKernel(mosaic_apps_system::DirectKernel {
                image: data.to_vec(),
                initrd: initrd_bytes,
                cmdline: append.clone(),
            });
        }
        BootPlan::Deferred => {
            return Err(format!(
                "{}: boot plan is deferred until the machine can express it",
                entry.id
            ))
        }
    }
    mosaic_apps_system::run(&config)
}

/// Assert the recorded expectation against what the guest actually emitted.
#[cfg(feature = "unicorn")]
pub fn check_entry(entry: &ImageEntry, report: &mosaic_apps_system::SystemReport) {
    let text = report.text();
    println!(
        "---- {}: stopped {} after {} instructions ----",
        entry.id,
        report.stop_reason(),
        report.instructions()
    );
    for line in report.screen() {
        println!("screen| {line}");
    }
    let serial = String::from_utf8_lossy(report.serial());
    for line in serial.split('\n') {
        println!("serial| {line}");
    }
    let mut missing: Vec<&str> = Vec::new();
    for pattern in &entry.expect {
        if !text.contains(pattern.as_str()) {
            missing.push(pattern);
        }
    }
    match entry.status {
        EntryStatus::Asserted | EntryStatus::BringUp => {
            // An entry without patterns is a bring-up entry whose milestone is
            // simply "the guest's own code ran" — the instruction count is
            // that milestone, and a machine that cannot even run the image
            // reports an error before this point.
            if !missing.is_empty() {
                panic!(
                    "{}: never reached {}: stopped {} at eip {:#x} (unhandled {:?}, unclaimed {:?}, unmapped {:?})",
                    entry.id,
                    missing.join(", "),
                    report.stop_reason(),
                    report.pc.eip,
                    report.pc.unhandled_interrupt,
                    report.pc.unclaimed_interrupt,
                    report.pc.unmapped,
                );
            }
        }
        EntryStatus::MachinePending => unreachable!("checked by the caller"),
    }
}
