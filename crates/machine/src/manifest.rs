//! Board-manifest parsing shared by the machine models (ADR 0032) and the
//! SoC-manifest parser every machine reuses (ADR 0033 wave 3).
//!
//! A board's grammar — `[[part]]` with its `[[part.face]]` wiring — is the
//! same for every machine, and so is the SoC grammar (`[soc]`, `[[cpu]]`,
//! `[[block]]`, `[[view]]` + `[[view.region]]`). Both live here so
//! `mosaic-describe` stays TOML-free (design §7.1) and so adding a new
//! machine pulls in zero parser code (ADR 0024).

use std::collections::BTreeMap;

use mosaic_describe::{
    BoardConnection, BoardIdentity, BoardManifest, ConfigValue, PartFace, PartInstance,
};

/// Parse and validate a board manifest (`board.toml` shape).
pub fn parse_board(text: &str) -> Result<BoardManifest, String> {
    let value: toml::Value = toml::from_str(text).map_err(|error| error.to_string())?;
    let root = value
        .as_table()
        .ok_or_else(|| "board manifest: expected a table".to_owned())?;
    reject_unknown(root, &["board", "conn", "part"])?;
    let board = table(root, "board")?;
    reject_unknown(board, &["id", "soc", "summary"])?;
    let soc = match board.get("soc") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or_else(|| "board.soc: expected a string".to_owned())?
                .to_owned(),
        ),
    };
    let conn = match root.get("conn") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| "conn: expected an array".to_owned())?
            .iter()
            .map(|item| {
                let item = item
                    .as_table()
                    .ok_or_else(|| "conn: expected a table".to_owned())?;
                reject_unknown(item, &["name", "port", "to", "kind"])?;
                Ok(BoardConnection {
                    name: string(item, "name")?,
                    port: string(item, "port")?,
                    to: string(item, "to")?,
                    kind: string(item, "kind")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    let part = match root.get("part") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| "part: expected an array".to_owned())?
            .iter()
            .map(parse_part)
            .collect::<Result<Vec<_>, String>>()?,
    };
    let manifest = BoardManifest {
        board: BoardIdentity {
            id: string(board, "id")?,
            soc,
            summary: string(board, "summary")?,
        },
        conn,
        part,
    };
    manifest.validate().map_err(|error| error.to_string())?;
    Ok(manifest)
}

/// One `[[part]]` with its `[[part.face]]` wiring (ADR 0032).
fn parse_part(value: &toml::Value) -> Result<PartInstance, String> {
    let item = value
        .as_table()
        .ok_or_else(|| "part: expected a table".to_owned())?;
    reject_unknown(item, &["name", "use", "face"])?;
    let faces = match item.get("face") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .ok_or_else(|| "part.face: expected an array".to_owned())?
            .iter()
            .map(parse_face)
            .collect::<Result<Vec<_>, String>>()?,
    };
    Ok(PartInstance {
        name: string(item, "name")?,
        use_id: string(item, "use")?,
        faces,
    })
}

/// One `[[part.face]]`: a port face wired to its far endpoint.
fn parse_face(value: &toml::Value) -> Result<PartFace, String> {
    let item = value
        .as_table()
        .ok_or_else(|| "part.face: expected a table".to_owned())?;
    // Free wiring parameters: every key the face grammar does not own
    // (`address`, `pull`, `cs`, …) rides onto the graph edge for the
    // model's spawn; the schema validator checks the grammar itself.
    let owned = ["port", "kind", "to", "index", "pin"];
    let mut parameters = BTreeMap::new();
    for (key, value) in item {
        if owned.contains(&key.as_str()) {
            continue;
        }
        let value = config_value(value)
            .ok_or_else(|| format!("part.face.{key}: expected a scalar or array"))?;
        parameters.insert(key.clone(), value);
    }
    Ok(PartFace {
        port: string(item, "port")?,
        kind: string(item, "kind")?,
        to: string(item, "to")?,
        index: match item.get("index") {
            None => None,
            Some(value) => Some(
                value
                    .as_integer()
                    .and_then(|index| u64::try_from(index).ok())
                    .ok_or_else(|| "part.face.index: expected a non-negative integer".to_owned())?,
            ),
        },
        pin: match item.get("pin") {
            None => None,
            Some(_) => Some(string(item, "pin")?),
        },
        parameters,
    })
}

fn config_value(value: &toml::Value) -> Option<ConfigValue> {
    Some(match value {
        toml::Value::String(value) => ConfigValue::String(value.clone()),
        toml::Value::Integer(value) => ConfigValue::Integer(*value),
        toml::Value::Float(value) => ConfigValue::Float(*value),
        toml::Value::Boolean(value) => ConfigValue::Boolean(*value),
        toml::Value::Datetime(value) => ConfigValue::Datetime(value.to_string()),
        toml::Value::Array(values) => ConfigValue::Array(
            values
                .iter()
                .map(config_value)
                .collect::<Option<Vec<_>>>()?,
        ),
        toml::Value::Table(table) => ConfigValue::Table(
            table
                .iter()
                .map(|(key, value)| Some((key.clone(), config_value(value)?)))
                .collect::<Option<BTreeMap<_, _>>>()?,
        ),
    })
}

fn reject_unknown(table: &toml::Table, allowed: &[&str]) -> Result<(), String> {
    if let Some(key) = table.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("unknown key {key:?}"));
    }
    Ok(())
}

fn table<'a>(value: &'a toml::Table, key: &str) -> Result<&'a toml::Table, String> {
    value
        .get(key)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| format!("{key}: expected a table"))
}

fn string(table: &toml::Table, key: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{key}: expected a string"))
}

// ---------------------------------------------------------------------------
// SoC manifest parser (ADR 0033 wave 3: shared by every machine).
// ---------------------------------------------------------------------------

use mosaic_describe::{
    AddressSpaceView, BlockInstance, CpuInstance, IrqRoute, Placement, RegionDescriptor,
    RegionKind, SocIdentity, SocManifest,
};

/// Parse and validate a SoC manifest (`soc.toml` shape). Each machine
/// module calls this with its own `include_str!`, so the SoC grammar
/// stays in one place while the manifest text stays next to the machine.
pub fn parse_soc(text: &str) -> Result<SocManifest, String> {
    let value: toml::Value = toml::from_str(text).map_err(|error| error.to_string())?;
    parse_soc_value(&value)
}

fn parse_soc_value(value: &toml::Value) -> Result<SocManifest, String> {
    let root = value.as_table().ok_or("soc.toml: expected a table")?;
    reject_unknown(root, &["soc", "cpu", "block", "view"])?;
    let soc = table(root, "soc")?;
    reject_unknown(soc, &["id", "summary"])?;
    let cpu = root
        .get("cpu")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "cpu: expected an array".to_owned())?
        .iter()
        .map(|item| {
            let item = item.as_table().ok_or("cpu: expected a table")?;
            reject_unknown(item, &["name", "use", "view"])?;
            Ok(CpuInstance {
                name: string(item, "name")?,
                use_id: string(item, "use")?,
                view: string(item, "view")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let block = root
        .get("block")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "block: expected an array".to_owned())?
        .iter()
        .map(parse_block)
        .collect::<Result<Vec<_>, String>>()?;
    let view = root
        .get("view")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "view: expected an array".to_owned())?
        .iter()
        .map(|item| {
            let item = item.as_table().ok_or("view: expected a table")?;
            reject_unknown(item, &["name", "region"])?;
            let region = item
                .get("region")
                .and_then(toml::Value::as_array)
                .ok_or_else(|| "view.region: expected an array".to_owned())?
                .iter()
                .map(parse_region)
                .collect::<Result<Vec<_>, String>>()?;
            Ok(AddressSpaceView {
                name: string(item, "name")?,
                region,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let manifest = SocManifest {
        soc: SocIdentity {
            id: string(soc, "id")?,
            summary: string(soc, "summary")?,
        },
        cpu,
        block,
        view,
    };
    manifest.validate().map_err(|error| error.to_string())?;
    Ok(manifest)
}

fn parse_block(value: &toml::Value) -> Result<BlockInstance, String> {
    let item = value.as_table().ok_or("block: expected a table")?;
    reject_unknown(item, &["name", "use", "at", "irq", "clk"])?;
    let at = match item.get("at") {
        None => None,
        Some(value) => {
            let at = value.as_table().ok_or("block.at: expected a table")?;
            reject_unknown(at, &["space", "base"])?;
            Some(Placement {
                space: string(at, "space")?,
                base: unsigned(at, "base")?,
            })
        }
    };
    let irq = match item.get("irq") {
        None => Vec::new(),
        Some(value) => {
            let routes = if value.as_table().is_some() {
                vec![value]
            } else {
                value
                    .as_array()
                    .ok_or("block.irq: expected a table or array")?
                    .iter()
                    .collect()
            };
            routes
                .into_iter()
                .map(|value| {
                    let route = value.as_table().ok_or("irq: expected a table")?;
                    reject_unknown(route, &["pin", "to"])?;
                    let to = route
                        .get("to")
                        .and_then(toml::Value::as_array)
                        .ok_or("irq.to: expected an array")?
                        .iter()
                        .map(|value| {
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or_else(|| "irq.to: expected a string".to_owned())
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(IrqRoute {
                        pin: string(route, "pin")?,
                        to,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?
        }
    };
    let clk = match item.get("clk") {
        None => BTreeMap::new(),
        Some(value) => {
            let table = value
                .as_table()
                .ok_or("block.clk: expected a table of input = source")?;
            let mut map = BTreeMap::new();
            for (input, source) in table {
                let source = source.as_str().ok_or(
                    "block.clk: clock source must be a string (a clock net or a block.port reference)",
                )?;
                map.insert(
                    input.clone(),
                    ConfigValue::String(source.to_owned()),
                );
            }
            map
        }
    };
    Ok(BlockInstance {
        name: string(item, "name")?,
        use_id: string(item, "use")?,
        at,
        irq,
        clk,
    })
}

fn parse_region(value: &toml::Value) -> Result<RegionDescriptor, String> {
    let region = value.as_table().ok_or("region: expected a table")?;
    reject_unknown(
        region,
        &["name", "target", "base", "size", "kind", "priority"],
    )?;
    let kind = match string(region, "kind")?.as_str() {
        "ram" => RegionKind::Ram,
        "rom" => RegionKind::Rom,
        "mmio" => RegionKind::Mmio,
        other => return Err(format!("unknown region kind {other:?}")),
    };
    let priority = match region.get("priority") {
        None => None,
        Some(value) => Some(
            value
                .as_integer()
                .ok_or("region.priority: expected an integer")?
                as i32,
        ),
    };
    Ok(RegionDescriptor {
        name: string(region, "name")?,
        target: string(region, "target")?,
        base: unsigned(region, "base")?,
        size: unsigned(region, "size")?,
        kind,
        priority,
    })
}

fn unsigned(table: &toml::Table, key: &str) -> Result<u64, String> {
    let value = table
        .get(key)
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| format!("{key}: expected an integer"))?;
    if value < 0 {
        return Err(format!("{key}: expected an unsigned integer"));
    }
    Ok(value as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_face_parts_parse_with_their_wiring() {
        let manifest = parse_board(
            r#"
[board]
id = 'board:test/rig'
soc = 'soc:st/stm32f103'
summary = 'composition rig'

[[part]]
name = 'ioexp'
use = 'part:microchip/mcp23017'

[[part.face]]
port = 'i2c'
kind = 'i2c'
to = 'soc.i2c1:i2c'
address = 0x20

[[part.face]]
port = 'gpio'
kind = 'gpio'
to = 'wire:ioexp-gpa0'
index = 0
pull = 'up'

[[part.face]]
port = 'irq'
kind = 'irq'
to = 'wire:ioexp-int'
pin = 'int'
"#,
        )
        .expect("board manifest");
        let part = &manifest.part[0];
        assert_eq!(part.name, "ioexp");
        assert_eq!(part.faces.len(), 3);
        assert_eq!(part.faces[0].port, "i2c");
        assert_eq!(
            part.faces[0]
                .parameters
                .get("address")
                .and_then(ConfigValue::as_integer),
            Some(0x20)
        );
        assert_eq!(part.faces[1].index, Some(0));
        assert_eq!(part.faces[2].pin.as_deref(), Some("int"));
    }

    #[test]
    fn unknown_face_kinds_are_rejected() {
        let error = parse_board(
            r#"
[board]
id = 'board:test/rig'
soc = 'soc:test/soc'
summary = 'rig'

[[part]]
name = 'p'
use = 'part:test/p'

[[part.face]]
port = 'i2c'
kind = 'can'
to = 'soc.c:c'
address = 1
"#,
        )
        .expect_err("unsupported kind");
        assert!(error.contains("unsupported face kind"));
    }
}
