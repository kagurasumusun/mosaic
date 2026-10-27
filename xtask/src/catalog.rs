//! TOML adapter for the format-independent `mosaic-describe` schema.
//!
//! Keeping this parser in tooling preserves the framework's no-general-purpose
//! external-dependency policy. Runtime and model crates consume typed values,
//! not TOML parser types.

use std::collections::{BTreeMap, BTreeSet};

use mosaic_describe::{
    AddressSpaceView, BlockInstance, BoardConnection, BoardIdentity, BoardManifest, ConfigValue,
    CpuInstance, GraphNode, GraphNodeKind, IrqRoute, MachineGraph, PartInstance, Placement,
    RegionDescriptor, RegionKind, SocIdentity, SocManifest,
};

/// Check the normalized graph against catalog endpoint declarations. This is
/// deliberately a tooling concern: `mosaic-describe` stays format-independent.
pub(super) fn validate_graph_ports(
    graph: &MachineGraph,
    catalog: &BTreeMap<String, toml::Value>,
) -> Vec<String> {
    let nodes: BTreeMap<&str, &GraphNode> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut errors = Vec::new();
    for edge in &graph.edges {
        if edge.kind == "address-space" {
            // This is a synthetic graph relation, not a hardware port.
            continue;
        }
        if let Some(node) = nodes.get(edge.from_node.as_str()) {
            if let Some(catalog_id) = &node.catalog_id {
                match catalog.get(catalog_id) {
                    Some(manifest) => {
                        if let Err(error) = validate_graph_endpoint(
                            node,
                            manifest,
                            &edge.from_port,
                            EndpointSide::Source,
                        ) {
                            errors.push(format!("edge {:?} source: {error}", edge.id));
                        }
                    }
                    None => errors.push(format!(
                        "edge {:?} source: no catalog manifest for {:?}",
                        edge.id, catalog_id
                    )),
                }
            }
        }
        if let Some(node) = nodes.get(edge.to_node.as_str()) {
            if let Some(catalog_id) = &node.catalog_id {
                match catalog.get(catalog_id) {
                    Some(manifest) => {
                        if let Err(error) = validate_graph_endpoint(
                            node,
                            manifest,
                            &edge.to_port,
                            EndpointSide::Target,
                        ) {
                            errors.push(format!("edge {:?} target: {error}", edge.id));
                        }
                    }
                    None => errors.push(format!(
                        "edge {:?} target: no catalog manifest for {:?}",
                        edge.id, catalog_id
                    )),
                }
            }
        }
    }
    // Region mappings are also endpoint uses: every MMIO window must point to
    // a catalog model exposing a compatible `mmio` target port. This closes a
    // gap that ordinary graph edges cannot catch because address mappings are
    // represented as regions rather than signal/bus edges.
    for region in &graph.regions {
        if region.kind != mosaic_describe::RegionKind::Mmio {
            continue;
        }
        let Some(node) = nodes.get(region.target_node.as_str()) else {
            // `MachineGraph::build` validates graph node references first.
            errors.push(format!(
                "region {:?}: unknown target node {:?}",
                region.name, region.target_node
            ));
            continue;
        };
        let Some(catalog_id) = node.catalog_id.as_ref() else {
            errors.push(format!(
                "region {:?}: MMIO target {:?} has no catalog identity",
                region.name, region.target_node
            ));
            continue;
        };
        let Some(manifest) = catalog.get(catalog_id) else {
            errors.push(format!(
                "region {:?}: no catalog manifest for {:?}",
                region.name, catalog_id
            ));
            continue;
        };
        let mmio = manifest
            .get("ports")
            .and_then(|value| value.get("mmio"))
            .and_then(toml::Value::as_table);
        let Some(mmio) = mmio else {
            errors.push(format!(
                "region {:?}: target {:?} has no declared [ports.mmio]",
                region.name, region.target_node
            ));
            continue;
        };
        if mmio.get("role").and_then(toml::Value::as_str) != Some("target") {
            errors.push(format!(
                "region {:?}: {:?} [ports.mmio].role must be 'target'",
                region.name, region.target_node
            ));
        }
        match mmio.get("size").and_then(toml::Value::as_integer) {
            Some(size) if size > 0 && u64::try_from(size).is_ok() => {
                let declared_size = size as u64;
                if region.size > declared_size {
                    errors.push(format!(
                        "region {:?}: size {:#x} exceeds {:?} MMIO window {:#x}",
                        region.name, region.size, region.target_node, declared_size
                    ));
                }
            }
            _ => errors.push(format!(
                "region {:?}: target {:?} [ports.mmio].size must be a positive integer",
                region.name, region.target_node
            )),
        }
    }

    errors
}

#[derive(Clone, Copy)]
enum EndpointSide {
    Source,
    Target,
}

fn validate_graph_endpoint(
    node: &GraphNode,
    manifest: &toml::Value,
    label: &str,
    side: EndpointSide,
) -> Result<(), String> {
    if label.is_empty() {
        return Err("port label is empty".into());
    }
    if node.kind == GraphNodeKind::Part {
        let part = manifest
            .get("part")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| "part manifest has no [part] table".to_owned())?;
        let bindings = part
            .get("bindings")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| "part manifest has no bindings array".to_owned())?;
        match side {
            EndpointSide::Target => {
                if bindings
                    .iter()
                    .any(|binding| binding.as_str() == Some(label))
                {
                    return Ok(());
                }
                return Err(format!(
                    "part {:?} does not declare binding {label:?}",
                    node.id
                ));
            }
            EndpointSide::Source => {
                if let Some(index) = label.strip_prefix("channel") {
                    let index = index.parse::<u64>().map_err(|_| {
                        format!(
                            "part {:?} has malformed channel endpoint {label:?}",
                            node.id
                        )
                    })?;
                    let channels = manifest
                        .get("i2c")
                        .and_then(|value| value.get("channels"))
                        .and_then(toml::Value::as_integer)
                        .and_then(|value| u64::try_from(value).ok())
                        .ok_or_else(|| {
                            format!("part {:?} has no declared channel count", node.id)
                        })?;
                    if index < channels {
                        return Ok(());
                    }
                    return Err(format!(
                        "part {:?} channel index {index} is outside 0..{channels}",
                        node.id
                    ));
                }
                return Err(format!(
                    "part {:?} source port {label:?} is not a declared channel",
                    node.id
                ));
            }
        }
    }

    let ports = manifest
        .get("ports")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("catalog model for {:?} has no [ports] table", node.id))?;
    let role_allowed = |role: &str| match side {
        EndpointSide::Source => matches!(role, "source" | "endpoint" | "initiator"),
        EndpointSide::Target => matches!(role, "sink" | "endpoint"),
    };
    let has_pin = |port: &toml::Value, suffix: &str| {
        ["pins", "names"].iter().any(|key| {
            port.get(key)
                .and_then(toml::Value::as_array)
                .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(suffix)))
        })
    };

    let (base, suffix) = match label.split_once(':') {
        Some((base, suffix)) if !base.is_empty() && !suffix.is_empty() && !suffix.contains(':') => {
            (base, Some(suffix))
        }
        Some(_) => return Err(format!("malformed port path {label:?}")),
        None => (label, None),
    };

    if let Some(port) = ports.get(base) {
        let role = port
            .get("role")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| format!("port {base:?} has no role"))?;
        if !role_allowed(role) {
            return Err(format!(
                "port {base:?} role {role:?} cannot be used on this edge side"
            ));
        }
        if let Some(suffix) = suffix {
            if let Ok(index) = suffix.parse::<u64>() {
                let count = port
                    .get("count")
                    .and_then(toml::Value::as_integer)
                    .and_then(|value| u64::try_from(value).ok())
                    .ok_or_else(|| format!("port {base:?} has no count for indexed endpoint"))?;
                if index >= count {
                    return Err(format!("port {base:?} index {index} is outside 0..{count}"));
                }
                return Ok(());
            }
            if !has_pin(port, suffix) {
                return Err(format!(
                    "port {base:?} does not declare pin/name {suffix:?}"
                ));
            }
        }
        return Ok(());
    }

    if suffix.is_some() {
        return Err(format!(
            "model {:?} does not declare port group {base:?}",
            node.id
        ));
    }
    for port in ports.values() {
        let role = port.get("role").and_then(toml::Value::as_str).unwrap_or("");
        if role_allowed(role) && has_pin(port, label) {
            return Ok(());
        }
    }
    Err(format!(
        "model {:?} does not declare port/pin {label:?}",
        node.id
    ))
}

/// Validate the fixed portions of CPU/IP/part catalog manifests. Port and
/// parameter names remain model-defined, but their property tables are strict
/// so misspelled schema keys don't silently become inert metadata.
pub(super) fn validate_leaf_catalog_manifest(
    file_name: &str,
    value: &toml::Value,
    known_origins: &BTreeSet<String>,
) -> Result<(), String> {
    let root = value
        .as_table()
        .ok_or_else(|| format!("{file_name}: expected a top-level table"))?;
    match file_name {
        "cpu.toml" | "ip.toml" => {
            let allowed_root: &[&str] = if file_name == "cpu.toml" {
                &["identity", "tags", "ports", "engine", "params"]
            } else {
                &["identity", "tags", "ports", "params"]
            };
            reject_unknown(root, allowed_root, file_name)?;

            let identity = section(value, "identity", file_name)?;
            reject_unknown(identity, &["id", "summary", "spec", "variants"], "identity")?;
            for key in ["id", "summary"] {
                string(identity, key, "identity")?;
            }
            string_array(identity, "spec", "identity", true)?;
            string_array(identity, "variants", "identity", false)?;

            let tags = section(value, "tags", file_name)?;
            reject_unknown(tags, &["function", "fidelity", "verified"], "tags")?;
            string_array(tags, "function", "tags", true)?;
            string(tags, "fidelity", "tags")?;
            string(tags, "verified", "tags")?;

            let ports = section(value, "ports", file_name)?;
            for (name, raw_port) in ports {
                if !valid_manifest_name(name) {
                    return Err(format!("ports: invalid port name {name:?}"));
                }
                let scope = format!("ports.{name}");
                let port = raw_port
                    .as_table()
                    .ok_or_else(|| format!("{scope}: expected a table"))?;
                reject_unknown(
                    port,
                    &[
                        "role", "size", "pins", "contract", "optional", "count", "names",
                    ],
                    &scope,
                )?;
                let role = string(port, "role", &scope)?;
                if !matches!(
                    role.as_str(),
                    "target" | "source" | "sink" | "endpoint" | "initiator"
                ) {
                    return Err(format!("{scope}.role: unsupported role {role:?}"));
                }
                if let Some(size) = port.get("size") {
                    positive_integer(size, &format!("{scope}.size"))?;
                }
                let count = if let Some(count) = port.get("count") {
                    let count = count
                        .as_integer()
                        .ok_or_else(|| format!("{scope}.count: expected an integer"))?;
                    if count <= 0 {
                        return Err(format!("{scope}.count: expected a positive integer"));
                    }
                    Some(usize::try_from(count).map_err(|_| {
                        format!("{scope}.count: value does not fit this host's usize")
                    })?)
                } else {
                    None
                };
                if let Some(contract) = port.get("contract") {
                    let contract = contract
                        .as_str()
                        .ok_or_else(|| format!("{scope}.contract: expected a string"))?;
                    let parts: Vec<_> = contract.split('/').collect();
                    if parts.len() != 2 || parts.iter().any(|part| !valid_manifest_name(part)) {
                        return Err(format!(
                            "{scope}.contract: expected a non-empty namespace/name pair"
                        ));
                    }
                }
                if let Some(optional) = port.get("optional") {
                    if optional.as_bool().is_none() {
                        return Err(format!("{scope}.optional: expected a boolean"));
                    }
                }
                for key in ["pins", "names"] {
                    if port.contains_key(key) {
                        let values = string_array(port, key, &scope, true)?;
                        if let Some(count) = count {
                            if values.len() != count {
                                return Err(format!(
                                    "{scope}.{key}: contains {} items but count is {count}",
                                    values.len()
                                ));
                            }
                        }
                    }
                }
            }

            let params = section(value, "params", file_name)?;
            for (name, raw_param) in params {
                if !valid_manifest_name(name) {
                    return Err(format!("params: invalid parameter name {name:?}"));
                }
                let scope = format!("params.{name}");
                let param = raw_param
                    .as_table()
                    .ok_or_else(|| format!("{scope}: expected a table"))?;
                reject_unknown(param, &["type", "default"], &scope)?;
                let parameter_type = string(param, "type", &scope)?;
                validate_parameter_type(&parameter_type, param.get("default"), &scope)?;
            }

            if file_name == "cpu.toml" {
                let engine = section(value, "engine", file_name)?;
                reject_unknown(engine, &["arch", "model"], "engine")?;
                string(engine, "arch", "engine")?;
                if engine.contains_key("model") {
                    string(engine, "model", "engine")?;
                }
            }
            Ok(())
        }
        "part.toml" => {
            // Part descriptions deliberately keep device-specific bus and
            // register sections at the file root; validate each known section
            // while rejecting misspelled top-level sections.
            reject_unknown(root, &["part", "i2c", "spi", "register"], file_name)?;
            let part = section(value, "part", file_name)?;
            reject_unknown(
                part,
                &["id", "origin", "name", "summary", "bindings", "provenance"],
                "part",
            )?;
            string(part, "id", "part")?;
            let origin = string(part, "origin", "part")?;
            if !known_origins.contains(&origin) {
                return Err(format!("part.origin: unknown canonical origin {origin:?}"));
            }
            string(part, "name", "part")?;
            string(part, "summary", "part")?;
            let bindings = string_array(part, "bindings", "part", true)?;
            let mut unique_bindings = BTreeSet::new();
            for binding in &bindings {
                if !unique_bindings.insert(binding) {
                    return Err(format!("part.bindings: duplicate binding {binding:?}"));
                }
                if !matches!(binding.as_str(), "i2c" | "spi") {
                    return Err(format!("part.bindings: unsupported binding {binding:?}"));
                }
            }
            let provenance = part
                .get("provenance")
                .and_then(toml::Value::as_table)
                .ok_or_else(|| "part.provenance: expected a table".to_owned())?;
            reject_unknown(provenance, &["doc", "confidence"], "part.provenance")?;
            string(provenance, "doc", "part.provenance")?;
            string(provenance, "confidence", "part.provenance")?;

            if let Some(i2c) = root.get("i2c") {
                if !bindings.iter().any(|binding| binding.as_str() == "i2c") {
                    return Err(
                        "i2c: section exists but part does not declare an i2c binding".into(),
                    );
                }
                let i2c = i2c
                    .as_table()
                    .ok_or_else(|| "i2c: expected a table".to_owned())?;
                reject_unknown(i2c, &["addresses", "channels", "pointer-width"], "i2c")?;
                if let Some(addresses) = i2c.get("addresses") {
                    let addresses = integer_array(addresses, "i2c.addresses", true)?;
                    let mut unique = BTreeSet::new();
                    for address in addresses {
                        if !(0..=0x3ff).contains(&address) {
                            return Err(format!(
                                "i2c.addresses: invalid 10-bit address {address:#x}"
                            ));
                        }
                        if !unique.insert(address) {
                            return Err(format!("i2c.addresses: duplicate address {address:#x}"));
                        }
                    }
                }
                for key in ["channels", "pointer-width"] {
                    if let Some(value) = i2c.get(key) {
                        positive_integer(value, &format!("i2c.{key}"))?;
                    }
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "i2c") && !root.contains_key("i2c")
            {
                return Err("part.bindings: i2c binding has no [i2c] section".into());
            }

            if let Some(spi) = root.get("spi") {
                if !bindings.iter().any(|binding| binding.as_str() == "spi") {
                    return Err(
                        "spi: section exists but part does not declare an spi binding".into(),
                    );
                }
                let spi = spi
                    .as_table()
                    .ok_or_else(|| "spi: expected a table".to_owned())?;
                reject_unknown(spi, &["mode", "dummy-bytes-on-read"], "spi")?;
                if let Some(mode) = spi.get("mode") {
                    for mode in integer_array(mode, "spi.mode", true)? {
                        if !(0..=3).contains(&mode) {
                            return Err(format!("spi.mode: invalid SPI mode {mode}"));
                        }
                    }
                }
                if let Some(value) = spi.get("dummy-bytes-on-read") {
                    non_negative_integer(value, "spi.dummy-bytes-on-read")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "spi") && !root.contains_key("spi")
            {
                return Err("part.bindings: spi binding has no [spi] section".into());
            }

            let registers: Vec<&toml::Value> = match root.get("register") {
                None => Vec::new(),
                Some(value) => value
                    .as_array()
                    .ok_or_else(|| "register: expected an array of tables".to_owned())?
                    .iter()
                    .collect(),
            };
            let mut register_names = BTreeSet::new();
            for (index, value) in registers.iter().enumerate() {
                let scope = format!("register[{index}]");
                let register = value
                    .as_table()
                    .ok_or_else(|| format!("{scope}: expected a table"))?;
                reject_unknown(
                    register,
                    &["name", "offset", "size", "access", "reset", "note"],
                    &scope,
                )?;
                let name = string(register, "name", &scope)?;
                if name.trim().is_empty() || !register_names.insert(name.clone()) {
                    return Err(format!(
                        "{scope}.name: empty or duplicate register name {name:?}"
                    ));
                }
                non_negative_integer(
                    register
                        .get("offset")
                        .ok_or_else(|| format!("{scope}.offset: missing field"))?,
                    &format!("{scope}.offset"),
                )?;
                if let Some(size) = register.get("size") {
                    positive_integer(size, &format!("{scope}.size"))?;
                }
                let access = string(register, "access", &scope)?;
                if !matches!(access.as_str(), "ro" | "rw" | "wo") {
                    return Err(format!(
                        "{scope}.access: unsupported access mode {access:?}"
                    ));
                }
                if let Some(reset) = register.get("reset") {
                    non_negative_integer(reset, &format!("{scope}.reset"))?;
                }
                if register.contains_key("note") {
                    string(register, "note", &scope)?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn valid_manifest_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn validate_parameter_type(
    type_name: &str,
    default: Option<&toml::Value>,
    scope: &str,
) -> Result<(), String> {
    let supported = [
        "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "bool", "string",
        "policy",
    ];
    if !supported.contains(&type_name) {
        return Err(format!(
            "{scope}.type: unsupported parameter type {type_name:?}"
        ));
    }
    let Some(value) = default else {
        return Ok(());
    };
    let is_integer = value.as_integer();
    match type_name {
        "u8" | "u16" | "u32" => {
            let bits: u32 = match type_name {
                "u8" => 8,
                "u16" => 16,
                _ => 32,
            };
            let max = (1i64 << bits) - 1;
            match is_integer {
                Some(value) if (0..=max).contains(&value) => Ok(()),
                Some(_) => Err(format!("{scope}.default: value is outside {type_name}")),
                None => Err(format!(
                    "{scope}.default: expected an integer for {type_name}"
                )),
            }
        }
        "u64" | "usize" => match is_integer {
            Some(value) if value >= 0 => Ok(()),
            Some(_) => Err(format!(
                "{scope}.default: expected a non-negative {type_name}"
            )),
            None => Err(format!(
                "{scope}.default: expected an integer for {type_name}"
            )),
        },
        "i8" | "i16" | "i32" => {
            let bits: u32 = match type_name {
                "i8" => 8,
                "i16" => 16,
                _ => 32,
            };
            let min = -(1i64 << (bits - 1));
            let max = (1i64 << (bits - 1)) - 1;
            match is_integer {
                Some(value) if (min..=max).contains(&value) => Ok(()),
                Some(_) => Err(format!("{scope}.default: value is outside {type_name}")),
                None => Err(format!(
                    "{scope}.default: expected an integer for {type_name}"
                )),
            }
        }
        "i64" | "isize" => {
            if is_integer.is_some() {
                Ok(())
            } else {
                Err(format!(
                    "{scope}.default: expected an integer for {type_name}"
                ))
            }
        }
        "bool" => {
            if value.as_bool().is_some() {
                Ok(())
            } else {
                Err(format!("{scope}.default: expected a boolean"))
            }
        }
        "string" | "policy" => {
            if value.as_str().is_some() {
                Ok(())
            } else {
                Err(format!(
                    "{scope}.default: expected a string for {type_name}"
                ))
            }
        }
        _ => Err(format!(
            "{scope}.type: unsupported parameter type {type_name:?}"
        )),
    }
}

fn integer_array(
    value: &toml::Value,
    scope: &str,
    require_non_empty: bool,
) -> Result<Vec<i64>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{scope}: expected an array of integers"))?;
    let mut result = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        result.push(
            value
                .as_integer()
                .ok_or_else(|| format!("{scope}[{index}]: expected an integer"))?,
        );
    }
    if require_non_empty && result.is_empty() {
        return Err(format!("{scope}: array must not be empty"));
    }
    Ok(result)
}

fn non_negative_integer(value: &toml::Value, scope: &str) -> Result<(), String> {
    match value.as_integer() {
        Some(value) if value >= 0 => Ok(()),
        Some(_) => Err(format!("{scope}: expected a non-negative integer")),
        None => Err(format!("{scope}: expected an integer")),
    }
}

fn positive_integer(value: &toml::Value, scope: &str) -> Result<(), String> {
    match value.as_integer() {
        Some(value) if value > 0 => Ok(()),
        Some(_) => Err(format!("{scope}: expected a positive integer")),
        None => Err(format!("{scope}: expected an integer")),
    }
}

fn string_array(
    table: &toml::Table,
    key: &str,
    scope: &str,
    require_non_empty: bool,
) -> Result<Vec<String>, String> {
    let values = table
        .get(key)
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("{scope}.{key}: expected an array of strings"))?;
    let mut result = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let value = value
            .as_str()
            .ok_or_else(|| format!("{scope}.{key}[{index}]: expected a string"))?;
        if value.trim().is_empty() {
            return Err(format!("{scope}.{key}[{index}]: string must not be empty"));
        }
        result.push(value.to_owned());
    }
    if require_non_empty && result.is_empty() {
        return Err(format!("{scope}.{key}: array must not be empty"));
    }
    Ok(result)
}

pub(super) fn parse_soc(value: &toml::Value) -> Result<SocManifest, String> {
    let root = value
        .as_table()
        .ok_or_else(|| "soc.toml: expected a top-level table".to_owned())?;
    reject_unknown(root, &["soc", "cpu", "block", "view"], "soc.toml")?;
    let soc_table = section(value, "soc", "soc.toml")?;
    reject_unknown(soc_table, &["id", "summary"], "soc.toml [soc]")?;
    let cpu = optional_array(value, "cpu", "soc.toml")?
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let scope = format!("soc.toml cpu[{index}]");
            let item = item
                .as_table()
                .ok_or_else(|| format!("{scope}: expected a table"))?;
            reject_unknown(item, &["name", "use", "view"], &scope)?;
            Ok(CpuInstance {
                name: string(item, "name", &scope)?,
                use_id: string(item, "use", &scope)?,
                view: string(item, "view", &scope)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let block = optional_array(value, "block", "soc.toml")?
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let scope = format!("soc.toml block[{index}]");
            let item = item
                .as_table()
                .ok_or_else(|| format!("{scope}: expected a table"))?;
            reject_unknown(item, &["name", "use", "at", "irq", "clk"], &scope)?;
            let at = match item.get("at") {
                Some(value) => {
                    let at = value
                        .as_table()
                        .ok_or_else(|| format!("{scope}.at: expected a table"))?;
                    reject_unknown(at, &["space", "base"], &format!("{scope}.at"))?;
                    Some(Placement {
                        space: string(at, "space", &format!("{scope}.at"))?,
                        base: unsigned(at, "base", &format!("{scope}.at"))?,
                    })
                }
                None => None,
            };
            let irq = match item.get("irq") {
                Some(value) => {
                    // Keep the original inline-table form for a single route,
                    // while allowing an array of tables for independent outputs
                    // such as SP804 timer1/timer2 or PL190 IRQ/FIQ.
                    let (routes, single) = if value.as_table().is_some() {
                        (vec![value], true)
                    } else {
                        (
                            value
                                .as_array()
                                .ok_or_else(|| format!("{scope}.irq: expected a table or an array of tables"))?
                                .iter()
                                .collect::<Vec<_>>(),
                            false,
                        )
                    };
                    routes
                        .into_iter()
                        .enumerate()
                        .map(|(route_index, value)| {
                            let route_scope = if single {
                                format!("{scope}.irq")
                            } else {
                                format!("{scope}.irq[{route_index}]")
                            };
                            let route = value
                                .as_table()
                                .ok_or_else(|| format!("{route_scope}: expected a table"))?;
                            reject_unknown(route, &["pin", "to"], &route_scope)?;
                            let to = match route.get("to") {
                                None => Vec::new(),
                                Some(value) => value
                                    .as_array()
                                    .ok_or_else(|| format!("{route_scope}.to: expected an array"))?
                                    .iter()
                                    .enumerate()
                                    .map(|(target_index, value)| {
                                        value.as_str().map(str::to_owned).ok_or_else(|| {
                                            format!("{route_scope}.to[{target_index}]: expected a string")
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?,
                            };
                            Ok(IrqRoute {
                                pin: string(route, "pin", &route_scope)?,
                                to,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?
                }
                None => Vec::new(),
            };
            let clk = match item.get("clk") {
                None => BTreeMap::new(),
                Some(value) => value
                    .as_table()
                    .ok_or_else(|| format!("{scope}.clk: expected a table"))?
                    .iter()
                    .map(|(key, value)| (key.clone(), config_value(value)))
                    .collect(),
            };
            Ok(BlockInstance {
                name: string(item, "name", &scope)?,
                use_id: string(item, "use", &scope)?,
                at,
                irq,
                clk,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let view = optional_array(value, "view", "soc.toml")?
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let scope = format!("soc.toml view[{index}]");
            let item = item
                .as_table()
                .ok_or_else(|| format!("{scope}: expected a table"))?;
            reject_unknown(item, &["name", "region"], &scope)?;
            let region = optional_table_array(item, "region", &scope)?
                .into_iter()
                .enumerate()
                .map(|(region_index, region)| {
                    let region_scope = format!("{scope}.region[{region_index}]");
                    let region = region
                        .as_table()
                        .ok_or_else(|| format!("{region_scope}: expected a table"))?;
                    reject_unknown(
                        region,
                        &["name", "target", "base", "size", "kind", "priority"],
                        &region_scope,
                    )?;
                    let priority = optional_integer(region, "priority", &region_scope)?
                        .map(|priority| {
                            i32::try_from(priority).map_err(|_| {
                                format!("{region_scope}.priority: outside the i32 range")
                            })
                        })
                        .transpose()?;
                    Ok(RegionDescriptor {
                        name: string(region, "name", &region_scope)?,
                        target: string(region, "target", &region_scope)?,
                        base: unsigned(region, "base", &region_scope)?,
                        size: unsigned(region, "size", &region_scope)?,
                        kind: parse_region_kind(region, &region_scope)?,
                        priority,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(AddressSpaceView {
                name: string(item, "name", &scope)?,
                region,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let manifest = SocManifest {
        soc: SocIdentity {
            id: string(soc_table, "id", "soc.toml [soc]")?,
            summary: string(soc_table, "summary", "soc.toml [soc]")?,
        },
        cpu,
        block,
        view,
    };
    manifest.validate().map_err(|error| error.to_string())?;
    Ok(manifest)
}

pub(super) fn parse_board(value: &toml::Value) -> Result<BoardManifest, String> {
    let root = value
        .as_table()
        .ok_or_else(|| "board.toml: expected a top-level table".to_owned())?;
    reject_unknown(root, &["board", "conn", "part"], "board.toml")?;
    let board_table = section(value, "board", "board.toml")?;
    reject_unknown(board_table, &["id", "soc", "summary"], "board.toml [board]")?;
    // `soc` is optional: a multi-chip machine (the IBM PC/AT) mounts discrete
    // chips instead of one SoC and wires them in its own glue crate (ADR 0023).
    let soc = match board_table.get("soc") {
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| "board.toml [board] soc: expected a string".to_owned())?
                .to_owned(),
        ),
        None => None,
    };
    let conn = optional_array(value, "conn", "board.toml")?
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let scope = format!("board.toml conn[{index}]");
            let item = item
                .as_table()
                .ok_or_else(|| format!("{scope}: expected a table"))?;
            reject_unknown(item, &["name", "port", "to", "kind"], &scope)?;
            Ok(BoardConnection {
                name: string(item, "name", &scope)?,
                port: string(item, "port", &scope)?,
                to: string(item, "to", &scope)?,
                kind: string(item, "kind", &scope)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let part = optional_array(value, "part", "board.toml")?
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let scope = format!("board.toml part[{index}]");
            let item = item
                .as_table()
                .ok_or_else(|| format!("{scope}: expected a table"))?;
            reject_unknown(item, &["name", "use", "bind", "conn"], &scope)?;
            let conn = match item.get("conn") {
                Some(value) => value
                    .as_table()
                    .ok_or_else(|| format!("{scope}.conn: expected a table"))?
                    .iter()
                    .map(|(key, value)| (key.clone(), config_value(value)))
                    .collect(),
                None => BTreeMap::new(),
            };
            Ok(PartInstance {
                name: string(item, "name", &scope)?,
                use_id: string(item, "use", &scope)?,
                bind: string(item, "bind", &scope)?,
                conn,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let manifest = BoardManifest {
        board: BoardIdentity {
            id: string(board_table, "id", "board.toml [board]")?,
            soc,
            summary: string(board_table, "summary", "board.toml [board]")?,
        },
        conn,
        part,
    };
    manifest.validate().map_err(|error| error.to_string())?;
    Ok(manifest)
}

fn parse_region_kind(table: &toml::Table, scope: &str) -> Result<RegionKind, String> {
    match string(table, "kind", scope)?.as_str() {
        "ram" => Ok(RegionKind::Ram),
        "rom" => Ok(RegionKind::Rom),
        "mmio" => Ok(RegionKind::Mmio),
        other => Err(format!(
            "{scope}.kind: unknown memory kind {other:?}; expected ram, rom, or mmio"
        )),
    }
}

fn reject_unknown(table: &toml::Table, allowed: &[&str], scope: &str) -> Result<(), String> {
    if let Some(key) = table.keys().find(|key| !allowed.contains(&key.as_str())) {
        Err(format!("{scope}: unknown field {key:?}"))
    } else {
        Ok(())
    }
}

fn section<'a>(
    value: &'a toml::Value,
    section: &str,
    scope: &str,
) -> Result<&'a toml::Table, String> {
    value
        .get(section)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{scope}: missing [{section}] table"))
}

fn optional_table_array<'a>(
    table: &'a toml::Table,
    key: &str,
    scope: &str,
) -> Result<Vec<&'a toml::Value>, String> {
    match table.get(key) {
        None => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .map(|values| values.iter().collect())
            .ok_or_else(|| format!("{scope}.{key}: expected an array")),
    }
}

fn string(table: &toml::Table, key: &str, scope: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{scope}.{key}: expected a string"))
}

fn unsigned(table: &toml::Table, key: &str, scope: &str) -> Result<u64, String> {
    let value = table
        .get(key)
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| format!("{scope}.{key}: expected an integer"))?;
    u64::try_from(value).map_err(|_| format!("{scope}.{key}: expected a non-negative integer"))
}

fn optional_integer(table: &toml::Table, key: &str, scope: &str) -> Result<Option<i64>, String> {
    table
        .get(key)
        .map(|value| {
            value
                .as_integer()
                .ok_or_else(|| format!("{scope}.{key}: expected an integer"))
        })
        .transpose()
}

fn optional_array<'a>(
    value: &'a toml::Value,
    key: &str,
    scope: &str,
) -> Result<Vec<&'a toml::Value>, String> {
    match value.get(key) {
        None => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .map(|values| values.iter().collect())
            .ok_or_else(|| format!("{scope}.{key}: expected an array")),
    }
}

fn config_value(value: &toml::Value) -> ConfigValue {
    match value {
        toml::Value::String(value) => ConfigValue::String(value.clone()),
        toml::Value::Integer(value) => ConfigValue::Integer(*value),
        toml::Value::Float(value) => ConfigValue::Float(*value),
        toml::Value::Boolean(value) => ConfigValue::Boolean(*value),
        toml::Value::Datetime(value) => ConfigValue::Datetime(value.to_string()),
        toml::Value::Array(values) => ConfigValue::Array(values.iter().map(config_value).collect()),
        toml::Value::Table(values) => ConfigValue::Table(
            values
                .iter()
                .map(|(key, value)| (key.clone(), config_value(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_metadata_rejects_misspelled_port_properties() {
        let value: toml::Value = toml::from_str(
            r#"
[identity]
id='ip:test/device'
summary='test device'
spec=['test spec']
variants=[]
[tags]
function=['device']
fidelity='functional'
verified='experimental'
[ports.mmio]
role='target'
siz=4096
[params]
"#,
        )
        .unwrap();
        let error =
            validate_leaf_catalog_manifest("ip.toml", &value, &BTreeSet::new()).unwrap_err();
        assert!(error.contains("unknown field"), "{error}");
    }

    #[test]
    fn part_metadata_requires_a_declared_origin() {
        let value: toml::Value = toml::from_str(
            r#"
[part]
id='part:test/device'
origin='misspelled-origin'
name='Test device'
summary='test part'
bindings=['i2c']
[part.provenance]
doc='test data sheet'
confidence='high'
"#,
        )
        .unwrap();
        let origins = ["arm".to_owned()].into_iter().collect();
        let error = validate_leaf_catalog_manifest("part.toml", &value, &origins).unwrap_err();
        assert!(error.contains("unknown canonical origin"), "{error}");
    }

    #[test]
    fn leaf_metadata_checks_parameter_default_type_and_range() {
        let value: toml::Value = toml::from_str(
            r#"
[identity]
id='ip:test/device'
summary='test device'
spec=['test spec']
variants=[]
[tags]
function=['device']
fidelity='functional'
verified='experimental'
[ports.mmio]
role='target'
size=4
[params.depth]
type='u8'
default=256
"#,
        )
        .unwrap();
        let error =
            validate_leaf_catalog_manifest("ip.toml", &value, &BTreeSet::new()).unwrap_err();
        assert!(error.contains("outside u8"), "{error}");
    }

    #[test]
    fn leaf_metadata_checks_port_role_and_declared_count() {
        let value: toml::Value = toml::from_str(
            r#"
[identity]
id='ip:test/device'
summary='test device'
spec=['test spec']
variants=[]
[tags]
function=['device']
fidelity='functional'
verified='experimental'
[ports.irq]
role='source'
count=2
pins=['irq0']
[params]
"#,
        )
        .unwrap();
        let error =
            validate_leaf_catalog_manifest("ip.toml", &value, &BTreeSet::new()).unwrap_err();
        assert!(error.contains("contains 1 items but count is 2"), "{error}");
    }

    #[test]
    fn parser_rejects_misspelled_schema_keys() {
        let typo: toml::Value = toml::from_str(
            r#"
[soc]
id='soc:test'
summary='test'
[[cpu]]
name='core0'
use='cpu:test'
view='sysbus'
[[blcok]]
name='uart0'
use='ip:arm/pl011'
[[view]]
name='sysbus'
[[view.region]]
name='ram'
target='ram'
base=0
size=4096
kind='ram'
"#,
        )
        .unwrap();
        assert!(parse_soc(&typo).unwrap_err().contains("unknown field"));
    }

    #[test]
    fn parser_rejects_unknown_memory_kind() {
        let value: toml::Value = toml::from_str(
            r#"
[soc]
id='soc:test'
summary='test'
[[cpu]]
name='core0'
use='cpu:test'
view='sysbus'
[[view]]
name='sysbus'
[[view.region]]
name='ram'
target='ram'
base=0
size=4096
kind='mmo'
"#,
        )
        .unwrap();
        // Keep the fixture's top-level shape valid; only the enum value is bad.
        let error = parse_soc(&value).unwrap_err();
        assert!(error.contains("unknown memory kind"));
    }

    #[test]
    fn graph_builder_rejects_unknown_bus_nodes() {
        // A self-contained pair: the board wires a part onto a bus the SoC
        // never declared, and the graph builder must say so by node id.
        let soc: toml::Value = toml::from_str(
            r#"
[soc]
id = 'soc:test/fixture'
summary = 'parser fixture'
[[cpu]]
name = 'core0'
use = 'cpu:test/core'
view = 'sysbus'
[[view]]
name = 'sysbus'
[[view.region]]
name = 'ram'
target = 'ram'
base = 0
size = 4096
kind = 'ram'
"#,
        )
        .unwrap();
        let soc = parse_soc(&soc).unwrap();
        let board: toml::Value = toml::from_str(
            r#"
[board]
id = 'board:test/fixture'
summary = 'parser fixture'
[[part]]
name = 'env0'
use = 'part:test/device'
bind = 'stream'
conn = { bus = 'soc.not-a-device' }
"#,
        )
        .unwrap();
        let board = parse_board(&board).unwrap();
        let error = mosaic_describe::MachineGraph::build(&soc, Some(&board)).unwrap_err();
        assert!(error.message.contains("unknown bus node"), "{error:?}");
    }

    #[test]
    fn graph_regions_must_fit_the_catalog_mmio_window() {
        let manifest: toml::Value = toml::from_str(
            r#"
[ports.mmio]
role = 'target'
size = 4096
"#,
        )
        .unwrap();
        let catalog = [("ip:test/device".to_owned(), manifest)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let graph = MachineGraph {
            soc_id: "soc:test".into(),
            board_id: None,
            nodes: vec![
                GraphNode {
                    id: "view.sysbus".into(),
                    kind: GraphNodeKind::AddressSpaceView,
                    catalog_id: None,
                    parameters: BTreeMap::new(),
                },
                GraphNode {
                    id: "soc.dev0".into(),
                    kind: GraphNodeKind::Peripheral,
                    catalog_id: Some("ip:test/device".into()),
                    parameters: BTreeMap::new(),
                },
            ],
            regions: vec![mosaic_describe::GraphRegion {
                name: "dev0-mmio".into(),
                view: "view.sysbus".into(),
                target_node: "soc.dev0".into(),
                base: 0x1000,
                size: 8192,
                kind: mosaic_describe::RegionKind::Mmio,
                priority: None,
            }],
            edges: Vec::new(),
        };
        let errors = validate_graph_ports(&graph, &catalog);
        assert!(
            errors.iter().any(|error| error.contains("exceeds")),
            "{errors:?}"
        );
    }

    #[test]
    fn parser_uses_typed_schema_for_checked_in_manifests() {
        // The real STM32F103 machine declaration is the checked-in fixture:
        // what the machine crate assembles is what this parser must accept.
        let soc: toml::Value = toml::from_str(include_str!(
            "../../manifests/machine/arm/stm32f103/soc.toml"
        ))
        .unwrap();
        let soc = parse_soc(&soc).unwrap();
        assert_eq!(soc.block.len(), 4);
        assert_eq!(soc.cpu.len(), 1);
        assert_eq!(soc.cpu[0].use_id, "cpu:arm/cortex-m3");

        let graph = mosaic_describe::MachineGraph::build(&soc, None).unwrap();
        assert_eq!(graph.board_id, None);
        assert_eq!(graph.regions.len(), 7);
        assert!(graph.nodes.iter().any(|node| node.id == "soc.usart1"));
        assert!(graph.nodes.iter().any(|node| node.id == "view.sysbus"));
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "address-space"
                && edge.from_node == "soc.core0"
                && edge.to_node == "view.sysbus"
        }));
        assert!(graph.edges.iter().any(|edge| {
            edge.kind == "irq"
                && edge.from_node == "soc.usart1"
                && edge.from_port == "combined"
                && edge.to_node == "soc.scs"
                && edge.to_port == "inputs:37"
        }));
    }
}
