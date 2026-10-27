//! Shared parser for schema-level endpoint paths such as `soc.uart0.irq`.

use crate::schema::{non_empty, validate_local_name, DescriptorError};

#[derive(Clone, Debug)]
pub(crate) struct ParsedEndpoint {
    pub(crate) node_id: String,
    pub(crate) port: String,
}

pub(crate) fn parse_endpoint(
    value: &str,
    default_namespace: &str,
) -> Result<ParsedEndpoint, DescriptorError> {
    non_empty("endpoint", value)?;
    if let Some(name) = value.strip_prefix("wire:") {
        validate_local_name("endpoint.wire", name)?;
        return Ok(ParsedEndpoint {
            node_id: value.to_owned(),
            port: String::new(),
        });
    }
    let (namespace, tail) = if let Some(tail) = value.strip_prefix("soc.") {
        ("soc", tail)
    } else if let Some(tail) = value.strip_prefix("board.") {
        ("board", tail)
    } else {
        (default_namespace, value)
    };
    let split_at = tail.find(['.', ':']);
    let (name, port) = match split_at {
        Some(index) => {
            let port = tail[index + 1..].to_owned();
            if port.is_empty() {
                return Err(DescriptorError::new(
                    "endpoint.port",
                    format!("endpoint {value:?} has an empty port path"),
                ));
            }
            for segment in port.split(['.', ':']) {
                validate_local_name("endpoint.port", segment)?;
            }
            (&tail[..index], port)
        }
        None => (tail, String::new()),
    };
    validate_local_name("endpoint.node", name)?;
    Ok(ParsedEndpoint {
        node_id: format!("{namespace}.{name}"),
        port,
    })
}
