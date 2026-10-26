#![forbid(unsafe_code)]
//! `cargo xtask images …` — provisioning for the boot-test catalog.
//!
//! The catalog (`tests/images.toml`) is the single source of truth for what
//! images exist and which bytes they must be. This tool fetches `download`
//! entries into `test-images/` and verifies every digest it can, the way
//! QEMU's test Makefile downloads its assets: reproducibly, from the URL the
//! catalog pins, refused when the bytes do not match.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::CheckResult;

pub struct Entry {
    pub id: String,
    pub os: String,
    pub distro: String,
    pub machine: String,
    pub arch: String,
    pub source: String,
    pub file: String,
    pub url: Option<String>,
    pub sha256: Option<String>,
    pub initrd_file: Option<String>,
    pub initrd_url: Option<String>,
    pub initrd_sha256: Option<String>,
    /// `Some(member)` when `url` names a zip whose member `file` is.
    pub archive_member: Option<String>,
    /// Digest of the archive when `archive_member` is used.
    pub archive_sha256: Option<String>,
    pub size: Option<u64>,
}

pub fn catalog_path(root: &Path) -> PathBuf {
    root.join("tests/images.toml")
}

pub fn image_dir(root: &Path) -> PathBuf {
    root.join("test-images")
}

pub fn load(root: &Path) -> CheckResult<Vec<Entry>> {
    let text = std::fs::read_to_string(catalog_path(root))
        .map_err(|error| format!("{}: {error}", catalog_path(root).display()))?;
    let value: toml::Value = text
        .parse()
        .map_err(|error| format!("{}: {error}", catalog_path(root).display()))?;
    let images = value
        .get("image")
        .and_then(|image| image.as_array())
        .ok_or("tests/images.toml: no [[image]] entries")?;
    let mut entries = Vec::new();
    for image in images {
        let table = image
            .as_table()
            .ok_or("tests/images.toml: an [[image]] entry is not a table")?;
        let text_field = |key: &str| -> Option<String> {
            table
                .get(key)
                .and_then(|value| value.as_str())
                .map(String::from)
        };
        entries.push(Entry {
            id: text_field("id").ok_or("images.toml: missing id")?,
            os: text_field("os").ok_or("images.toml: missing os")?,
            distro: text_field("distro").ok_or("images.toml: missing distro")?,
            machine: text_field("machine").ok_or("images.toml: missing machine")?,
            arch: text_field("arch").ok_or("images.toml: missing arch")?,
            source: text_field("source").ok_or("images.toml: missing source")?,
            file: text_field("file").ok_or("images.toml: missing file")?,
            url: text_field("url"),
            sha256: text_field("sha256"),
            initrd_file: text_field("initrd-file"),
            initrd_url: text_field("initrd-url"),
            initrd_sha256: text_field("initrd-sha256"),
            archive_member: text_field("archive-member"),
            archive_sha256: text_field("archive-sha256"),
            size: table
                .get("size")
                .and_then(|value| value.as_integer())
                .map(|v| v as u64),
        });
    }
    Ok(entries)
}

/// Digest of a local file, via `sha256sum`. The tool is present everywhere CI
/// runs; `doctor` says so when it is not.
fn sha256_of(path: &Path) -> CheckResult<String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|error| format!("sha256sum: {error} (is it installed?)"))?;
    if !output.status.success() {
        return Err(format!("sha256sum failed on {}", path.display()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| format!("sha256sum printed nothing for {}", path.display()))
}

enum Presence {
    Absent,
    Present { digest_ok: bool, pinned: bool },
}

fn presence(root: &Path, entry: &Entry) -> Presence {
    let path = image_dir(root).join(&entry.file);
    if path.exists() {
        let pinned = entry.sha256.as_deref().unwrap_or_default();
        if pinned.is_empty() {
            return Presence::Present {
                digest_ok: true,
                pinned: false,
            };
        }
        let digest = sha256_of(&path).unwrap_or_default();
        Presence::Present {
            digest_ok: digest == pinned,
            pinned: true,
        }
    } else {
        Presence::Absent
    }
}

pub fn list(root: &Path) -> CheckResult<()> {
    let entries = load(root)?;
    let widest = entries
        .iter()
        .map(|entry| entry.id.len())
        .max()
        .unwrap_or(4);
    println!(
        "{:<width$} {:<9} {:<7} {:<8} file",
        "id",
        "os",
        "source",
        "status",
        width = widest
    );
    for entry in &entries {
        let status = match presence(root, entry) {
            Presence::Absent => "missing".to_string(),
            Presence::Present {
                digest_ok: true,
                pinned: true,
            } => "verified".to_string(),
            Presence::Present {
                digest_ok: true,
                pinned: false,
            } => "present".to_string(),
            Presence::Present {
                digest_ok: false, ..
            } => "DIGEST MISMATCH".to_string(),
        };
        println!(
            "{:<width$} {:<9} {:<7} {:<8} {}",
            entry.id,
            entry.os,
            entry.source,
            status,
            entry.file,
            width = widest
        );
    }
    Ok(())
}

fn download(url: &str, destination: &Path, expected_size: Option<u64>) -> CheckResult<()> {
    let status = Command::new("curl")
        .args(["-fL", "--retry", "3", "--max-time", "1800", "-o"])
        .arg(destination)
        .arg(url)
        .status()
        .map_err(|error| format!("curl: {error} (is it installed?)"))?;
    if !status.success() {
        let _ = std::fs::remove_file(destination);
        return Err(format!("download failed: {url}"));
    }
    if let Some(size) = expected_size {
        let size_got = std::fs::metadata(destination).map(|m| m.len()).unwrap_or(0);
        if size_got != size {
            return Err(format!(
                "{url}: expected {size} bytes, got {size_got} (catalog size is stale?)"
            ));
        }
    }
    Ok(())
}

fn verify_digest(path: &Path, pinned: &str, what: &str) -> CheckResult<()> {
    let digest = sha256_of(path)?;
    if digest != pinned {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "{what}: digest {digest} does not match the catalog's {pinned}"
        ));
    }
    Ok(())
}

/// Extract one member of a zip archive, verifying the archive first.
fn extract_member(archive: &Path, member: &str, destination: &Path, what: &str) -> CheckResult<()> {
    let output = Command::new("unzip")
        .args(["-p"])
        .arg(archive)
        .arg(member)
        .output()
        .map_err(|error| format!("unzip: {error} (is it installed?)"))?;
    if !output.status.success() {
        return Err(format!(
            "{what}: extracting '{member}' failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    std::fs::write(destination, &output.stdout).map_err(|error| format!("{what}: {error}"))?;
    Ok(())
}

fn fetch_one(root: &Path, entry: &Entry) -> CheckResult<()> {
    let dir = image_dir(root);
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let destination = dir.join(&entry.file);
    let url = entry
        .url
        .as_deref()
        .ok_or_else(|| format!("{}: entry has no url to fetch", entry.id))?;
    if let Some(member) = &entry.archive_member {
        let archive_path = dir.join(format!("{}.archive", entry.file));
        download(url, &archive_path, entry.size)?;
        let pinned = entry
            .archive_sha256
            .as_deref()
            .ok_or_else(|| format!("{}: archive-member needs archive-sha256", entry.id))?;
        verify_digest(&archive_path, pinned, &entry.id)?;
        extract_member(&archive_path, member, &destination, &entry.id)?;
        let _ = std::fs::remove_file(&archive_path);
    } else if destination.exists() {
        println!("{}: already present", entry.id);
    } else {
        download(url, &destination, entry.size)?;
    }
    if let Some(pinned) = &entry.sha256 {
        verify_digest(&destination, pinned, &entry.id)?;
    }
    // The initramfs of a kernel-files entry, when it has its own URL.
    if let (Some(initrd_file), Some(initrd_url)) = (&entry.initrd_file, &entry.initrd_url) {
        let initrd_path = dir.join(initrd_file);
        if !initrd_path.exists() {
            download(initrd_url, &initrd_path, None)?;
        }
        if let Some(pinned) = &entry.initrd_sha256 {
            verify_digest(&initrd_path, pinned, &entry.id)?;
        }
    }
    println!("{}: fetched and verified", entry.id);
    Ok(())
}

pub fn fetch(root: &Path, only: &[String]) -> CheckResult<()> {
    let entries = load(root)?;
    let mut failures = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &entries {
        if entry.source != "download" {
            continue;
        }
        if !only.is_empty() && !only.contains(&entry.id) {
            continue;
        }
        match fetch_one(root, entry) {
            Ok(()) => *counts.entry("fetched".into()).or_default() += 1,
            Err(error) => {
                *counts.entry("failed".into()).or_default() += 1;
                failures.push(error);
            }
        }
    }
    for (what, count) in &counts {
        println!("images: {what} {count}");
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

pub fn verify(root: &Path) -> CheckResult<()> {
    let entries = load(root)?;
    let mut failures = Vec::new();
    let mut verified = 0usize;
    let mut present = 0usize;
    for entry in &entries {
        match presence(root, entry) {
            Presence::Absent => {}
            Presence::Present {
                digest_ok: true,
                pinned: true,
            } => verified += 1,
            Presence::Present {
                digest_ok: true,
                pinned: false,
            } => present += 1,
            Presence::Present {
                digest_ok: false, ..
            } => failures.push(format!("{}: digest mismatch", entry.id)),
        }
    }
    println!(
        "images: {verified} verified, {present} present (unpinned), {} mismatched",
        failures.len()
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
