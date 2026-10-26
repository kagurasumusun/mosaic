#![forbid(unsafe_code)]
//! Pure architecture policy; independent of Cargo metadata transport.

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

const CATALOG_KINDS: &[&str] = &[
    "cpu", "ip", "ip-lib", "part", "subsys", "soc", "module", "board", "world",
];
const COMPOSITE_KINDS: &[&str] = &["subsys", "soc", "module", "board", "world"];
const FORBIDDEN: &[&str] = &[
    "misc", "common", "utils", "util", "generic", "base", "helper", "manager", "handler", "impl",
    "shared", "new", "old", "stuff",
];

fn framework_level(kind: &str) -> Option<u64> {
    match kind {
        "prim" => Some(0),
        "types" => Some(1),
        "contract" | "modelkit" | "isa" => Some(2),
        "runtime" | "interconnect" | "engine" => Some(3),
        "describe" => Some(4),
        "assemble" | "host" | "ops" | "bridge" => Some(5),
        _ => None,
    }
}

/// Path is authoritative: metadata cannot turn a model into an application.
pub fn validate_layer(path: &str, layer: &Layer) -> bool {
    let kind = layer.kind.as_str();
    match layer.domain.as_str() {
        "framework" => {
            let prefix = match kind {
                "types" => "framework/core/types".to_string(),
                "runtime" => "framework/core/runtime".to_string(),
                _ => format!("framework/{kind}"),
            };
            framework_level(kind) == Some(layer.level)
                && (path == prefix || path.starts_with(&(prefix + "/")))
        }
        "catalog" => {
            let directory = match kind {
                "part" => "parts",
                "ip-lib" => "ip",
                _ => kind,
            };
            CATALOG_KINDS.contains(&kind)
                && layer.level == 3
                && path.starts_with(&format!("catalog/{directory}/"))
                && (!COMPOSITE_KINDS.contains(&kind) || path.ends_with("/glue"))
        }
        "apps" => kind == "app" && layer.level == 6 && path.starts_with("apps/"),
        // Machine-level guest tests: the design document's `tests/` directory
        // (§6), holding real-image boot tests. Same rights as apps, because a
        // boot test assembles a whole machine the same way an app does.
        "tests" => {
            kind == "machine" && layer.level == 6 && (path == "tests" || path.starts_with("tests/"))
        }
        "tooling" => kind == "tooling" && layer.level == 7 && path == "xtask",
        _ => false,
    }
}

/// R1/R2/R3/R5. Equal-level framework dependencies are explicitly enumerated.
pub fn dependency_allowed(from: &Layer, to: &Layer) -> bool {
    let f = from.kind.as_str();
    let t = to.kind.as_str();
    match from.domain.as_str() {
        "tooling" | "apps" | "tests" => true,
        "catalog" => {
            if to.domain == "catalog" {
                return !COMPOSITE_KINDS.contains(&f) && t == "ip-lib";
            }
            if to.domain != "framework" {
                return false;
            }
            if COMPOSITE_KINDS.contains(&f) {
                return matches!(t, "types" | "contract");
            }
            matches!(t, "prim" | "types" | "contract" | "modelkit" | "isa")
        }
        "framework" if to.domain == "framework" => {
            if to.level < from.level {
                return true;
            }
            matches!((f, t), ("contract", "contract") | ("modelkit", "contract"))
        }
        _ => false,
    }
}

/// R7: only this exact engine boundary may bring in the native CPU backend.
pub fn external_dependency_allowed(path: &str, layer: &Layer, dependency: &str) -> bool {
    if matches!(dependency, "unicorn-engine" | "unicorn-engine-sys") {
        return path == "framework/engine/unicorn"
            && layer.domain == "framework"
            && layer.kind == "engine"
            && layer.level == 3
            && dependency == "unicorn-engine";
    }
    matches!(layer.domain.as_str(), "tooling" | "apps" | "tests")
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

    fn layer(domain: &str, kind: &str, level: u64) -> Layer {
        Layer {
            domain: domain.into(),
            kind: kind.into(),
            level,
        }
    }
    fn fw(kind: &str) -> Layer {
        layer("framework", kind, framework_level(kind).unwrap())
    }
    fn cat(kind: &str) -> Layer {
        layer("catalog", kind, 3)
    }

    #[test]
    fn unicorn_is_allowlisted_only_at_its_adapter_boundary() {
        assert!(external_dependency_allowed(
            "framework/engine/unicorn",
            &fw("engine"),
            "unicorn-engine"
        ));
        assert!(!external_dependency_allowed(
            "apps/cli",
            &layer("apps", "app", 6),
            "unicorn-engine"
        ));
        assert!(!external_dependency_allowed(
            "framework/engine/native",
            &fw("engine"),
            "unicorn-engine"
        ));
        assert!(!external_dependency_allowed(
            "catalog/cpu/arm/cortex-a9",
            &cat("cpu"),
            "unicorn-engine"
        ));
        assert!(!external_dependency_allowed(
            "framework/engine/unicorn",
            &fw("engine"),
            "unicorn-engine-sys"
        ));
        assert!(!external_dependency_allowed(
            "framework/engine/unicorn",
            &fw("engine"),
            "rand"
        ));
    }

    #[test]
    fn framework_cannot_depend_on_catalog_or_upper_layers() {
        assert!(!dependency_allowed(&fw("runtime"), &cat("ip")));
        assert!(!dependency_allowed(&fw("types"), &fw("contract")));
        assert!(!dependency_allowed(&fw("runtime"), &fw("engine")));
        assert!(!dependency_allowed(&fw("engine"), &fw("interconnect")));
        assert!(dependency_allowed(&fw("runtime"), &fw("contract")));
    }
    #[test]
    fn contracts_can_share_contracts_but_not_model_implementations() {
        assert!(dependency_allowed(&fw("contract"), &fw("contract")));
        assert!(dependency_allowed(&fw("modelkit"), &fw("contract")));
        assert!(!dependency_allowed(&fw("contract"), &fw("modelkit")));
    }
    #[test]
    fn catalog_dependencies_are_restricted() {
        for kind in ["prim", "types", "contract", "modelkit", "isa"] {
            assert!(dependency_allowed(&cat("ip"), &fw(kind)));
        }
        for kind in [
            "runtime", "engine", "assemble", "describe", "host", "ops", "bridge",
        ] {
            assert!(!dependency_allowed(&cat("ip"), &fw(kind)));
        }
        assert!(!dependency_allowed(&cat("ip"), &cat("ip")));
        assert!(!dependency_allowed(&cat("part"), &cat("cpu")));
        assert!(dependency_allowed(&cat("ip"), &cat("ip-lib")));
    }
    #[test]
    fn glue_can_only_use_types_and_contracts() {
        for kind in COMPOSITE_KINDS {
            assert!(dependency_allowed(&cat(kind), &fw("contract")));
            assert!(dependency_allowed(&cat(kind), &fw("types")));
            assert!(!dependency_allowed(&cat(kind), &fw("prim")));
            assert!(!dependency_allowed(&cat(kind), &fw("modelkit")));
            assert!(!dependency_allowed(&cat(kind), &cat("ip-lib")));
        }
    }
    #[test]
    fn metadata_cannot_spoof_a_layer() {
        assert!(!validate_layer("catalog/ip/arm/pl011", &fw("prim")));
        assert!(!validate_layer("framework/core/runtime", &fw("types")));
        assert!(!validate_layer(
            "catalog/board/raspberrypi/pico",
            &cat("board")
        ));
        assert!(validate_layer(
            "catalog/board/raspberrypi/pico/glue",
            &cat("board")
        ));
        assert!(validate_layer("catalog/parts/bosch/bme280", &cat("part")));
        assert!(!validate_layer("framework/core/types-extra", &fw("types")));
    }
    #[test]
    fn names_use_tokens_not_substrings() {
        assert!(forbidden_name("mosaic-ip-arm-utils"));
        assert!(!forbidden_name("mosaic-ip-arm-pl011"));
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
            layer: fw("types"),
            dependencies: vec!["/external/unclassified".into()],
        };
        assert_eq!(lint_layers(&[package]).len(), 1);
    }
}
