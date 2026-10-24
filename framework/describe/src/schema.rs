#![forbid(unsafe_code)]
//! Typed, serialization-format-independent machine descriptions.
//!
//! This crate intentionally has no external dependencies and does not depend
//! on the runtime, catalog model implementations, or host I/O. A tooling or
//! application adapter parses a file format into these types, then calls the
//! validation methods before asking a higher layer to construct a machine.

use std::collections::{BTreeMap, BTreeSet};

use crate::endpoint::parse_endpoint;

/// A parse or semantic validation failure with a stable, user-readable message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DescriptorError {
    pub path: &'static str,
    pub message: String,
}

impl DescriptorError {
    pub(crate) fn new(path: &'static str, message: impl Into<String>) -> Self {
        Self {
            path,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for DescriptorError {}

/// Open-ended scalar/collection value for model-specific connection and clock
/// parameters. It is independent of TOML/JSON or any parser crate.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<ConfigValue>),
    Table(BTreeMap<String, ConfigValue>),
    Datetime(String),
}

impl ConfigValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&BTreeMap<String, ConfigValue>> {
        match self {
            Self::Table(value) => Some(value),
            _ => None,
        }
    }
}

/// Root of a SoC machine description.
#[derive(Clone, Debug)]
pub struct SocManifest {
    pub soc: SocIdentity,
    pub cpu: Vec<CpuInstance>,
    pub block: Vec<BlockInstance>,
    pub view: Vec<AddressSpaceView>,
}

#[derive(Clone, Debug)]
pub struct SocIdentity {
    pub id: String,
    pub summary: String,
}

#[derive(Clone, Debug)]
pub struct CpuInstance {
    pub name: String,
    pub use_id: String,
    /// Every CPU must select an address-space view explicitly.
    pub view: String,
}

#[derive(Clone, Debug)]
pub struct BlockInstance {
    pub name: String,
    pub use_id: String,
    pub at: Option<Placement>,
    /// Each independently driven interrupt output has its own route.
    pub irq: Vec<IrqRoute>,
    /// Clock signal names are model-defined, so preserve the open-ended map.
    pub clk: BTreeMap<String, ConfigValue>,
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub space: String,
    pub base: u64,
}

/// Valid memory-region kinds are represented as an enum after parsing so the
/// validated schema cannot carry a typo such as `"mmo"` into graph building.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionKind {
    Ram,
    Rom,
    Mmio,
}

#[derive(Clone, Debug)]
pub struct IrqRoute {
    pub pin: String,
    pub to: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct AddressSpaceView {
    pub name: String,
    pub region: Vec<RegionDescriptor>,
}

#[derive(Clone, Debug)]
pub struct RegionDescriptor {
    pub name: String,
    /// Stable local node/backing name, independent of runtime numeric NodeIds.
    pub target: String,
    pub base: u64,
    pub size: u64,
    pub kind: RegionKind,
    /// Missing priority means priority zero. Explicitly higher priorities may
    /// overlay lower-priority mappings; equal-priority overlaps are ambiguous.
    pub priority: Option<i32>,
}

impl SocManifest {
    /// Validate structural invariants without resolving catalog IDs or loading
    /// model implementations. The upper bound is exclusive, so a mapping may
    /// end exactly at 2^64 but may not wrap beyond the address-space width.
    pub fn validate(&self) -> Result<(), DescriptorError> {
        validate_catalog_id("soc.id", &self.soc.id, &["soc"])?;
        non_empty("soc.summary", &self.soc.summary)?;

        let mut instance_names = BTreeSet::new();
        for cpu in &self.cpu {
            validate_local_name("cpu.name", &cpu.name)?;
            unique_name("cpu.name", &cpu.name, &mut instance_names)?;
            validate_catalog_id("cpu.use", &cpu.use_id, &["cpu"])?;
            validate_local_name("cpu.view", &cpu.view)?;
        }
        for block in &self.block {
            validate_local_name("block.name", &block.name)?;
            unique_name("block.name", &block.name, &mut instance_names)?;
            validate_catalog_id("block.use", &block.use_id, &["ip", "ip-lib"])?;
            if let Some(at) = &block.at {
                non_empty("block.at.space", &at.space)?;
            }
            for (input, source) in &block.clk {
                validate_local_name("block.clk.input", input)?;
                let source = source.as_str().ok_or_else(|| {
                    DescriptorError::new(
                        "block.clk",
                        format!(
                            "clock source for {:?}.{:?} must be a string",
                            block.name, input
                        ),
                    )
                })?;
                validate_local_name("block.clk.source", source)?;
            }
            let mut irq_pins = BTreeSet::new();
            for irq in &block.irq {
                validate_local_name("block.irq.pin", &irq.pin)?;
                unique_name("block.irq.pin", &irq.pin, &mut irq_pins)?;
                if irq.to.is_empty() {
                    return Err(DescriptorError::new(
                        "block.irq.to",
                        format!(
                            "interrupt output {:?}.{:?} needs at least one target",
                            block.name, irq.pin
                        ),
                    ));
                }
                for target in &irq.to {
                    non_empty("block.irq.to", target)?;
                }
            }
        }

        let mut view_names = BTreeSet::new();
        let mut view_by_name = BTreeMap::new();
        for (index, view) in self.view.iter().enumerate() {
            validate_local_name("view.name", &view.name)?;
            unique_name("view.name", &view.name, &mut view_names)?;
            view_by_name.insert(view.name.as_str(), index);

            let mut region_names = BTreeSet::new();
            for (region_index, region) in view.region.iter().enumerate() {
                validate_local_name("view.region.name", &region.name)?;
                unique_name("view.region.name", &region.name, &mut region_names)?;
                validate_local_name("view.region.target", &region.target)?;
                if region.kind == RegionKind::Mmio {
                    let target = self.block.iter().find(|block| block.name == region.target);
                    let Some(target) = target else {
                        return Err(DescriptorError::new(
                            "view.region.target",
                            format!(
                                "MMIO region {:?} targets unknown block {:?}",
                                region.name, region.target
                            ),
                        ));
                    };
                    let matches_placement = target.at.as_ref().is_some_and(|placement| {
                        placement.space == view.name && placement.base == region.base
                    });
                    if !matches_placement {
                        return Err(DescriptorError::new(
                            "view.region.target",
                            format!(
                                "MMIO region {:?} must match block {:?} placement in view {:?} \
                                 at {:#x}",
                                region.name, region.target, view.name, region.base
                            ),
                        ));
                    }
                }
                if region.size == 0 {
                    return Err(DescriptorError::new(
                        "view.region.size",
                        format!("region {:?} has zero size", region.name),
                    ));
                }
                if u128::from(region.base) + u128::from(region.size) > (1u128 << 64) {
                    return Err(DescriptorError::new(
                        "view.region",
                        format!(
                            "region {:?} overflows the 64-bit address space",
                            region.name
                        ),
                    ));
                }
                for previous in &view.region[..region_index] {
                    let same_priority =
                        region.priority.unwrap_or(0) == previous.priority.unwrap_or(0);
                    let overlaps = u128::from(region.base)
                        < u128::from(previous.base) + u128::from(previous.size)
                        && u128::from(previous.base)
                            < u128::from(region.base) + u128::from(region.size);
                    if same_priority && overlaps {
                        return Err(DescriptorError::new(
                            "view.region",
                            format!(
                                "regions {:?} and {:?} overlap at the same priority; \
                                 assign an explicit overlay priority",
                                previous.name, region.name
                            ),
                        ));
                    }
                }
            }
        }

        if self.cpu.is_empty() {
            return Err(DescriptorError::new(
                "cpu",
                "at least one CPU instance is required",
            ));
        }
        if self.view.is_empty() {
            return Err(DescriptorError::new(
                "view",
                "at least one address-space view is required",
            ));
        }
        for cpu in &self.cpu {
            if !view_by_name.contains_key(cpu.view.as_str()) {
                return Err(DescriptorError::new(
                    "cpu.view",
                    format!("CPU {:?} references unknown view {:?}", cpu.name, cpu.view),
                ));
            }
        }
        for block in &self.block {
            if let Some(at) = &block.at {
                if !view_by_name.contains_key(at.space.as_str()) {
                    return Err(DescriptorError::new(
                        "block.at.space",
                        format!(
                            "block {:?} references unknown view {:?}",
                            block.name, at.space
                        ),
                    ));
                }
            }
            if let Some(at) = &block.at {
                let mapped = self.view.iter().any(|view| {
                    view.name == at.space
                        && view.region.iter().any(|region| {
                            region.kind == RegionKind::Mmio
                                && region.target == block.name
                                && region.base == at.base
                        })
                });
                if !mapped {
                    return Err(DescriptorError::new(
                        "block.at",
                        format!(
                            "block {:?} at {:#x} has no MMIO region targeting it in view {:?}",
                            block.name, at.base, at.space
                        ),
                    ));
                }
            }
            for irq in &block.irq {
                for target in &irq.to {
                    // Resolve the whole endpoint so an IRQ can target a
                    // controller input (for example `vic0.inputs:12`) or a
                    // CPU line (`core0.irq`) without guessing the root name.
                    let endpoint = parse_endpoint(target, "soc")?;
                    let root = endpoint
                        .node_id
                        .strip_prefix("soc.")
                        .unwrap_or(&endpoint.node_id);
                    if !instance_names.contains(root) {
                        return Err(DescriptorError::new(
                            "block.irq.to",
                            format!(
                                "block {:?} references unknown endpoint {:?}",
                                block.name, target
                            ),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Root of a board description.
#[derive(Clone, Debug)]
pub struct BoardManifest {
    pub board: BoardIdentity,
    pub conn: Vec<BoardConnection>,
    pub part: Vec<PartInstance>,
}

#[derive(Clone, Debug)]
pub struct BoardIdentity {
    pub id: String,
    pub soc: String,
    pub summary: String,
}

#[derive(Clone, Debug)]
pub struct BoardConnection {
    pub name: String,
    pub port: String,
    pub to: String,
    pub kind: String,
}

#[derive(Clone, Debug)]
pub struct PartInstance {
    pub name: String,
    pub use_id: String,
    pub bind: String,
    pub conn: BTreeMap<String, ConfigValue>,
}

impl BoardManifest {
    pub fn validate(&self) -> Result<(), DescriptorError> {
        validate_catalog_id("board.id", &self.board.id, &["board"])?;
        validate_catalog_id("board.soc", &self.board.soc, &["soc"])?;
        non_empty("board.summary", &self.board.summary)?;

        // Connection declaration names and part instance names occupy separate
        // namespaces; only part names become graph nodes.
        let mut connection_names = BTreeSet::new();
        let mut part_names = BTreeSet::new();
        for connection in &self.conn {
            validate_local_name("conn.name", &connection.name)?;
            unique_name("conn.name", &connection.name, &mut connection_names)?;
            non_empty("conn.port", &connection.port)?;
            non_empty("conn.to", &connection.to)?;
            non_empty("conn.kind", &connection.kind)?;
        }
        let mut bus_addresses = BTreeSet::new();
        let mut chip_selects = BTreeSet::new();
        for part in &self.part {
            validate_local_name("part.name", &part.name)?;
            unique_name("part.name", &part.name, &mut part_names)?;
            validate_catalog_id("part.use", &part.use_id, &["part"])?;
            non_empty("part.bind", &part.bind)?;
            if part.conn.is_empty() {
                return Err(DescriptorError::new(
                    "part.conn",
                    format!("part {:?} must declare connection parameters", part.name),
                ));
            }
            let bus = part.conn.get("bus").and_then(ConfigValue::as_str);
            match part.bind.as_str() {
                "i2c" => {
                    let bus = bus.ok_or_else(|| {
                        DescriptorError::new(
                            "part.conn.bus",
                            format!("I2C part {:?} must declare a string bus", part.name),
                        )
                    })?;
                    non_empty("part.conn.bus", bus)?;
                    let address = part
                        .conn
                        .get("addr")
                        .and_then(ConfigValue::as_integer)
                        .ok_or_else(|| {
                            DescriptorError::new(
                                "part.conn.addr",
                                format!("I2C part {:?} must declare an integer address", part.name),
                            )
                        })?;
                    if !(0..=0x3ff).contains(&address) {
                        return Err(DescriptorError::new(
                            "part.conn.addr",
                            format!(
                                "part {:?} has an out-of-range I2C address {address:#x}",
                                part.name
                            ),
                        ));
                    }
                    let key = (bus.to_owned(), address);
                    if !bus_addresses.insert(key.clone()) {
                        return Err(DescriptorError::new(
                            "part.conn.addr",
                            format!("duplicate I2C address {:#x} on bus {:?}", address, key.0),
                        ));
                    }
                }
                "spi" => {
                    let bus = bus.ok_or_else(|| {
                        DescriptorError::new(
                            "part.conn.bus",
                            format!("SPI part {:?} must declare a string bus", part.name),
                        )
                    })?;
                    non_empty("part.conn.bus", bus)?;
                    let cs = part
                        .conn
                        .get("cs")
                        .and_then(ConfigValue::as_integer)
                        .ok_or_else(|| {
                            DescriptorError::new(
                                "part.conn.cs",
                                format!(
                                    "SPI part {:?} must declare an integer chip-select",
                                    part.name
                                ),
                            )
                        })?;
                    if cs < 0 {
                        return Err(DescriptorError::new(
                            "part.conn.cs",
                            format!("part {:?} has a negative chip-select index", part.name),
                        ));
                    }
                    let key = (bus.to_owned(), cs);
                    if !chip_selects.insert(key.clone()) {
                        return Err(DescriptorError::new(
                            "part.conn.cs",
                            format!("duplicate SPI chip-select {cs} on bus {:?}", key.0),
                        ));
                    }
                }
                other => {
                    return Err(DescriptorError::new(
                        "part.bind",
                        format!(
                            "part {:?} uses unsupported binding kind {:?}; expected i2c or spi",
                            part.name, other
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn validate_catalog_id(
    path: &'static str,
    value: &str,
    allowed_namespaces: &[&str],
) -> Result<(), DescriptorError> {
    let Some((namespace, suffix)) = value.split_once(':') else {
        return Err(DescriptorError::new(
            path,
            format!("catalog ID {value:?} must use a namespace prefix"),
        ));
    };
    if !allowed_namespaces.contains(&namespace) {
        return Err(DescriptorError::new(
            path,
            format!(
                "catalog ID {value:?} must use one of these namespaces: {}",
                allowed_namespaces.join(", ")
            ),
        ));
    }
    if suffix.is_empty()
        || suffix
            .split('/')
            .any(|segment| validate_local_name(path, segment).is_err())
    {
        return Err(DescriptorError::new(
            path,
            format!("catalog ID {value:?} has an invalid path suffix"),
        ));
    }
    Ok(())
}

pub(crate) fn non_empty(path: &'static str, value: &str) -> Result<(), DescriptorError> {
    if value.trim().is_empty() {
        Err(DescriptorError::new(path, "value must not be empty"))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_local_name(path: &'static str, value: &str) -> Result<(), DescriptorError> {
    non_empty(path, value)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DescriptorError::new(
            path,
            format!("invalid local name {value:?}; use ASCII letters, digits, '-' or '_'"),
        ));
    }
    Ok(())
}

fn unique_name(
    path: &'static str,
    value: &str,
    names: &mut BTreeSet<String>,
) -> Result<(), DescriptorError> {
    if names.insert(value.to_owned()) {
        Ok(())
    } else {
        Err(DescriptorError::new(
            path,
            format!("duplicate name {value:?}"),
        ))
    }
}
