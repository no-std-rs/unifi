use core::fmt::{self, Write};

pub fn merge_objects<'a>(
    base: Json<'_>,
    updates: Option<Json<'_>>,
    excluded: &[&str],
    buffer: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut writer =
        unifi::json::ObjectWriter::new(buffer).map_err(|_| "payload capacity exceeded")?;
    for (key, value) in base
        .fields()
        .map_err(|_| "existing setting must be an object")?
    {
        let name: heapless::String<512> = key.decode().map_err(|_| "setting name too long")?;
        if excluded.contains(&name.as_str()) {
            continue;
        }
        if updates.is_some_and(|v| v.get(&name).ok().flatten().is_some()) {
            continue;
        }
        writer
            .raw_field(&name, value)
            .map_err(|_| "payload capacity exceeded")?;
    }
    if let Some(updates) = updates {
        for (key, value) in updates.fields().map_err(|_| "update must be an object")? {
            let name: heapless::String<512> = key.decode().map_err(|_| "setting name too long")?;
            if excluded.contains(&name.as_str()) {
                continue;
            }
            writer
                .raw_field(&name, value)
                .map_err(|_| "payload capacity exceeded")?;
        }
    }
    Json::parse(writer.finish().map_err(|_| "payload capacity exceeded")?)
        .map_err(|_| "invalid merged payload")
}
use unifi::Json;

pub fn filter<'a>(
    value: Json<'_>,
    mut predicate: impl FnMut(Json<'_>) -> bool,
    buffer: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut out = crate::buffer::Buffer::new(buffer);
    out.write_char('[')
        .map_err(|_| "output capacity exceeded")?;
    let mut first = true;
    for item in value.items().map_err(|_| "expected a list response")? {
        if !predicate(item) {
            continue;
        }
        if !first {
            out.write_char(',')
                .map_err(|_| "output capacity exceeded")?;
        }
        first = false;
        out.write_str(item.raw())
            .map_err(|_| "output capacity exceeded")?;
    }
    out.write_char(']')
        .map_err(|_| "output capacity exceeded")?;
    Json::parse(out.finish()).map_err(|_| "invalid filtered response")
}
pub fn contains(value: Json<'_>, needle: &str) -> bool {
    let mut query = heapless::String::<1024>::new();
    for c in needle.chars() {
        for c in c.to_lowercase() {
            if query.push(c).is_err() {
                return false;
            }
        }
    }
    [
        "name",
        "hostname",
        "id",
        "_id",
        "ip",
        "ipAddress",
        "mac",
        "macAddress",
    ]
    .iter()
    .any(|key| {
        value
            .get(key)
            .ok()
            .flatten()
            .and_then(|v| v.string().ok())
            .is_some_and(|value| {
                let mut text = heapless::String::<1024>::new();
                for c in value.chars() {
                    for c in c.to_lowercase() {
                        if text.push(c).is_err() {
                            return false;
                        }
                    }
                }
                text.contains(query.as_str())
            })
    })
}
pub fn sanitize<'a>(
    value: Json<'_>,
    demo: bool,
    with_secrets: bool,
    buffer: &'a mut [u8],
) -> Result<Json<'a>, &'static str> {
    let mut output = crate::buffer::Buffer::new(buffer);
    rewrite(&mut output, value, demo, with_secrets, "")
        .map_err(|_| "sanitized output exceeds capacity")?;
    Json::parse(output.finish()).map_err(|_| "invalid sanitized response")
}
pub fn secret(name: &str) -> bool {
    name.starts_with("x_")
        || [
            "password",
            "passphrase",
            "psk",
            "privatekey",
            "private_key",
            "presharedkey",
            "preshared_key",
            "pre_shared_key",
            "secret",
            "cookie",
            "csrf",
            "token",
            "apikey",
            "api_key",
        ]
        .iter()
        .any(|key| name.contains(key))
}
fn rewrite(
    out: &mut dyn Write,
    value: Json<'_>,
    demo: bool,
    with_secrets: bool,
    key: &str,
) -> fmt::Result {
    if !with_secrets && secret(key) {
        return out.write_str("\"[REDACTED]\"");
    }
    if demo
        && value.string().is_ok()
        && ![
            "state",
            "status",
            "type",
            "model",
            "version",
            "applicationversion",
            "band",
            "action",
        ]
        .contains(&key)
    {
        return out.write_str("\"[demo]\"");
    }
    if let Ok(fields) = value.fields() {
        out.write_char('{')?;
        let mut first = true;
        for (name, child) in fields {
            if !first {
                out.write_char(',')?;
            }
            first = false;
            crate::output::quoted(out, name)?;
            out.write_char(':')?;
            let mut key = heapless::String::<512>::new();
            for c in name.chars() {
                key.push(c.to_ascii_lowercase()).map_err(|_| fmt::Error)?;
            }
            rewrite(out, child, demo, with_secrets, &key)?;
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
            rewrite(out, item, demo, with_secrets, key)?;
        }
        out.write_char(']')
    } else {
        out.write_str(value.raw())
    }
}

/// Merge positional command fields into a JSON object without dropping fields from a full payload.
pub fn merge<'a>(
    base: Option<&[u8]>,
    fields: &[(&str, &str)],
    json_fields: &[(&str, &str)],
    buffer: &'a mut [u8],
) -> Result<&'a [u8], &'static str> {
    fn key(s: &str) -> &str {
        s.trim_start_matches(['@', '=', '~'])
    }
    for (i, (name, _)) in fields.iter().enumerate() {
        if fields[..i].iter().any(|(old, _)| key(old) == key(name))
            || json_fields.iter().any(|(old, _)| *old == key(name))
        {
            return Err("duplicate payload field");
        }
    }
    for (i, (name, _)) in json_fields.iter().enumerate() {
        if json_fields[..i].iter().any(|(old, _)| old == name) {
            return Err("duplicate payload field");
        }
    }
    let mut writer =
        unifi::json::ObjectWriter::new(buffer).map_err(|_| "payload buffer too small")?;
    if let Some(base) = base {
        let json = Json::parse(base).map_err(|_| "invalid JSON body")?;
        for (name, value) in json
            .fields()
            .map_err(|_| "command payload must be an object")?
        {
            if fields
                .iter()
                .any(|(key, _)| name.equals(key.trim_start_matches(['@', '=', '~'])))
                || json_fields.iter().any(|(key, _)| name.equals(key))
            {
                continue;
            }
            let name: heapless::String<512> =
                name.decode().map_err(|_| "payload field name too long")?;
            writer
                .raw_field(&name, value)
                .map_err(|_| "payload exceeds capacity")?;
        }
    }
    for (name, value) in fields {
        if let Some(name) = name.strip_prefix('@') {
            let mut buffer = [0; 4096];
            let n = unifi::json::encode_to_slice(&[*value], &mut buffer)
                .map_err(|_| "payload value too long")?;
            writer
                .raw_field(name, Json::parse(&buffer[..n]).unwrap())
                .map_err(|_| "payload exceeds capacity")?;
        } else if let Some(name) = name.strip_prefix('~') {
            let mut items = heapless::Vec::<&str, 64>::new();
            for item in value.split(',') {
                items.push(item).map_err(|_| "too many list values")?;
            }
            let mut buffer = [0; 8192];
            let n = unifi::json::encode_to_slice(&items, &mut buffer)
                .map_err(|_| "list value too long")?;
            writer
                .raw_field(name, Json::parse(&buffer[..n]).unwrap())
                .map_err(|_| "payload exceeds capacity")?;
        } else if let Some(name) = name.strip_prefix('=') {
            writer
                .raw_field(
                    name,
                    Json::parse(value.as_bytes()).map_err(|_| "field value must be JSON")?,
                )
                .map_err(|_| "payload exceeds capacity")?;
        } else {
            writer
                .field(name, value)
                .map_err(|_| "payload exceeds capacity")?;
        }
    }
    for (name, value) in json_fields {
        writer
            .raw_field(
                name,
                Json::parse(value.as_bytes()).map_err(|_| "field value must be JSON")?,
            )
            .map_err(|_| "payload exceeds capacity")?;
    }
    writer.finish().map_err(|_| "payload exceeds capacity")
}
pub fn select<'a>(value: Json<'a>, field: &str, wanted: &str) -> Result<Json<'a>, &'static str> {
    let records = value
        .get("data")
        .ok()
        .flatten()
        .or_else(|| value.get("connections").ok().flatten())
        .unwrap_or(value);
    let mut found = None;
    if field == "id" {
        for item in records
            .items()
            .map_err(|_| "resource lookup did not return an array")?
        {
            if ["id", "_id"].iter().any(|key| {
                item.get(key)
                    .ok()
                    .flatten()
                    .and_then(|v| v.string().ok())
                    .is_some_and(|v| {
                        v.chars()
                            .map(|c| c.to_ascii_lowercase())
                            .eq(wanted.chars().map(|c| c.to_ascii_lowercase()))
                    })
            }) {
                if found.is_some() {
                    return Err("resource identifier is duplicated");
                }
                found = Some(item);
            }
        }
        if let Some(item) = found {
            return Ok(item);
        }
    }
    let wanted_mac = crate::lookup::mac(wanted).ok();
    for item in records
        .items()
        .map_err(|_| "resource lookup did not return an array")?
    {
        let keys = if field != "id" {
            core::slice::from_ref(&field)
        } else {
            &[
                "id",
                "_id",
                "name",
                "description",
                "mac",
                "macAddress",
                "ip",
                "ipAddress",
                "hostname",
            ][..]
        };
        if keys.iter().any(|key| {
            item.get(key)
                .ok()
                .flatten()
                .and_then(|v| v.string().ok())
                .is_some_and(|v| {
                    v.chars()
                        .map(|c| c.to_ascii_lowercase())
                        .eq(wanted.chars().map(|c| c.to_ascii_lowercase()))
                        || ["mac", "macAddress"].contains(key)
                            && wanted_mac.as_ref().is_some_and(|wanted| {
                                v.decode::<512>()
                                    .ok()
                                    .and_then(|s| crate::lookup::mac(&s).ok())
                                    .as_ref()
                                    == Some(wanted)
                            })
                })
        }) {
            if found.is_some() {
                return Err("resource selector is ambiguous; use an exact ID");
            }
            found = Some(item);
        }
    }
    found.ok_or("resource not found")
}
