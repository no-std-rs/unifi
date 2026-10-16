use core::fmt::{self, Write};
use unifi::Json;

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Table,
    Json,
    Compact,
    Plain,
    Yaml,
}
impl Format {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "table" => Ok(Self::Table),
            "json" => Ok(Self::Json),
            "json-compact" => Ok(Self::Compact),
            "plain" => Ok(Self::Plain),
            "yaml" => Ok(Self::Yaml),
            _ => Err("output must be table, json, json-compact, yaml or plain"),
        }
    }
}
fn indent(out: &mut dyn Write, depth: usize) -> fmt::Result {
    for _ in 0..depth {
        out.write_str("  ")?;
    }
    Ok(())
}
pub(crate) fn quoted(out: &mut dyn Write, value: unifi::JsonString<'_>) -> fmt::Result {
    out.write_char('"')?;
    for c in value.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            c if c.is_control() || c == '\u{2028}' || c == '\u{2029}' => {
                write!(out, "\\u{:04x}", c as u32)?
            }
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}
fn pretty(out: &mut dyn Write, value: Json<'_>, depth: usize) -> fmt::Result {
    if let Ok(fields) = value.fields() {
        out.write_char('{')?;
        let mut first = true;
        for (key, value) in fields {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            out.write_char('\n')?;
            indent(out, depth + 1)?;
            quoted(out, key)?;
            out.write_str(": ")?;
            pretty(out, value, depth + 1)?;
        }
        if !first {
            out.write_char('\n')?;
            indent(out, depth)?;
        }
        out.write_char('}')
    } else if let Ok(items) = value.items() {
        out.write_char('[')?;
        let mut first = true;
        for item in items {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            out.write_char('\n')?;
            indent(out, depth + 1)?;
            pretty(out, item, depth + 1)?;
        }
        if !first {
            out.write_char('\n')?;
            indent(out, depth)?;
        }
        out.write_char(']')
    } else {
        out.write_str(value.raw())
    }
}
fn compact(out: &mut dyn Write, value: Json<'_>) -> fmt::Result {
    if let Ok(fields) = value.fields() {
        out.write_char('{')?;
        let mut first = true;
        for (key, value) in fields {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            quoted(out, key)?;
            out.write_char(':')?;
            compact(out, value)?;
        }
        out.write_char('}')
    } else if let Ok(items) = value.items() {
        out.write_char('[')?;
        let mut first = true;
        for item in items {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            compact(out, item)?;
        }
        out.write_char(']')
    } else {
        out.write_str(value.raw())
    }
}
fn yaml(out: &mut dyn Write, value: Json<'_>, depth: usize) -> fmt::Result {
    if let Ok(fields) = value.fields() {
        if fields.clone().next().is_none() {
            indent(out, depth)?;
            return out.write_str("{}\n");
        }
        for (key, value) in fields {
            indent(out, depth)?;
            quoted(out, key)?;
            out.write_char(':')?;
            if value.fields().is_ok() || value.items().is_ok() {
                out.write_char('\n')?;
                yaml(out, value, depth + 1)?;
            } else {
                out.write_char(' ')?;
                yaml_scalar(out, value)?;
                out.write_char('\n')?;
            }
        }
        Ok(())
    } else if let Ok(items) = value.items() {
        if items.clone().next().is_none() {
            indent(out, depth)?;
            return out.write_str("[]\n");
        }
        for value in items {
            indent(out, depth)?;
            out.write_char('-')?;
            if value.fields().is_ok() || value.items().is_ok() {
                out.write_char('\n')?;
                yaml(out, value, depth + 1)?;
            } else {
                out.write_char(' ')?;
                yaml_scalar(out, value)?;
                out.write_char('\n')?;
            }
        }
        Ok(())
    } else {
        indent(out, depth)?;
        yaml_scalar(out, value)?;
        out.write_char('\n')
    }
}
fn yaml_scalar(out: &mut dyn Write, value: Json<'_>) -> fmt::Result {
    if let Ok(text) = value.string() {
        quoted(out, text)
    } else {
        out.write_str(value.raw())
    }
}
fn cell(out: &mut dyn Write, value: Json<'_>, width: usize) -> fmt::Result {
    let mut used = 0;
    if let Ok(text) = value.string() {
        let total = text.chars().count();
        for c in text.chars().take(if total > width {
            width.saturating_sub(1)
        } else {
            width
        }) {
            out.write_char(if c.is_control() { '?' } else { c })?;
            used += 1;
        }
        if total > width {
            out.write_char('…')?;
            used += 1;
        }
    } else {
        let mut array = heapless::String::<128>::new();
        let simple_array = value.items().is_ok_and(|items| {
            items
                .clone()
                .take(16)
                .all(|v| v.fields().is_err() && v.items().is_err())
                && items.take(17).count() <= 16
        });
        let array_complete = if simple_array {
            compact(&mut array, value).is_ok()
        } else {
            false
        };
        let text = if value.fields().is_ok() {
            "{…}"
        } else if simple_array {
            array.as_str()
        } else if value.items().is_ok() {
            "[…]"
        } else {
            value.raw()
        };
        let truncated = text.chars().count() > width || simple_array && !array_complete;
        for c in text.chars().take(if truncated {
            width.saturating_sub(1)
        } else {
            width
        }) {
            out.write_char(c)?;
            used += 1;
        }
        if truncated {
            out.write_char('…')?;
            used += 1;
        }
    }
    for _ in used..width {
        out.write_char(' ')?;
    }
    Ok(())
}
fn table(out: &mut dyn Write, value: Json<'_>) -> fmt::Result {
    table_columns(
        out,
        value,
        &[
            "name",
            "state",
            "enabled",
            "model",
            "id",
            "type",
            "ipAddress",
            "hostname",
            "ssid",
            "_id",
        ],
    )
}
fn lookup<'a>(value: Json<'a>, path: &str) -> Option<Json<'a>> {
    match path {
        "@dnsValue" => [
            "ipv4Address",
            "ipv6Address",
            "targetDomain",
            "mailServerDomain",
            "text",
            "ipAddress",
            "serverDomain",
        ]
        .iter()
        .find_map(|key| value.optional(key).ok().flatten()),
        "@clientName" => ["name", "hostname", "macAddress", "mac", "id"]
            .iter()
            .find_map(|key| {
                value
                    .optional(key)
                    .ok()
                    .flatten()
                    .filter(|v| v.string().is_ok_and(|s| s.chars().next().is_some()))
            }),
        _ => {
            let mut value = value;
            for part in path.split('.') {
                value = value.optional(part).ok().flatten()?;
            }
            Some(value)
        }
    }
}
fn label(path: &str) -> &str {
    match path {
        "@dnsValue" => "value",
        "@clientName" => "name",
        "securityConfiguration.type" => "security",
        "broadcastingFrequenciesGHz" => "bands",
        "meta.name" => "name",
        "reportedState.name" => "name",
        "reportedState.state" => "state",
        _ => path,
    }
}
pub fn table_columns(out: &mut dyn Write, value: Json<'_>, preferred: &[&str]) -> fmt::Result {
    let records = value.get("data").ok().flatten().unwrap_or(value);
    if let Ok(items) = records.items() {
        let Some(_) = items.clone().next() else {
            return out.write_str("No results.\n");
        };
        let mut columns = heapless::Vec::<(&str, usize), 4>::new();
        for key in preferred.iter().copied() {
            if items.clone().any(|item| lookup(item, key).is_some()) {
                let max = if key == "connected" {
                    32
                } else if key == "id" || key == "_id" {
                    42
                } else {
                    24
                };
                let width = items
                    .clone()
                    .filter_map(|item| lookup(item, key))
                    .map(|v| {
                        v.string()
                            .map(|s| s.chars().count())
                            .unwrap_or(v.raw().len())
                    })
                    .max()
                    .unwrap_or(0)
                    .min(max)
                    .max(label(key).len());
                if columns.push((key, width)).is_err() {
                    break;
                }
            }
        }
        if columns.is_empty() {
            for item in items {
                compact(out, item)?;
                out.write_char('\n')?;
            }
            return Ok(());
        }
        for (i, (key, width)) in columns.iter().enumerate() {
            if i > 0 {
                out.write_str("  ")?;
            }
            for c in label(key).chars() {
                out.write_char(c.to_ascii_uppercase())?;
            }
            for _ in label(key).len()..*width {
                out.write_char(' ')?;
            }
        }
        out.write_char('\n')?;
        for item in items {
            for (i, (key, width)) in columns.iter().enumerate() {
                if i > 0 {
                    out.write_str("  ")?;
                }
                if let Some(value) = lookup(item, key) {
                    cell(out, value, *width)?;
                } else {
                    write!(out, "{:<width$}", "-", width = *width)?;
                }
            }
            out.write_char('\n')?;
        }
        Ok(())
    } else if let Ok(fields) = records.fields() {
        for (key, value) in fields {
            for c in key.chars() {
                out.write_char(if c.is_control() { '?' } else { c })?;
            }
            out.write_str(": ")?;
            if let Ok(text) = value.string() {
                for c in text.chars() {
                    out.write_char(if c.is_control() { '?' } else { c })?;
                }
            } else {
                compact(out, value)?;
            }
            out.write_char('\n')?;
        }
        Ok(())
    } else {
        writeln!(out, "{}", value.raw())
    }
}
pub fn render(out: &mut dyn Write, value: Json<'_>, format: Format) -> fmt::Result {
    match format {
        Format::Table => table(out, value),
        Format::Json => {
            pretty(out, value, 0)?;
            out.write_char('\n')
        }
        Format::Compact => {
            compact(out, value)?;
            out.write_char('\n')
        }
        Format::Yaml => yaml(out, value, 0),
        Format::Plain => {
            let records = value.get("data").ok().flatten().unwrap_or(value);
            if let Ok(items) = records.items() {
                for item in items {
                    let item = item
                        .get("name")
                        .ok()
                        .flatten()
                        .or_else(|| item.get("id").ok().flatten())
                        .unwrap_or(item);
                    if let Ok(text) = item.string() {
                        for c in text.chars() {
                            out.write_char(if c.is_control() { '?' } else { c })?;
                        }
                    } else {
                        compact(out, item)?;
                    }
                    out.write_char('\n')?;
                }
                Ok(())
            } else {
                table(out, records)
            }
        }
    }
}
