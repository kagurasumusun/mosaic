#![forbid(unsafe_code)]
//! Architecture checks use all declared dependencies, not only active features.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
            "framework" | "catalog" | "apps" | "xtask"
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
        } else if let Some(rest) = path.strip_prefix("framework/") {
            format!("mosaic-{}", rest.replace('/', "-"))
        } else if let Some(rest) = path.strip_prefix("apps/") {
            format!("mosaic-apps-{}", rest.replace('/', "-"))
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
            ["arch_arm"]
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
}
