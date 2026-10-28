//! TOML adapter for the format-independent `mosaic-describe` schema.
//!
//! Keeping this parser in tooling preserves the framework's no-general-purpose
//! external-dependency policy. Runtime and model crates consume typed values,
//! not TOML parser types.

use std::collections::{BTreeMap, BTreeSet};

use mosaic_describe::{
    AddressSpaceView, BlockInstance, BoardConnection, BoardIdentity, BoardManifest, ConfigValue,
    CpuInstance, GraphNode, GraphNodeKind, IrqRoute, MachineGraph, PartFace, PartInstance,
    Placement, RegionDescriptor, RegionKind, SocIdentity, SocManifest,
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
            reject_unknown(
                root,
                &[
                    "part",
                    "ports",
                    "i2c",
                    "spi",
                    "onewire",
                    "smbus",
                    "pmbus",
                    "microwire",
                    "uart",
                    "can",
                    "ps2",
                    "spdif",
                    "espi",
                    "midi",
                    "gameport",
                    "hdmi",
                    "displayport",
                    "mipi_dsi",
                    "mipi_csi",
                    "soundwire",
                    "slimbus",
                    "firewire",
                    "vga",
                    "dvi",
                    "lvds",
                    "sdio",
                    "pdm",
                    "pwm",
                    "nand",
                    "isa",
                    "pcmcia",
                    "ufs",
                    "hyperbus",
                    "gpib",
                    "register",
                ],
                file_name,
            )?;
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
                if !matches!(
                    binding.as_str(),
                    "i2c"
                        | "spi"
                        | "onewire"
                        | "smbus"
                        | "pmbus"
                        | "microwire"
                        | "uart"
                        | "can"
                        | "ps2"
                        | "spdif"
                        | "espi"
                        | "midi"
                        | "gameport"
                        | "hdmi"
                        | "displayport"
                        | "mipi_dsi"
                        | "mipi_csi"
                        | "soundwire"
                        | "slimbus"
                        | "firewire"
                        | "vga"
                        | "dvi"
                        | "lvds"
                        | "sdio"
                        | "pdm"
                        | "pwm"
                        | "nand"
                        | "isa"
                        | "pcmcia"
                        | "ufs"
                        | "hyperbus"
                        | "gpib"
                ) {
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

            // Port faces (ADR 0032): one table per face. A target face's
            // fabric must be declared in bindings, and every binding must
            // have a matching target face — the two lists cannot drift.
            if let Some(ports) = root.get("ports") {
                let ports = ports
                    .as_table()
                    .ok_or_else(|| "ports: expected a table of faces".to_owned())?;
                let mut target_fabrics = BTreeSet::new();
                for (name, face) in ports {
                    let face = face
                        .as_table()
                        .ok_or_else(|| format!("ports.{name}: expected a face table"))?;
                    reject_unknown(
                        face,
                        &["contract", "role", "count", "pins"],
                        &format!("ports.{name}"),
                    )?;
                    let role = string(face, "role", &format!("ports.{name}"))?;
                    // Target and initiator faces name their contract
                    // (`i2c/target`); source faces follow the ip-side irq
                    // convention (role + pins, no contract).
                    let contract = face
                        .get("contract")
                        .and_then(toml::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    match role.as_str() {
                        "target" => {
                            if contract.is_empty() {
                                return Err(format!(
                                    "ports.{name}: target face must name a contract"
                                ));
                            }
                            let fabric = contract
                                .split_once('/')
                                .map(|(fabric, _)| fabric.to_owned())
                                .unwrap_or(contract.clone());
                            if !bindings.iter().any(|binding| *binding == fabric) {
                                return Err(format!(
                                    "ports.{name}: target face on {fabric:?} but part does not \
                                     declare that binding"
                                ));
                            }
                            target_fabrics.insert(fabric);
                        }
                        "initiator" => {}
                        "provider" => {
                            if contract.is_empty() {
                                return Err(format!(
                                    "ports.{name}: provider face must name a contract"
                                ));
                            }
                        }
                        "source" => {
                            // Signal outputs (irq convention): pins, no contract.
                            if !face.contains_key("pins") {
                                return Err(format!(
                                    "ports.{name}: source face must declare its pins"
                                ));
                            }
                        }

                        other => {
                            return Err(format!(
                                "ports.{name}: unknown role {other:?}; expected target, \
                                 initiator, provider or source"
                            ));
                        }
                    }
                    if let Some(count) = face.get("count") {
                        let count = count
                            .as_integer()
                            .ok_or_else(|| format!("ports.{name}.count: expected an integer"))?;
                        if count < 0 {
                            return Err(format!("ports.{name}.count: negative count"));
                        }
                    }
                    if let Some(pins) = face.get("pins") {
                        if role != "source" {
                            return Err(format!(
                                "ports.{name}.pins: only source faces declare output pins"
                            ));
                        }
                        let pin_list = pins
                            .as_array()
                            .ok_or_else(|| format!("ports.{name}.pins: expected an array"))?;
                        if pin_list.is_empty() {
                            return Err(format!("ports.{name}.pins: empty pin list"));
                        }
                        for pin in pin_list {
                            if pin.as_str().is_none() {
                                return Err(format!("ports.{name}.pins: expected strings"));
                            }
                        }
                    }
                }
            } else if !bindings.is_empty() {
                // Parts without a [ports] table keep the bindings-only shape
                // (declared, not yet spawn-registered); the face grammar is
                // required only for parts a board can wire today.
            }

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

            // Fabric bindings that are point-to-point (no bus addressing and
            // hence no extra config section) — see ADR 0026 step 3 for the
            // full list. They declare the binding in `part.bindings` so the
            // graph can route them, but the manifest has no `[<name>]`
            // section to validate.
            let _point_to_point_bindings: &[&str] = &[
                "hdmi",
                "displayport",
                "mipi_dsi",
                "mipi_csi",
                "soundwire",
                "slimbus",
                "firewire",
            ];

            // 1-Wire binding: requires a family code and optional serial.
            if let Some(onewire) = root.get("onewire") {
                if !bindings.iter().any(|binding| binding.as_str() == "onewire") {
                    return Err(
                        "onewire: section exists but part does not declare an onewire binding"
                            .into(),
                    );
                }
                let onewire = onewire
                    .as_table()
                    .ok_or_else(|| "onewire: expected a table".to_owned())?;
                reject_unknown(onewire, &["family", "serial"], "onewire")?;
                if let Some(family) = onewire.get("family") {
                    non_negative_integer(family, "onewire.family")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "onewire")
                && !root.contains_key("onewire")
            {
                return Err("part.bindings: onewire binding has no [onewire] section".into());
            }

            // SMBus binding: address + optional pec.
            if let Some(smbus) = root.get("smbus") {
                if !bindings.iter().any(|binding| binding.as_str() == "smbus") {
                    return Err(
                        "smbus: section exists but part does not declare an smbus binding".into(),
                    );
                }
                let smbus = smbus
                    .as_table()
                    .ok_or_else(|| "smbus: expected a table".to_owned())?;
                reject_unknown(smbus, &["address", "pec"], "smbus")?;
                if let Some(addr) = smbus.get("address") {
                    non_negative_integer(addr, "smbus.address")?;
                }
                if let Some(pec) = smbus.get("pec") {
                    if pec.as_bool().is_none() {
                        return Err("smbus.pec: expected a boolean".to_owned());
                    }
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "smbus")
                && !root.contains_key("smbus")
            {
                return Err("part.bindings: smbus binding has no [smbus] section".into());
            }

            // PMBus binding: address.
            if let Some(pmbus) = root.get("pmbus") {
                if !bindings.iter().any(|binding| binding.as_str() == "pmbus") {
                    return Err(
                        "pmbus: section exists but part does not declare a pmbus binding".into(),
                    );
                }
                let pmbus = pmbus
                    .as_table()
                    .ok_or_else(|| "pmbus: expected a table".to_owned())?;
                reject_unknown(pmbus, &["address"], "pmbus")?;
                if let Some(addr) = pmbus.get("address") {
                    non_negative_integer(addr, "pmbus.address")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "pmbus")
                && !root.contains_key("pmbus")
            {
                return Err("part.bindings: pmbus binding has no [pmbus] section".into());
            }

            // Microwire binding: instruction bits + address bits.
            if let Some(microwire) = root.get("microwire") {
                if !bindings
                    .iter()
                    .any(|binding| binding.as_str() == "microwire")
                {
                    return Err(
                        "microwire: section exists but part does not declare a microwire binding"
                            .into(),
                    );
                }
                let microwire = microwire
                    .as_table()
                    .ok_or_else(|| "microwire: expected a table".to_owned())?;
                reject_unknown(
                    microwire,
                    &["instruction-bits", "address-bits"],
                    "microwire",
                )?;
                for key in ["instruction-bits", "address-bits"] {
                    if let Some(value) = microwire.get(key) {
                        positive_integer(value, &format!("microwire.{key}"))?;
                    }
                }
            }
            if bindings
                .iter()
                .any(|binding| binding.as_str() == "microwire")
                && !root.contains_key("microwire")
            {
                return Err("part.bindings: microwire binding has no [microwire] section".into());
            }

            // UART binding: optional baud/parity.
            if let Some(uart) = root.get("uart") {
                if !bindings.iter().any(|binding| binding.as_str() == "uart") {
                    return Err(
                        "uart: section exists but part does not declare a uart binding".into(),
                    );
                }
                let uart = uart
                    .as_table()
                    .ok_or_else(|| "uart: expected a table".to_owned())?;
                reject_unknown(uart, &["baud", "parity", "data-bits", "stop-bits"], "uart")?;
                if let Some(baud) = uart.get("baud") {
                    positive_integer(baud, "uart.baud")?;
                }
                if let Some(parity) = uart.get("parity") {
                    let parity = parity.as_str().ok_or_else(|| {
                        "uart.parity: expected a string (none, even, odd, mark, space)".to_owned()
                    })?;
                    if !matches!(parity, "none" | "even" | "odd" | "mark" | "space") {
                        return Err(format!("uart.parity: invalid parity {parity:?}"));
                    }
                }
                for key in ["data-bits", "stop-bits"] {
                    if let Some(value) = uart.get(key) {
                        positive_integer(value, &format!("uart.{key}"))?;
                    }
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "uart")
                && !root.contains_key("uart")
            {
                return Err("part.bindings: uart binding has no [uart] section".into());
            }

            // CAN binding: optional base/extended ids.
            if let Some(can) = root.get("can") {
                if !bindings.iter().any(|binding| binding.as_str() == "can") {
                    return Err(
                        "can: section exists but part does not declare a can binding".into(),
                    );
                }
                let can = can
                    .as_table()
                    .ok_or_else(|| "can: expected a table".to_owned())?;
                reject_unknown(can, &["ids", "extended"], "can")?;
                if let Some(ids) = can.get("ids") {
                    integer_array(ids, "can.ids", true)?;
                }
                if let Some(ext) = can.get("extended") {
                    if ext.as_bool().is_none() {
                        return Err("can.extended: expected a boolean".to_owned());
                    }
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "can") && !root.contains_key("can")
            {
                return Err("part.bindings: can binding has no [can] section".into());
            }
            if let Some(ps2) = root.get("ps2") {
                if !bindings.iter().any(|binding| binding.as_str() == "ps2") {
                    return Err(
                        "ps2: section exists but part does not declare a ps2 binding".into(),
                    );
                }
                let ps2 = ps2
                    .as_table()
                    .ok_or_else(|| "ps2: expected a table".to_owned())?;
                reject_unknown(ps2, &["kind", "device-id"], "ps2")?;
                if let Some(kind) = ps2.get("kind") {
                    let kind_str = kind
                        .as_str()
                        .ok_or_else(|| "ps2.kind: expected a string (keyboard|mouse)".to_owned())?;
                    if !matches!(kind_str, "keyboard" | "mouse") {
                        return Err(format!("ps2.kind: unsupported {kind_str:?}"));
                    }
                }
                if let Some(id) = ps2.get("device-id") {
                    non_negative_integer(id, "ps2.device-id")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "ps2") && !root.contains_key("ps2")
            {
                return Err("part.bindings: ps2 binding has no [ps2] section".into());
            }
            if let Some(spdif) = root.get("spdif") {
                if !bindings.iter().any(|binding| binding.as_str() == "spdif") {
                    return Err(
                        "spdif: section exists but part does not declare an spdif binding".into(),
                    );
                }
                let spdif = spdif
                    .as_table()
                    .ok_or_else(|| "spdif: expected a table".to_owned())?;
                reject_unknown(spdif, &["mode", "rate"], "spdif")?;
                if let Some(mode) = spdif.get("mode") {
                    let m = mode.as_str().ok_or_else(|| {
                        "spdif.mode: expected a string (consumer|professional)".to_owned()
                    })?;
                    if !matches!(m, "consumer" | "professional") {
                        return Err(format!("spdif.mode: unsupported {m:?}"));
                    }
                }
                if let Some(rate) = spdif.get("rate") {
                    non_negative_integer(rate, "spdif.rate")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "spdif")
                && !root.contains_key("spdif")
            {
                return Err("part.bindings: spdif binding has no [spdif] section".into());
            }
            if let Some(espi) = root.get("espi") {
                if !bindings.iter().any(|binding| binding.as_str() == "espi") {
                    return Err(
                        "espi: section exists but part does not declare an espi binding".into(),
                    );
                }
                let espi = espi
                    .as_table()
                    .ok_or_else(|| "espi: expected a table".to_owned())?;
                reject_unknown(espi, &["cycles", "chip-select"], "espi")?;
                if let Some(cs) = espi.get("chip-select") {
                    non_negative_integer(cs, "espi.chip-select")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "espi")
                && !root.contains_key("espi")
            {
                return Err("part.bindings: espi binding has no [espi] section".into());
            }
            if let Some(midi) = root.get("midi") {
                if !bindings.iter().any(|binding| binding.as_str() == "midi") {
                    return Err(
                        "midi: section exists but part does not declare a midi binding".into(),
                    );
                }
                let midi = midi
                    .as_table()
                    .ok_or_else(|| "midi: expected a table".to_owned())?;
                reject_unknown(midi, &["direction", "channels"], "midi")?;
                if let Some(dir) = midi.get("direction") {
                    let d = dir.as_str().ok_or_else(|| {
                        "midi.direction: expected a string (in|out|both)".to_owned()
                    })?;
                    if !matches!(d, "in" | "out" | "both") {
                        return Err(format!("midi.direction: unsupported {d:?}"));
                    }
                }
                if let Some(ch) = midi.get("channels") {
                    non_negative_integer(ch, "midi.channels")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "midi")
                && !root.contains_key("midi")
            {
                return Err("part.bindings: midi binding has no [midi] section".into());
            }
            if let Some(gp) = root.get("gameport") {
                if !bindings
                    .iter()
                    .any(|binding| binding.as_str() == "gameport")
                {
                    return Err(
                        "gameport: section exists but part does not declare a gameport binding"
                            .into(),
                    );
                }
                let gp = gp
                    .as_table()
                    .ok_or_else(|| "gameport: expected a table".to_owned())?;
                reject_unknown(gp, &["axes", "buttons"], "gameport")?;
                if let Some(axes) = gp.get("axes") {
                    non_negative_integer(axes, "gameport.axes")?;
                }
                if let Some(btns) = gp.get("buttons") {
                    non_negative_integer(btns, "gameport.buttons")?;
                }
            }
            if bindings
                .iter()
                .any(|binding| binding.as_str() == "gameport")
                && !root.contains_key("gameport")
            {
                return Err("part.bindings: gameport binding has no [gameport] section".into());
            }
            if let Some(sec) = root.get("vga") {
                if !bindings.iter().any(|binding| binding.as_str() == "vga") {
                    return Err(
                        "vga: section exists but part does not declare a vga binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "vga: expected a table".to_owned())?;
                reject_unknown(sec, &["max-pixel-clock-khz"], "vga")?;
                if let Some(value) = sec.get("max-pixel-clock-khz") {
                    non_negative_integer(value, "vga.max-pixel-clock-khz")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "vga") && !root.contains_key("vga")
            {
                return Err("part.bindings: vga binding has no [vga] section".into());
            }
            if let Some(sec) = root.get("dvi") {
                if !bindings.iter().any(|binding| binding.as_str() == "dvi") {
                    return Err(
                        "dvi: section exists but part does not declare a dvi binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "dvi: expected a table".to_owned())?;
                reject_unknown(sec, &["lanes"], "dvi")?;
                if let Some(value) = sec.get("lanes") {
                    non_negative_integer(value, "dvi.lanes")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "dvi") && !root.contains_key("dvi")
            {
                return Err("part.bindings: dvi binding has no [dvi] section".into());
            }
            if let Some(sec) = root.get("lvds") {
                if !bindings.iter().any(|binding| binding.as_str() == "lvds") {
                    return Err(
                        "lvds: section exists but part does not declare a lvds binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "lvds: expected a table".to_owned())?;
                reject_unknown(sec, &["lanes"], "lvds")?;
                if let Some(value) = sec.get("lanes") {
                    non_negative_integer(value, "lvds.lanes")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "lvds")
                && !root.contains_key("lvds")
            {
                return Err("part.bindings: lvds binding has no [lvds] section".into());
            }
            if let Some(sec) = root.get("sdio") {
                if !bindings.iter().any(|binding| binding.as_str() == "sdio") {
                    return Err(
                        "sdio: section exists but part does not declare a sdio binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "sdio: expected a table".to_owned())?;
                reject_unknown(sec, &["functions"], "sdio")?;
                if let Some(value) = sec.get("functions") {
                    non_negative_integer(value, "sdio.functions")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "sdio")
                && !root.contains_key("sdio")
            {
                return Err("part.bindings: sdio binding has no [sdio] section".into());
            }
            if let Some(sec) = root.get("pdm") {
                if !bindings.iter().any(|binding| binding.as_str() == "pdm") {
                    return Err(
                        "pdm: section exists but part does not declare a pdm binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "pdm: expected a table".to_owned())?;
                reject_unknown(sec, &["channels"], "pdm")?;
                if let Some(value) = sec.get("channels") {
                    non_negative_integer(value, "pdm.channels")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "pdm") && !root.contains_key("pdm")
            {
                return Err("part.bindings: pdm binding has no [pdm] section".into());
            }
            if let Some(sec) = root.get("pwm") {
                if !bindings.iter().any(|binding| binding.as_str() == "pwm") {
                    return Err(
                        "pwm: section exists but part does not declare a pwm binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "pwm: expected a table".to_owned())?;
                reject_unknown(sec, &["channels"], "pwm")?;
                if let Some(value) = sec.get("channels") {
                    non_negative_integer(value, "pwm.channels")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "pwm") && !root.contains_key("pwm")
            {
                return Err("part.bindings: pwm binding has no [pwm] section".into());
            }
            if let Some(sec) = root.get("nand") {
                if !bindings.iter().any(|binding| binding.as_str() == "nand") {
                    return Err(
                        "nand: section exists but part does not declare a nand binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "nand: expected a table".to_owned())?;
                reject_unknown(sec, &["page-size", "oob-size", "blocks"], "nand")?;
                if let Some(value) = sec.get("page-size") {
                    non_negative_integer(value, "nand.page-size")?;
                }
                if let Some(value) = sec.get("oob-size") {
                    non_negative_integer(value, "nand.oob-size")?;
                }
                if let Some(value) = sec.get("blocks") {
                    non_negative_integer(value, "nand.blocks")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "nand")
                && !root.contains_key("nand")
            {
                return Err("part.bindings: nand binding has no [nand] section".into());
            }
            if let Some(sec) = root.get("isa") {
                if !bindings.iter().any(|binding| binding.as_str() == "isa") {
                    return Err(
                        "isa: section exists but part does not declare a isa binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "isa: expected a table".to_owned())?;
                reject_unknown(sec, &["dma-channels"], "isa")?;
                if let Some(value) = sec.get("dma-channels") {
                    non_negative_integer(value, "isa.dma-channels")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "isa") && !root.contains_key("isa")
            {
                return Err("part.bindings: isa binding has no [isa] section".into());
            }
            if let Some(sec) = root.get("ufs") {
                if !bindings.iter().any(|binding| binding.as_str() == "ufs") {
                    return Err(
                        "ufs: section exists but part does not declare a ufs binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "ufs: expected a table".to_owned())?;
                reject_unknown(sec, &["lu-count"], "ufs")?;
                if let Some(value) = sec.get("lu-count") {
                    non_negative_integer(value, "ufs.lu-count")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "ufs") && !root.contains_key("ufs")
            {
                return Err("part.bindings: ufs binding has no [ufs] section".into());
            }
            if let Some(sec) = root.get("hyperbus") {
                if !bindings
                    .iter()
                    .any(|binding| binding.as_str() == "hyperbus")
                {
                    return Err(
                        "hyperbus: section exists but part does not declare a hyperbus binding"
                            .into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "hyperbus: expected a table".to_owned())?;
                reject_unknown(sec, &["size-mb"], "hyperbus")?;
                if let Some(value) = sec.get("size-mb") {
                    non_negative_integer(value, "hyperbus.size-mb")?;
                }
            }
            if bindings
                .iter()
                .any(|binding| binding.as_str() == "hyperbus")
                && !root.contains_key("hyperbus")
            {
                return Err("part.bindings: hyperbus binding has no [hyperbus] section".into());
            }
            if let Some(sec) = root.get("gpib") {
                if !bindings.iter().any(|binding| binding.as_str() == "gpib") {
                    return Err(
                        "gpib: section exists but part does not declare a gpib binding".into(),
                    );
                }
                let sec = sec
                    .as_table()
                    .ok_or_else(|| "gpib: expected a table".to_owned())?;
                reject_unknown(sec, &["address"], "gpib")?;
                if let Some(value) = sec.get("address") {
                    non_negative_integer(value, "gpib.address")?;
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "gpib")
                && !root.contains_key("gpib")
            {
                return Err("part.bindings: gpib binding has no [gpib] section".into());
            }
            if let Some(pc) = root.get("pcmcia") {
                if !bindings.iter().any(|binding| binding.as_str() == "pcmcia") {
                    return Err(
                        "pcmcia: section exists but part does not declare a pcmcia binding".into(),
                    );
                }
                let pc = pc
                    .as_table()
                    .ok_or_else(|| "pcmcia: expected a table".to_owned())?;
                reject_unknown(pc, &["slot"], "pcmcia")?;
                if let Some(slot) = pc.get("slot") {
                    let s = slot.as_str().ok_or_else(|| {
                        "pcmcia.slot: expected a string (16bit|cardbus)".to_owned()
                    })?;
                    if !matches!(s, "16bit" | "cardbus") {
                        return Err(format!("pcmcia.slot: unsupported {s:?}"));
                    }
                }
            }
            if bindings.iter().any(|binding| binding.as_str() == "pcmcia")
                && !root.contains_key("pcmcia")
            {
                return Err("part.bindings: pcmcia binding has no [pcmcia] section".into());
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
            reject_unknown(item, &["name", "use", "face"], &scope)?;
            let faces = optional_array(&toml::Value::Table(item.clone()), "face", &scope)?
                .into_iter()
                .enumerate()
                .map(|(face_index, face)| {
                    let face_scope = format!("{scope}.face[{face_index}]");
                    let face = face
                        .as_table()
                        .ok_or_else(|| format!("{face_scope}: expected a table"))?;
                    // The face grammar owns five keys; every other key is a
                    // wiring parameter (address, pull, cs, …) for the spawn.
                    let owned = ["port", "kind", "to", "index", "pin"];
                    let mut parameters = BTreeMap::new();
                    for (key, value) in face {
                        if owned.contains(&key.as_str()) {
                            continue;
                        }
                        parameters.insert(key.clone(), config_value(value));
                    }
                    Ok(PartFace {
                        port: string(face, "port", &face_scope)?,
                        kind: string(face, "kind", &face_scope)?,
                        to: string(face, "to", &face_scope)?,
                        index: match face.get("index") {
                            None => None,
                            Some(value) => Some(
                                value
                                    .as_integer()
                                    .and_then(|index| u64::try_from(index).ok())
                                    .ok_or_else(|| {
                                        format!(
                                            "{face_scope}.index: expected a non-negative integer"
                                        )
                                    })?,
                            ),
                        },
                        pin: match face.get("pin") {
                            None => None,
                            Some(_) => Some(string(face, "pin", &face_scope)?),
                        },
                        parameters,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(PartInstance {
                name: string(item, "name", &scope)?,
                use_id: string(item, "use", &scope)?,
                faces,
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
soc = 'soc:test/fixture'
summary = 'parser fixture'
[[part]]
name = 'env0'
use = 'part:test/device'
[[part.face]]
port = 'i2c'
kind = 'i2c'
to = 'soc.not-a-device:i2c'
address = 0x48
"#,
        )
        .unwrap();
        let board = parse_board(&board).unwrap();
        let error = mosaic_describe::MachineGraph::build(&soc, Some(&board)).unwrap_err();
        assert!(
            error.message.contains("unknown initiator node"),
            "{error:?}"
        );
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
            "../../../manifests/machine/arm/stm32f103/soc.toml"
        ))
        .unwrap();
        let soc = parse_soc(&soc).unwrap();
        assert_eq!(soc.block.len(), 5);
        assert_eq!(soc.cpu.len(), 1);
        assert_eq!(soc.cpu[0].use_id, "cpu:arm/cortex-m3");

        let graph = mosaic_describe::MachineGraph::build(&soc, None).unwrap();
        assert_eq!(graph.board_id, None);
        assert_eq!(graph.regions.len(), 8);
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
