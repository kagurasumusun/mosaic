#![forbid(unsafe_code)]
//! Architecture checks use all declared dependencies, not only active features.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod catalog;
pub mod images;
mod policy;
pub use policy::{
    dependency_allowed, external_dependency_allowed, forbidden_name, kebab_case, level_of,
    lint_layers, validate_layer, Layer, Package,
};

pub type CheckResult<T> = Result<T, String>;

pub fn load_packages(root: &Path) -> CheckResult<Vec<Package>> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .ok_or("missing workspace_members")?
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    let mut packages = Vec::new();
    for p in metadata["packages"].as_array().ok_or("missing packages")? {
        if !members.contains(p["id"].as_str().ok_or("missing package id")?) {
            continue;
        }
        let name = p["name"].as_str().ok_or("missing name")?.to_owned();
        let layer = &p["metadata"]["layer"];
        let layer = Layer {
            domain: layer["domain"]
                .as_str()
                .ok_or_else(|| format!("{name}: missing layer.domain"))?
                .into(),
            kind: layer["kind"]
                .as_str()
                .ok_or_else(|| format!("{name}: missing layer.kind"))?
                .into(),
            level: layer["level"]
                .as_u64()
                .ok_or_else(|| format!("{name}: missing layer.level"))?,
        };
        let dir = Path::new(p["manifest_path"].as_str().ok_or("missing manifest_path")?)
            .parent()
            .ok_or("manifest has no parent")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let relative = dir.strip_prefix(root).map_err(|e| e.to_string())?;
        if !validate_layer(&relative.to_string_lossy(), &layer) {
            return Err(format!(
                "{name}: invalid layer metadata for {}",
                relative.display()
            ));
        }
        let mut dependencies = Vec::new();
        for dep in p["dependencies"].as_array().ok_or("missing dependencies")? {
            if let Some(path) = dep["path"].as_str() {
                dependencies.push(Path::new(path).canonicalize().map_err(|e| e.to_string())?);
            } else if !external_dependency_allowed(
                &relative.to_string_lossy(),
                &layer,
                dep["name"].as_str().ok_or("missing dependency name")?,
            ) {
                return Err(format!(
                    "{name}: external dependency {} is not allowlisted",
                    dep["name"]
                ));
            }
        }
        packages.push(Package {
            name,
            dir,
            layer,
            dependencies,
        });
    }
    Ok(packages)
}

fn walk_dirs(path: &Path, directories: &mut Vec<PathBuf>) -> CheckResult<()> {
    for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || matches!(
                name.as_ref(),
                "target" | "node_modules" | "build" | "debug" | "test-images"
            )
        {
            continue;
        }
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            directories.push(entry.path());
            walk_dirs(&entry.path(), directories)?;
        }
    }
    Ok(())
}

/// The `ip` function vocabulary (ADR 0031 §4): CPU-addressed device models,
/// QEMU's `hw/<function>/` axis. An `ip:` manifest lives in exactly one of
/// these under `manifests/hw/ip/`, and its code mirrors the same directory
/// under `crates/hw/src/ip/`.
pub const IP_FUNCTION_DIRS: &[&str] = &[
    "audio", "char", "clock", "display", "dma", "gpio", "i2c", "ide", "input", "intc", "mailbox",
    "net", "nvram", "pci", "platform", "rtc", "sd", "spi", "timer", "usb", "virtio", "wdt",
];

/// The `part` domain vocabulary (ADR 0031 §3): fabric-mediated leaf parts,
/// placed by what they are — never by which bus they speak. A `part:`
/// manifest lives in exactly one of these under `manifests/hw/part/`, and
/// its code mirrors the same directory under `crates/hw/src/part/`.
pub const PART_DOMAIN_DIRS: &[&str] = &[
    "adc",
    "audio",
    "can",
    "dac",
    "display",
    "ec",
    "expander",
    "fan",
    "i3c",
    "input",
    "instrument",
    "led",
    "memory",
    "motion",
    "mux",
    "net",
    "nfc",
    "power",
    "rtc",
    "security",
    "sensor",
    "serial",
    "storage",
];

/// Top-level directory roots whose contents must be kebab-case. Workspace
/// crates live under `crates/` (ADR 0028), so a crate tree rooted at
/// `crates/<root>` and a data tree rooted at `<root>` are both checked.
const KEBAB_ROOTS: &[&str] = &[
    "util",
    "exec",
    "contract",
    "runtime",
    "isa",
    "accel",
    "hw",
    "machine",
    "block",
    "describe",
    "assemble",
    "mosaic",
    "tests",
    "xtask",
    "manifests",
    "firmware",
    "host",
    "ops",
    "bridge",
    "ffi",
    "third_party",
    "docs",
];

fn crate_name(path: &str) -> CheckResult<String> {
    // All workspace crates live under crates/ (ADR 0028); the layer
    // vocabulary is the crate path below that root.
    let path = path.strip_prefix("crates/").unwrap_or(path);
    let expected = if path == "mosaic" {
        "mosaic".to_owned()
    } else if path == "xtask" {
        "mosaic-xtask".to_owned()
    } else if path == "tests" {
        // The machine-level boot-test package: one crate, not a per-test tree.
        "mosaic-tests-boot".to_owned()
    } else {
        format!("mosaic-{}", path.replace('/', "-"))
    };
    Ok(expected)
}

pub fn lint_names(root: &Path, packages: &[Package]) -> CheckResult<Vec<String>> {
    let mut errors = Vec::new();
    let mut directories = Vec::new();
    walk_dirs(root, &mut directories)?;
    for directory in directories {
        let relative = directory.strip_prefix(root).map_err(|e| e.to_string())?;
        let parts: Vec<_> = relative
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let name = parts.last().ok_or("empty directory path")?;
        let text = relative.to_string_lossy();
        // `misc` was the one QEMU catch-all ADR 0024 allowed; ADR 0031
        // eliminates it — every model now has a real home.
        if forbidden_name(name) || name == "misc" {
            errors.push(format!("{text}: forbidden directory name"));
        }
        // Workspace crates root their trees at crates/<root> (ADR 0028);
        // data trees (manifests/, firmware/, docs/) root at the top level.
        let root_segment = if !parts.is_empty() && parts[0] == "crates" {
            parts.get(1).map(String::as_str).unwrap_or("")
        } else {
            parts.first().map(String::as_str).unwrap_or("")
        };
        if KEBAB_ROOTS.contains(&root_segment) && !kebab_case(name) && root_segment != "third_party"
        {
            errors.push(format!("{text}: directory must use kebab-case"));
        }
    }
    for package in packages {
        let path = package
            .dir
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        let expected = crate_name(&path)?;
        if package.name != expected || !kebab_case(&package.name) || forbidden_name(&package.name) {
            errors.push(format!("{}: expected crate name {expected}", package.name));
        }
    }
    Ok(errors)
}

/// kebab-case model name (manifests, IDs) to snake_case module file stem.
fn snake_case(name: &str) -> String {
    name.replace('-', "_")
}

/// Validate the manifests tree against the code tree (ADR 0024 §3.3):
/// every manifest mirrors the module that implements it, and every
/// declarative composition still builds a machine graph.
pub fn lint_catalog(root: &Path) -> CheckResult<Vec<String>> {
    use crate::catalog::{
        parse_board, parse_soc, validate_graph_ports, validate_leaf_catalog_manifest,
    };
    use mosaic_describe::{BoardManifest, MachineGraph, SocManifest};

    fn collect(path: &Path, files: &mut Vec<PathBuf>) -> CheckResult<()> {
        for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules") {
                continue;
            }
            let path = entry.path();
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                collect(&path, files)?;
            } else if name.ends_with(".toml") && name != "origins.toml" {
                files.push(path);
            }
        }
        Ok(())
    }

    let manifests_root = root.join("manifests");
    if !manifests_root.is_dir() {
        return Err("manifests/ tree is missing".into());
    }
    let origins_text = fs::read_to_string(manifests_root.join("origins.toml"))
        .map_err(|error| format!("manifests/origins.toml: {error}"))?;
    let origins_value: toml::Value = toml::from_str(&origins_text)
        .map_err(|error| format!("manifests/origins.toml: {error}"))?;
    let origins_table = origins_value
        .get("origins")
        .and_then(toml::Value::as_table)
        .ok_or("manifests/origins.toml: [origins] must be a table")?;
    let known_origins: BTreeSet<String> = origins_table.keys().cloned().collect();
    let mut files = Vec::new();
    collect(&manifests_root, &mut files)?;
    files.sort();

    let mut errors = Vec::new();
    let mut ids: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut leaf_metadata: BTreeMap<String, toml::Value> = BTreeMap::new();
    struct Located<T> {
        source: PathBuf,
        value: T,
    }
    let mut references: Vec<(PathBuf, String)> = Vec::new();
    let mut soc_manifests: BTreeMap<String, Located<SocManifest>> = BTreeMap::new();
    let mut board_manifests: BTreeMap<String, Located<BoardManifest>> = BTreeMap::new();

    for file in files {
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) => {
                errors.push(format!("{}: {error}", file.display()));
                continue;
            }
        };
        let relative = file
            .strip_prefix(root)
            .map_err(|error| error.to_string())?
            .to_path_buf();
        let value: toml::Value = match toml::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                errors.push(format!("{}: invalid TOML: {error}", relative.display()));
                continue;
            }
        };
        let file_name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");

        // The manifest's kind: composition manifests keep their file names
        // (soc.toml / board.toml); leaf manifests carry it in the ID prefix
        // (cpu:/ip:/ip-lib:/part:), so the file name is free for the model.
        let leaf_id = ["identity", "part"]
            .iter()
            .find_map(|section| {
                value
                    .get(section)
                    .and_then(|entry| entry.get("id"))
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            })
            .filter(|id| id.contains(':'));
        let kind = if file_name == "soc.toml" {
            "soc"
        } else if file_name == "board.toml" {
            "board"
        } else if let Some(id) = &leaf_id {
            match id.split_once(':').map(|(prefix, _)| prefix) {
                Some("cpu") => "cpu",
                Some("ip" | "ip-lib") => "ip",
                Some("part") => "part",
                _ => "",
            }
        } else {
            ""
        };

        if kind.is_empty() {
            errors.push(format!(
                "{}: leaf manifests must exist as manifests/<domain>/... with an id like 'ip:arm/pl011'",
                relative.display()
            ));
            continue;
        }

        if matches!(kind, "cpu" | "ip" | "part") {
            // Two manifest forms exist (ADR 0031 §7): the `[part]` schema
            // carries bindings + register tables for decode-level leaf
            // parts; the `[identity]` schema carries ports + params for
            // spawn-registered models. A `part:` ID may use either form
            // (a USB HID keyboard is a part that owns ports), so the form
            // picks its own validator and never the kind.
            let has_part_section = value.get("part").is_some();
            let kind_file = match (kind, has_part_section) {
                ("cpu", _) => "cpu.toml",
                ("part", true) => "part.toml",
                (_, _) => "ip.toml",
            };
            if let Err(error) = validate_leaf_catalog_manifest(kind_file, &value, &known_origins) {
                errors.push(format!("{}: {error}", relative.display()));
            }
        }

        // The id's section follows the form, not the kind: `[part]`-form
        // manifests read their id from `[part]`, `[identity]`-form ones
        // from `[identity]` (both were probed above, so this only picks).
        let id_section = ["part", "identity"]
            .iter()
            .find(|section| value.get(*section).is_some())
            .copied()
            .unwrap_or("identity");
        let (section, prefixes): (&str, &[&str]) = match kind {
            "cpu" => ("identity", &["cpu:"]),
            "ip" => ("identity", &["ip:", "ip-lib:"]),
            "part" => (id_section, &["part:"]),
            "soc" => ("soc", &["soc:"]),
            _ => ("board", &["board:"]),
        };
        let id = value
            .get(section)
            .and_then(|entry| entry.get("id"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        if matches!(kind, "cpu" | "ip" | "part") {
            if let Some(catalog_id) = id.as_ref() {
                leaf_metadata.insert(catalog_id.clone(), value.clone());
            }
        }

        let Some(id) = id.filter(|id| !id.trim().is_empty()) else {
            errors.push(format!(
                "{}: expected a non-empty id and summary in the manifest's {section:?} section",
                relative.display()
            ));
            continue;
        };
        if !prefixes.iter().any(|prefix| id.starts_with(prefix)) {
            errors.push(format!(
                "{}: ID {id:?} must use one of the expected namespaces: {}",
                relative.display(),
                prefixes.join(", ")
            ));
            continue;
        }
        let (_, id_suffix) = id.split_once(':').unwrap_or(("", id.as_str()));
        let Some((origin, name)) = id_suffix.split_once('/') else {
            errors.push(format!(
                "{}: ID {id:?} must read <kind>:<origin>/<name>",
                relative.display()
            ));
            continue;
        };
        if !known_origins.contains(origin) {
            errors.push(format!(
                "{}: unknown canonical origin {origin:?}",
                relative.display()
            ));
        }
        let summary = value
            .get(section)
            .and_then(|entry| entry.get("summary"))
            .and_then(toml::Value::as_str);
        if summary.is_none_or(|value| value.trim().is_empty()) {
            errors.push(format!(
                "{}: [{section}] must have a non-empty summary",
                relative.display()
            ));
        }
        match ids.entry(id.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(relative.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                errors.push(format!(
                    "{}: catalog ID {id:?} is already declared by {}",
                    relative.display(),
                    entry.get().display()
                ));
            }
        }

        // Mirror rule (ADR 0024 §3.3, revised by ADR 0031 §7): the manifest
        // path mirrors the code path AND the id's kind mirrors the tree:
        // an `ip:` manifest under manifests/hw/ip/<function>/ is implemented
        // by hw/src/ip/<function>/<model>.rs, a `part:` manifest under
        // manifests/hw/part/<domain>/ by hw/src/part/<domain>/<model>.rs
        // (or a directory of the same name for multi-bus models), an ip-lib
        // under manifests/hw/modelkit/ by hw/src/modelkit/<model>.rs, a cpu
        // under manifests/isa/<arch>/<model>.toml by isa/<arch>/src/cpu/
        // <model>.rs, and a machine manifest under
        // manifests/machine/<arch>/<name>/ by machine/src/<arch>/<name>/.
        let mirror_error = |expected: &str| -> String {
            format!(
                "{}: no implementing {} for {id:?} (manifest and code must mirror, ADR 0031 §7)",
                relative.display(),
                expected
            )
        };
        let rel = relative
            .parent()
            .unwrap_or(Path::new(""))
            .to_string_lossy()
            .replace('\\', "/");
        let parts: Vec<&str> = rel.split('/').collect();
        let stem = file_name.strip_suffix(".toml").unwrap_or(file_name);
        let stem_error = |expected: &str| -> String {
            format!(
                "{}: manifest stem {stem:?} must match the ID name {expected:?}",
                relative.display()
            )
        };
        match (kind, parts.as_slice()) {
            ("ip", ["manifests", "hw", "ip", function]) => {
                if !IP_FUNCTION_DIRS.contains(function) {
                    errors.push(format!(
                        "{}: '{function}' is not an ip function directory; the vocabulary is {}",
                        relative.display(),
                        IP_FUNCTION_DIRS.join(", ")
                    ));
                }
                if stem != name {
                    errors.push(stem_error(name));
                }
                let code = root
                    .join(format!("crates/hw/src/ip/{function}/{}.rs", snake_case(stem)));
                let code_dir = root.join(format!("crates/hw/src/ip/{function}/{}", stem));
                if !code.is_file() && !code_dir.is_dir() {
                    errors.push(mirror_error(&code.display().to_string()));
                }
            }
            ("ip", ["manifests", "hw", "modelkit"]) => {
                // The shared-core library (ip-lib:): flat in modelkit/.
                if stem != name {
                    errors.push(stem_error(name));
                }
                let code = root.join(format!("crates/hw/src/modelkit/{}.rs", snake_case(stem)));
                if !code.is_file() {
                    errors.push(mirror_error(&code.display().to_string()));
                }
            }
            ("part", ["manifests", "hw", "part", domain]) => {
                if !PART_DOMAIN_DIRS.contains(domain) {
                    errors.push(format!(
                        "{}: '{domain}' is not a part domain directory; the vocabulary is {}",
                        relative.display(),
                        PART_DOMAIN_DIRS.join(", ")
                    ));
                }
                if stem != name {
                    errors.push(stem_error(name));
                }
                let code = root
                    .join(format!("crates/hw/src/part/{domain}/{}.rs", snake_case(stem)));
                let code_dir = root.join(format!("crates/hw/src/part/{domain}/{}", stem));
                if !code.is_file() && !code_dir.is_dir() {
                    errors.push(mirror_error(&code.display().to_string()));
                }
            }
            // The responsibility split is structural (ADR 0031 §1): an
            // ip-kind manifest on the part side of the tree (or the
            // reverse) is a classification error, not a style choice.
            ("ip", ["manifests", "hw", "part", _]) => {
                errors.push(format!(
                    "{}: ip-kind model {id:?} belongs under manifests/hw/ip/<function>/ (ADR 0031 §1)",
                    relative.display()
                ));
            }
            ("part", ["manifests", "hw", "ip", _]) => {
                errors.push(format!(
                    "{}: part-kind model {id:?} belongs under manifests/hw/part/<domain>/ (ADR 0031 §1)",
                    relative.display()
                ));
            }
            ("cpu", ["manifests", "isa", arch]) => {
                if stem != name {
                    errors.push(format!(
                        "{}: manifest stem {stem:?} must match the ID name {name:?}",
                        relative.display()
                    ));
                }
                let code =
                    root.join(format!("crates/isa/{arch}/src/cpu/{}.rs", snake_case(stem)));
                if !code.is_file() {
                    errors.push(mirror_error(&code.display().to_string()));
                }
            }
            ("soc" | "board", ["manifests", "machine", arch, machine]) => {
                let code_dir = root.join(format!("crates/machine/src/{arch}/{machine}"));
                if !code_dir.is_dir() {
                    errors.push(mirror_error(&code_dir.display().to_string()));
                }
            }
            _ => errors.push(format!(
                "{}: manifest is not on a mirrored path (manifests/hw/..., manifests/isa/..., manifests/machine/<arch>/<name>/)",
                relative.display()
            )),
        }

        match kind {
            "soc" => match parse_soc(&value) {
                Ok(manifest) => {
                    references.extend(
                        manifest
                            .cpu
                            .iter()
                            .map(|cpu| (relative.clone(), cpu.use_id.clone())),
                    );
                    references.extend(
                        manifest
                            .block
                            .iter()
                            .map(|block| (relative.clone(), block.use_id.clone())),
                    );
                    soc_manifests.insert(
                        manifest.soc.id.clone(),
                        Located {
                            source: relative.clone(),
                            value: manifest,
                        },
                    );
                }
                Err(error) => errors.push(format!("{}: {error}", relative.display())),
            },
            "board" => match parse_board(&value) {
                Ok(manifest) => {
                    if let Some(soc) = &manifest.board.soc {
                        references.push((relative.clone(), soc.clone()));
                    }
                    references.extend(
                        manifest
                            .part
                            .iter()
                            .map(|part| (relative.clone(), part.use_id.clone())),
                    );
                    board_manifests.insert(
                        manifest.board.id.clone(),
                        Located {
                            source: relative.clone(),
                            value: manifest,
                        },
                    );
                }
                Err(error) => errors.push(format!("{}: {error}", relative.display())),
            },
            _ => {}
        }
    }

    for (source, reference) in references {
        if !ids.contains_key(&reference) {
            errors.push(format!(
                "{}: unresolved catalog reference {reference:?}",
                source.display()
            ));
        }
    }

    // Build the normalized declarative graph only after every manifest has been
    // collected, because the board may sort before its SoC file on disk.
    for (id, located) in &soc_manifests {
        if let Err(error) = MachineGraph::build(&located.value, None) {
            errors.push(format!("{}: {error}", located.source.display()));
        }
        if !ids.contains_key(id) {
            errors.push(format!(
                "{}: SoC manifest ID {id:?} was not indexed",
                located.source.display()
            ));
        }
    }
    let mut port_checked_socs = BTreeSet::new();
    for (id, located) in &board_manifests {
        match &located.value.board.soc {
            Some(soc_id) => match soc_manifests.get(soc_id) {
                Some(soc) => match MachineGraph::build(&soc.value, Some(&located.value)) {
                    Ok(graph) => {
                        errors.extend(
                            validate_graph_ports(&graph, &leaf_metadata)
                                .into_iter()
                                .map(|error| format!("{}: {error}", located.source.display())),
                        );
                        port_checked_socs.insert(soc_id.clone());
                    }
                    Err(error) => errors.push(format!("{}: {error}", located.source.display())),
                },
                None => errors.push(format!(
                    "{}: cannot build graph; referenced SoC {soc_id:?} has no valid manifest",
                    located.source.display()
                )),
            },
            // A SoC-less board (multi-chip machine, ADR 0023) has no
            // declarative graph: the machine crate assembles it, and the
            // lint has nothing to build.
            None => {}
        }
        if !ids.contains_key(id) {
            errors.push(format!(
                "{}: board manifest ID {id:?} was not indexed",
                located.source.display()
            ));
        }
    }
    for (id, located) in &soc_manifests {
        if port_checked_socs.contains(id) {
            continue;
        }
        match MachineGraph::build(&located.value, None) {
            Ok(graph) => errors.extend(
                validate_graph_ports(&graph, &leaf_metadata)
                    .into_iter()
                    .map(|error| format!("{}: {error}", located.source.display())),
            ),
            Err(error) => errors.push(format!("{}: {error}", located.source.display())),
        }
    }
    errors.sort();
    Ok(errors)
}

pub fn workspace_root() -> CheckResult<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        // crates/xtask → crates/ → workspace root (ADR 0028)
        .nth(2)
        .ok_or("missing workspace root")?
        .canonicalize()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "mosaic-xtask-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn unicorn_dependency_is_pinned_and_unsafe_is_scoped() {
        let root = workspace_root().unwrap();
        let text = fs::read_to_string(root.join("crates/accel/unicorn/Cargo.toml")).unwrap();
        let manifest: toml::Value = toml::from_str(&text).unwrap();
        let dep = &manifest["dependencies"]["unicorn-engine"];
        assert_eq!(dep["version"].as_str(), Some("=2.1.5"));
        assert_eq!(dep["default-features"].as_bool(), Some(false));
        assert_eq!(
            manifest["lints"]["rust"]["unsafe_code"].as_str(),
            Some("deny")
        );
    }

    #[test]
    fn name_lint_rejects_misc_everywhere() {
        // ADR 0031: the QEMU-style catch-all is gone from every tree.
        let f = Fixture::new();
        f.write("crates/hw/src/ip/char/keep", "");
        f.write("crates/hw/src/part/sensor/keep", "");
        f.write("manifests/hw/part/sensor/keep", "");
        assert!(lint_names(&f.0, &[]).unwrap().is_empty());
        f.write("crates/hw/src/ip/misc/keep", "");
        f.write("manifests/hw/misc/keep", "");
        let errors = lint_names(&f.0, &[]).unwrap();
        assert!(errors.iter().any(|e| e.contains("forbidden directory")));
    }

    #[test]
    fn catalog_lint_rejects_duplicate_global_ids() {
        let fixture = Fixture::new();
        fixture.write("manifests/origins.toml", "[origins]\narm = 'Arm'\n");
        fixture.write(
            "manifests/isa/arm/core-a.toml",
            "[identity]\nid='cpu:arm/core-a'\nsummary='first core'\n",
        );
        fixture.write(
            "manifests/isa/arm/core-b.toml",
            "[identity]\nid='cpu:arm/core-a'\nsummary='second core'\n",
        );
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(errors
            .iter()
            .any(|error| error.contains("already declared")));
    }

    #[test]
    fn leaf_manifest_must_mirror_its_module() {
        let fixture = Fixture::new();
        fixture.write("manifests/origins.toml", "[origins]\narm = 'Arm'\n");
        fixture.write(
            "manifests/hw/ip/char/pl011.toml",
            "[identity]\nid='ip:arm/pl011'\nsummary='PrimeCell UART'\n",
        );
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(
            errors.iter().any(|error| error.contains("must mirror")),
            "{errors:?}"
        );
        // The mirror present: no mirror error any more.
        fixture.write("crates/hw/src/ip/char/pl011.rs", "");
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(
            !errors.iter().any(|error| error.contains("must mirror")),
            "{errors:?}"
        );
    }

    #[test]
    fn the_kind_prefix_must_match_the_side_of_the_tree() {
        // ADR 0031 §1: an ip-kind manifest on the part side of the tree
        // (or the reverse) is a classification error.
        let fixture = Fixture::new();
        fixture.write(
            "manifests/origins.toml",
            "[origins]\narm = 'Arm'\nbosch = 'Bosch'\n",
        );
        fixture.write(
            "manifests/hw/part/sensor/pl011.toml",
            "[identity]\nid='ip:arm/pl011'\nsummary='PrimeCell UART'\n",
        );
        fixture.write("crates/hw/src/part/sensor/pl011.rs", "");
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(errors
            .iter()
            .any(|error| error.contains("belongs under manifests/hw/ip/")));
        fixture.write(
            "manifests/hw/ip/char/bme280.toml",
            "[identity]\nid='part:bosch/bme280'\nsummary='sensor on the wrong side'\n",
        );
        fixture.write("crates/hw/src/ip/char/bme280.rs", "");
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(errors
            .iter()
            .any(|error| error.contains("belongs under manifests/hw/part/")));
    }

    #[test]
    fn part_manifests_may_use_the_identity_form() {
        // ADR 0031 §7: a part that owns ports (USB HID keyboard) keeps the
        // [identity] form with a part: id.
        let fixture = Fixture::new();
        fixture.write("manifests/origins.toml", "[origins]\nusb = 'USB-IF'\n");
        fixture.write(
            "manifests/hw/part/input/hid-keyboard.toml",
            "[identity]\nid='part:usb/hid-keyboard'\nsummary='HID boot keyboard'\nspec=['USB HID 1.11']\nvariants=[]\n\n[tags]\nfunction=['input']\nfidelity='functional'\nverified='spec-tested'\n\n[ports]\nusb={contract='usb/device', role='endpoint'}\n\n[params]\n",
        );
        fixture.write("crates/hw/src/part/input/hid_keyboard.rs", "");
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(
            !errors.iter().any(|error| error.contains("part")),
            "{errors:?}"
        );
    }

    #[test]
    fn device_manifest_must_use_a_function_directory() {
        let fixture = Fixture::new();
        fixture.write("manifests/origins.toml", "[origins]\narm = 'Arm'\n");
        fixture.write(
            "manifests/hw/ip/uart/pl011.toml",
            "[identity]\nid='ip:arm/pl011'\nsummary='PrimeCell UART'\n",
        );
        fixture.write("crates/hw/src/ip/uart/pl011.rs", "");
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(errors
            .iter()
            .any(|error| error.contains("not an ip function directory")));
    }

    #[test]
    fn checked_in_manifests_validate_and_resolve() {
        let root = workspace_root().unwrap();
        let errors = lint_catalog(&root).unwrap();
        assert!(errors.is_empty(), "catalog errors:\n{}", errors.join("\n"));
    }
}
