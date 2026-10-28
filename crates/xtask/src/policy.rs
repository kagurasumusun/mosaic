#![forbid(unsafe_code)]
//! Pure architecture policy for the QEMU-shaped tree (ADR 0024); independent
//! of Cargo metadata transport.
//!
//! The tree is one domain per directory, mirroring QEMU's own top level:
//! `util`, `exec`, `contract`, `runtime`, `isa/<arch>` (QEMU `target/`),
//! `accel/<engine>`, `bus` (the interconnect foundation: one fabric per
//! connection method, ADR 0025), `hw` (device models, one function
//! directory per kind), `boot` (the boot and image-reading foundation:
//! loaders, formats, per-arch boot protocols), `describe`, `assemble`,
//! `machine` (SoC/board models per arch, QEMU `hw/<arch>`), the `mosaic`
//! front-end, `tests`, `xtask`.
//! Dependencies only point downward; `hw` never sees `machine`, and nothing
//! below `machine` sees an engine's concrete types except `accel` itself.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layer {
    pub domain: String,
    pub kind: String,
    pub level: u64,
}

#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub dir: PathBuf,
    pub layer: Layer,
    pub dependencies: Vec<PathBuf>,
}

/// `misc` is deliberately absent: QEMU's one catch-all function directory is
/// the only accepted `misc`, and `lint_names` allows it exactly there
/// (ADR 0024 §3.2).
const FORBIDDEN: &[&str] = &[
    "common", "utils", "generic", "base", "helper", "manager", "handler", "impl", "shared", "new",
    "old", "stuff",
];

/// Every domain's level. One function per line, one direction: downward.
pub fn level_of(kind: &str) -> Option<u64> {
    Some(match kind {
        "util" => 0,
        "exec" => 1,
        // contract=2, isa=2, boot=2 (decode-only: bytes in, typed
        // structure out; boot never sees a sim-side type).
        "contract" | "isa" | "boot" => 2,
        // The interconnect fabrics (3) speak the contract vocabulary only.
        "bus" => 3,
        "runtime" | "accel" => 4,
        "describe" => 5,
        // The composition interface (6): hw's factories implement against
        // it, exactly as QEMU's `hw/` includes `qdev` (ADR 0025).
        "assemble" => 6,
        // The device models (7): contract + bus + runtime + assemble below.
        "hw" => 7,
        "machine" => 8,
        "front" | "tests" => 9,
        "tooling" => 10,
        _ => return None,
    })
}

/// Path is authoritative: metadata cannot turn a device into a machine.
///
/// Workspace crates live under `crates/` (ADR 0028), so every crate path is
/// `crates/<domain>/...`. `isa` and `accel` are family roots: a crate lives
/// at `crates/isa/<arch>` / `crates/accel/<engine>`, so a bare
/// `crates/isa`/`crates/accel` path is not a crate and never validates.
pub fn validate_layer(path: &str, layer: &Layer) -> bool {
    let prefix = match layer.domain.as_str() {
        "util" => "crates/util",
        "exec" => "crates/exec",
        "contract" => "crates/contract",
        "runtime" => "crates/runtime",
        "isa" => "crates/isa",
        "accel" => "crates/accel",
        "hw" => "crates/hw",
        "bus" => "crates/bus",
        "boot" => "crates/boot",
        "describe" => "crates/describe",
        "assemble" => "crates/assemble",
        "machine" => "crates/machine",
        "front" => "crates/mosaic",
        "tests" => "crates/tests",
        "tooling" => "crates/xtask",
        _ => return false,
    };
    if matches!(layer.kind.as_str(), "isa" | "accel") && path == prefix {
        // The family roots are directories of crates, not crates.
        return false;
    }
    layer.kind == layer.domain
        && layer.level == level_of(&layer.kind).unwrap_or(u64::MAX)
        && (path == prefix || path.starts_with(&format!("{prefix}/")))
}

/// Dependencies point downward only, with the single exception that the boot
/// test suite drives the front-end binary's own library at the same level.
pub fn dependency_allowed(from: &Layer, to: &Layer) -> bool {
    // The boot tests drive the front-end library at their own level.
    if from.kind == "tests" && to.kind == "front" {
        return true;
    }
    // Device models never see engine types (ADR 0024 §3.4): the engine
    // vocabulary crosses hw → machine through contracts only.
    if from.kind == "hw" && matches!(to.kind.as_str(), "accel" | "isa") {
        return false;
    }
    // The interconnect foundation stays below every device: fabrics never
    // see models, machines, composition or runtime state (ADR 0025).
    if from.kind == "bus"
        && matches!(
            to.kind.as_str(),
            "hw" | "machine" | "assemble" | "runtime" | "describe" | "accel"
        )
    {
        return false;
    }
    // The boot foundation is decode-only: bytes in, typed structure out,
    // never a runtime, device or machine type.
    if from.kind == "boot"
        && matches!(
            to.kind.as_str(),
            "hw" | "machine"
                | "assemble"
                | "runtime"
                | "accel"
                | "isa"
                | "bus"
                | "contract"
                | "exec"
        )
    {
        return false;
    }
    // CPU models speak the engine contract: QEMU's target/ includes cpu.h.
    // (isa and contract share level 2; this is the one sanctioned edge.)
    if from.kind == "isa" && to.kind == "contract" {
        return true;
    }
    // The engine adapters drive the family's shared conformance fixtures
    // through the CpuEngine contract (ADR 0030: Unicorn is the one engine,
    // the fixtures stay its regression suite). Engines never depend on each
    // other — this edge exists for the fixture crate only.
    if from.kind == "accel" && to.kind == "accel" {
        return true;
    }
    to.level < from.level
}

/// R7: external crates are allowlisted per boundary. The Unicorn binding may
/// only enter at its adapter, TOML only where manifests or ledgers are read.
pub fn external_dependency_allowed(path: &str, layer: &Layer, dependency: &str) -> bool {
    if matches!(dependency, "unicorn-engine" | "unicorn-engine-sys") {
        return path == "crates/accel/unicorn"
            && layer.domain == "accel"
            && layer.kind == "accel"
            && dependency == "unicorn-engine";
    }
    matches!(
        (layer.kind.as_str(), dependency),
        ("tooling", "toml") | ("tooling", "serde_json") | ("tests", "toml") | ("machine", "toml")
    )
}

pub fn lint_layers(packages: &[Package]) -> Vec<String> {
    let by_dir: BTreeMap<_, _> = packages.iter().map(|p| (p.dir.clone(), p)).collect();
    let mut errors = Vec::new();
    for package in packages {
        for dep in &package.dependencies {
            match by_dir.get(dep) {
                Some(target) if dependency_allowed(&package.layer, &target.layer) => {}
                Some(target) => errors.push(format!(
                    "{} -> {}: forbidden dependency",
                    package.name, target.name
                )),
                None => errors.push(format!(
                    "{}: local dependency {} is not a classified workspace member",
                    package.name,
                    dep.display()
                )),
            }
        }
    }
    errors
}

pub fn kebab_case(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

pub fn forbidden_name(s: &str) -> bool {
    s.split('-').any(|word| FORBIDDEN.contains(&word))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(kind: &str) -> Layer {
        Layer {
            domain: kind.into(),
            kind: kind.into(),
            level: level_of(kind).unwrap(),
        }
    }

    #[test]
    fn unicorn_is_allowlisted_only_at_its_adapter_boundary() {
        assert!(external_dependency_allowed(
            "crates/accel/unicorn",
            &layer("accel"),
            "unicorn-engine"
        ));
        assert!(!external_dependency_allowed(
            "crates/machine",
            &layer("machine"),
            "unicorn-engine"
        ));
        assert!(!external_dependency_allowed(
            "crates/hw",
            &layer("hw"),
            "unicorn-engine"
        ));
        assert!(!external_dependency_allowed(
            "crates/accel/unicorn",
            &layer("accel"),
            "unicorn-engine-sys"
        ));
        assert!(!external_dependency_allowed(
            "crates/accel/unicorn",
            &layer("accel"),
            "rand"
        ));
    }

    #[test]
    fn toml_is_only_for_manifest_and_ledger_readers() {
        assert!(external_dependency_allowed(
            "crates/machine",
            &layer("machine"),
            "toml"
        ));
        assert!(external_dependency_allowed(
            "crates/tests",
            &layer("tests"),
            "toml"
        ));
        assert!(external_dependency_allowed(
            "crates/xtask",
            &layer("tooling"),
            "toml"
        ));
        assert!(!external_dependency_allowed(
            "crates/hw",
            &layer("hw"),
            "toml"
        ));
        assert!(!external_dependency_allowed(
            "crates/mosaic",
            &layer("front"),
            "toml"
        ));
    }

    #[test]
    fn dependencies_point_downward_only() {
        // The two-axis boundary (ADR 0024 §3.4): machines reach hw, never
        // the other way around.
        assert!(dependency_allowed(&layer("machine"), &layer("hw")));
        assert!(!dependency_allowed(&layer("hw"), &layer("machine")));
        assert!(dependency_allowed(&layer("hw"), &layer("contract")));
        assert!(dependency_allowed(&layer("machine"), &layer("accel")));
        assert!(dependency_allowed(&layer("accel"), &layer("isa")));
        // Engines and devices sit at the same level and stay strangers.
        assert!(!dependency_allowed(&layer("accel"), &layer("hw")));
        assert!(!dependency_allowed(&layer("hw"), &layer("accel")));
        assert!(!dependency_allowed(&layer("runtime"), &layer("accel")));
        // The boot tests drive the front-end library at their own level.
        assert!(dependency_allowed(&layer("tests"), &layer("front")));
        assert!(!dependency_allowed(&layer("front"), &layer("tests")));
    }

    #[test]
    fn metadata_cannot_spoof_a_layer() {
        assert!(!validate_layer("crates/hw", &layer("machine")));
        assert!(validate_layer("crates/hw", &layer("hw")));
        assert!(validate_layer("crates/isa/arm", &layer("isa")));
        assert!(!validate_layer("crates/isa", &layer("isa")));
        assert!(validate_layer("crates/mosaic", &layer("front")));
        assert!(!validate_layer("crates/apps/system", &layer("front")));
        // The bare pre-crates/ spellings must no longer validate (ADR 0028).
        assert!(!validate_layer("hw", &layer("hw")));
        assert!(!validate_layer("mosaic", &layer("front")));
    }

    #[test]
    fn names_use_tokens_not_substrings() {
        assert!(forbidden_name("mosaic-hw-utils"));
        assert!(!forbidden_name("mosaic-hw-intc"));
        assert!(!forbidden_name("newton"));
        for invalid in ["", "PL011", "foo_bar", "foo--bar", "-foo", "foo-"] {
            assert!(!kebab_case(invalid));
        }
        assert!(kebab_case("dw-apb-i2c"));
    }

    #[test]
    fn unresolved_local_dependency_is_not_an_escape_hatch() {
        let package = Package {
            name: "test".into(),
            dir: "/repo/test".into(),
            layer: layer("exec"),
            dependencies: vec!["/external/unclassified".into()],
        };
        assert_eq!(lint_layers(&[package]).len(), 1);
    }
}
