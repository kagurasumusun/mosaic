//! Load the checked-in STM32F103 SoC declaration. Parsing stays in the app so
//! `mosaic-describe` does not take a TOML dependency (設計書 §7.1).

use std::collections::BTreeMap;

use mosaic_describe::{
    AddressSpaceView, BlockInstance, CpuInstance, IrqRoute, Placement, RegionDescriptor,
    RegionKind, SocIdentity, SocManifest,
};

pub fn stm32f103_soc() -> Result<SocManifest, String> {
    let value: toml::Value =
        toml::from_str(include_str!("../../../catalog/soc/st/stm32f103/soc.toml"))
            .map_err(|error| error.to_string())?;
    parse_soc(&value)
}

fn parse_soc(value: &toml::Value) -> Result<SocManifest, String> {
    let root = value.as_table().ok_or("soc.toml: expected a table")?;
    reject_unknown(root, &["soc", "cpu", "block", "view"])?;
    let soc = table(root, "soc")?;
    reject_unknown(soc, &["id", "summary"])?;
    let cpu = array(root, "cpu")?
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
    let block = array(root, "block")?
        .iter()
        .map(parse_block)
        .collect::<Result<Vec<_>, String>>()?;
    let view = array(root, "view")?
        .iter()
        .map(|item| {
            let item = item.as_table().ok_or("view: expected a table")?;
            reject_unknown(item, &["name", "region"])?;
            let region = array(item, "region")?
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
        Some(_) => {
            return Err("block.clk is not used by the STM32F103 loader".into());
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
    if region.contains_key("priority") {
        return Err("region.priority is not used by the STM32F103 map".into());
    }
    let kind = match string(region, "kind")?.as_str() {
        "ram" => RegionKind::Ram,
        "rom" => RegionKind::Rom,
        "mmio" => RegionKind::Mmio,
        other => return Err(format!("unknown region kind {other:?}")),
    };
    Ok(RegionDescriptor {
        name: string(region, "name")?,
        target: string(region, "target")?,
        base: unsigned(region, "base")?,
        size: unsigned(region, "size")?,
        kind,
        priority: None,
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

fn array<'a>(value: &'a toml::Table, key: &str) -> Result<&'a Vec<toml::Value>, String> {
    value
        .get(key)
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("{key}: expected an array"))
}

fn string(table: &toml::Table, key: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{key}: expected a string"))
}

fn unsigned(table: &toml::Table, key: &str) -> Result<u64, String> {
    let value = table
        .get(key)
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| format!("{key}: expected an integer"))?;
    u64::try_from(value).map_err(|_| format!("{key}: expected a non-negative integer"))
}
