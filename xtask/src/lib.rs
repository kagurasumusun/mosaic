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
    dependency_allowed, external_dependency_allowed, forbidden_name, kebab_case, lint_layers,
    validate_layer, Layer, Package,
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
        if name.starts_with('.') || matches!(name.as_ref(), "target" | "node_modules") {
            continue;
        }
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            directories.push(entry.path());
            walk_dirs(&entry.path(), directories)?;
        }
    }
    Ok(())
}

pub fn lint_names(root: &Path, packages: &[Package]) -> CheckResult<Vec<String>> {
    let mut errors = Vec::new();
    let mut directories = Vec::new();
    walk_dirs(root, &mut directories)?;
    let text = fs::read_to_string(root.join("catalog/origins.toml")).map_err(|e| e.to_string())?;
    let origins: toml::Value = toml::from_str(&text).map_err(|e| e.to_string())?;
    let origins = origins
        .get("origins")
        .and_then(toml::Value::as_table)
        .ok_or("origins must be a table")?;
    for origin in origins.keys() {
        if !kebab_case(origin) || forbidden_name(origin) {
            errors.push(format!("invalid canonical origin: {origin}"));
        }
    }
    for directory in directories {
        let relative = directory.strip_prefix(root).map_err(|e| e.to_string())?;
        let parts: Vec<_> = relative
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let name = parts.last().ok_or("empty directory path")?;
        if forbidden_name(name) {
            errors.push(format!("{}: forbidden directory name", relative.display()));
        }
        if matches!(
            parts[0].as_str(),
            "framework" | "catalog" | "apps" | "tests" | "xtask"
        ) && !kebab_case(name)
        {
            errors.push(format!(
                "{}: directory must use kebab-case",
                relative.display()
            ));
        }
        if parts[0] == "catalog"
            && parts.len() == 3
            && parts[1] != "firmware"
            && parts[1] != "world"
            && !origins.contains_key(name)
        {
            errors.push(format!("{}: unknown origin", relative.display()));
        }
    }
    for package in packages {
        let path = package
            .dir
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy();
        let expected = if path == "xtask" {
            "mosaic-xtask".into()
        } else if path == "tests" {
            // The machine-level boot-test package: the design document's
            // `tests/` root (§6), one crate, not a per-test crate tree.
            "mosaic-tests-boot".into()
        } else if let Some(rest) = path.strip_prefix("framework/") {
            format!("mosaic-{}", rest.replace('/', "-"))
        } else if let Some(rest) = path.strip_prefix("apps/") {
            format!("mosaic-apps-{}", rest.replace('/', "-"))
        } else if let Some(rest) = path.strip_prefix("tests/") {
            format!("mosaic-tests-{}", rest.replace('/', "-"))
        } else {
            let rest = path
                .strip_prefix("catalog/")
                .ok_or("invalid catalog path")?;
            format!(
                "mosaic-{}",
                rest.replacen("parts/", "part/", 1).replace('/', "-")
            )
        };
        if package.name != expected || !kebab_case(&package.name) || forbidden_name(&package.name) {
            errors.push(format!("{}: expected crate name {expected}", package.name));
        }
    }
    Ok(errors)
}

/// Validate typed SoC/board manifests, enforce globally unique catalog IDs,
/// and resolve every `use`/`soc` reference to a declared catalog object.
/// The describe layer owns format-independent schema/semantic checks; tooling
/// owns TOML parsing, filesystem traversal, and repository-wide policy.
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
            } else if matches!(
                name.as_ref(),
                "cpu.toml" | "ip.toml" | "part.toml" | "soc.toml" | "board.toml"
            ) {
                files.push(path);
            }
        }
        Ok(())
    }

    let catalog_root = root.join("catalog");
    let origins_text = fs::read_to_string(catalog_root.join("origins.toml"))
        .map_err(|error| format!("catalog/origins.toml: {error}"))?;
    let origins_value: toml::Value =
        toml::from_str(&origins_text).map_err(|error| format!("catalog/origins.toml: {error}"))?;
    let origins_table = origins_value
        .get("origins")
        .and_then(toml::Value::as_table)
        .ok_or("catalog/origins.toml: [origins] must be a table")?;
    let known_origins: BTreeSet<String> = origins_table.keys().cloned().collect();
    let mut files = Vec::new();
    collect(&catalog_root, &mut files)?;
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

        const CPU_PREFIXES: &[&str] = &["cpu:"];
        const IP_PREFIXES: &[&str] = &["ip:", "ip-lib:"];
        const PART_PREFIXES: &[&str] = &["part:"];
        const SOC_PREFIXES: &[&str] = &["soc:"];
        const BOARD_PREFIXES: &[&str] = &["board:"];
        if let Some(file_name) = file.file_name().and_then(|name| name.to_str()) {
            if matches!(file_name, "cpu.toml" | "ip.toml" | "part.toml") {
                if let Err(error) =
                    validate_leaf_catalog_manifest(file_name, &value, &known_origins)
                {
                    errors.push(format!("{}: {error}", relative.display()));
                }
            }
        }

        let expected: Option<(&str, &[&str])> =
            match file.file_name().and_then(|name| name.to_str()) {
                Some("cpu.toml") => Some(("identity", CPU_PREFIXES)),
                Some("ip.toml") => Some(("identity", IP_PREFIXES)),
                Some("part.toml") => Some(("part", PART_PREFIXES)),
                Some("soc.toml") => Some(("soc", SOC_PREFIXES)),
                Some("board.toml") => Some(("board", BOARD_PREFIXES)),
                _ => None,
            };
        let id = expected
            .and_then(|(section, _)| value.get(section).and_then(|v| v.get("id")))
            .and_then(toml::Value::as_str)
            .map(str::to_owned);
        if matches!(
            file.file_name().and_then(|name| name.to_str()),
            Some("cpu.toml" | "ip.toml" | "part.toml")
        ) {
            if let Some(catalog_id) = id.as_ref() {
                leaf_metadata.insert(catalog_id.clone(), value.clone());
            }
        }

        match (expected, id) {
            (Some((section, prefixes)), Some(id)) if !id.trim().is_empty() => {
                if !prefixes.iter().any(|prefix| id.starts_with(prefix)) {
                    errors.push(format!(
                        "{}: ID {id:?} must use one of the expected namespaces: {}",
                        relative.display(),
                        prefixes.join(", ")
                    ));
                }
                let path_prefix = match file.file_name().and_then(|name| name.to_str()) {
                    Some("cpu.toml") => Some(Path::new("catalog/cpu")),
                    Some("ip.toml") => Some(Path::new("catalog/ip")),
                    Some("part.toml") => Some(Path::new("catalog/parts")),
                    Some("soc.toml") => Some(Path::new("catalog/soc")),
                    Some("board.toml") => Some(Path::new("catalog/board")),
                    _ => None,
                };
                if let (Some(prefix), Some(parent), Some((_, id_suffix))) =
                    (path_prefix, relative.parent(), id.split_once(':'))
                {
                    if let Ok(relative_id) = parent.strip_prefix(prefix) {
                        let canonical_suffix = relative_id.to_string_lossy().replace('\\', "/");
                        if canonical_suffix != id_suffix {
                            errors.push(format!(
                                "{}: ID suffix {id_suffix:?} must match canonical catalog \
                                 path {canonical_suffix:?}",
                                relative.display()
                            ));
                        }
                    }
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
            }
            _ => errors.push(format!(
                "{}: expected a non-empty id and summary in the manifest's expected section",
                relative.display()
            )),
        }

        match file.file_name().and_then(|name| name.to_str()) {
            Some("soc.toml") => match parse_soc(&value) {
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
            Some("board.toml") => match parse_board(&value) {
                Ok(manifest) => {
                    references.push((relative.clone(), manifest.board.soc.clone()));
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
        match soc_manifests.get(&located.value.board.soc) {
            Some(soc) => match MachineGraph::build(&soc.value, Some(&located.value)) {
                Ok(graph) => {
                    errors.extend(
                        validate_graph_ports(&graph, &leaf_metadata)
                            .into_iter()
                            .map(|error| format!("{}: {error}", located.source.display())),
                    );
                    port_checked_socs.insert(located.value.board.soc.clone());
                }
                Err(error) => errors.push(format!("{}: {error}", located.source.display())),
            },
            None => errors.push(format!(
                "{}: cannot build graph; referenced SoC {:?} has no valid manifest",
                located.source.display(),
                located.value.board.soc
            )),
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
        .parent()
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
    fn unicorn_dependency_is_pinned_arm_only_and_unsafe_is_scoped() {
        let root = workspace_root().unwrap();
        let text = fs::read_to_string(root.join("framework/engine/unicorn/Cargo.toml")).unwrap();
        let manifest: toml::Value = toml::from_str(&text).unwrap();
        let dep = &manifest["dependencies"]["unicorn-engine"];
        assert_eq!(dep["version"].as_str(), Some("=2.1.5"));
        assert_eq!(dep["default-features"].as_bool(), Some(false));
        assert_eq!(
            dep["features"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect::<Vec<_>>(),
            ["arch_arm", "arch_aarch64", "arch_x86"]
        );
        assert_eq!(
            manifest["lints"]["rust"]["unsafe_code"].as_str(),
            Some("deny")
        );
    }

    #[test]
    fn cargo_metadata_checks_optional_dev_and_inactive_target_dependencies() {
        let f = Fixture::new();
        f.write("Cargo.toml", "[workspace]\nresolver = '2'\nmembers = ['framework/core/types', 'framework/core/runtime']\n");
        for kind in ["types", "runtime"] {
            let level = if kind == "types" { 1 } else { 3 };
            let mut manifest = format!(
                "[package]\nname = 'mosaic-core-{kind}'\nversion = '0.1.0'\nedition = '2021'\n[package.metadata.layer]\ndomain = 'framework'\nkind = '{kind}'\nlevel = {level}\n"
            );
            if kind == "types" {
                manifest.push_str(
                    "[dependencies]\nrenamed = { package = 'mosaic-core-runtime', path = '../runtime', optional = true }\n[dev-dependencies]\nrenamed = { package = 'mosaic-core-runtime', path = '../runtime' }\n[target.'cfg(any())'.build-dependencies]\nrenamed = { package = 'mosaic-core-runtime', path = '../runtime' }\n"
                );
            }
            f.write(&format!("framework/core/{kind}/Cargo.toml"), &manifest);
            f.write(&format!("framework/core/{kind}/src/lib.rs"), "");
        }
        let packages = load_packages(&f.0.canonicalize().unwrap()).unwrap();
        let errors = lint_layers(&packages);
        assert_eq!(errors.len(), 3, "{errors:?}");
    }

    #[test]
    fn name_lint_checks_origins_and_forbidden_directory_tokens() {
        let f = Fixture::new();
        f.write("catalog/origins.toml", "[origins]\narm = 'Arm'\n");
        f.write("catalog/ip/stmicro/pl011/src/lib.rs", "");
        f.write("framework/modelkit/uart-utils/README.md", "");
        let errors = lint_names(&f.0, &[]).unwrap();
        assert!(errors.iter().any(|e| e.contains("unknown origin")));
        assert!(errors.iter().any(|e| e.contains("forbidden directory")));
    }

    #[test]
    fn name_lint_accepts_a_canonical_origin() {
        let f = Fixture::new();
        f.write("catalog/origins.toml", "[origins]\narm = 'Arm'\n");
        f.write("catalog/ip/arm/pl011/src/lib.rs", "");
        assert!(lint_names(&f.0, &[]).unwrap().is_empty());
    }

    #[test]
    fn catalog_lint_rejects_duplicate_global_ids() {
        let fixture = Fixture::new();
        fixture.write("catalog/origins.toml", "[origins]\narm = 'Arm'\n");
        fixture.write(
            "catalog/cpu/arm/core-a/cpu.toml",
            "[identity]\nid='cpu:test'\nsummary='first core'\n",
        );
        fixture.write(
            "catalog/cpu/arm/core-b/cpu.toml",
            "[identity]\nid='cpu:test'\nsummary='second core'\n",
        );
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(errors
            .iter()
            .any(|error| error.contains("already declared")));
    }

    #[test]
    fn catalog_id_suffix_must_match_canonical_path() {
        let fixture = Fixture::new();
        fixture.write("catalog/origins.toml", "[origins]\narm = 'Arm'\n");
        fixture.write(
            "catalog/cpu/arm/core-a/cpu.toml",
            "[identity]\nid='cpu:other/core-a'\nsummary='test core'\n",
        );
        let errors = lint_catalog(&fixture.0).unwrap();
        assert!(errors
            .iter()
            .any(|error| error.contains("match canonical catalog path")));
    }

    #[test]
    fn checked_in_catalog_manifests_validate_and_resolve() {
        let root = workspace_root().unwrap();
        let errors = lint_catalog(&root).unwrap();
        assert!(errors.is_empty(), "catalog errors:\n{}", errors.join("\n"));
    }
}
