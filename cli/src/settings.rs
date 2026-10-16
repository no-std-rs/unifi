use crate::{Options, Url, payload, request};
use unifi::Json;

#[allow(clippy::too_many_arguments)]
pub fn prepare<'a>(
    url: &Url<'_>,
    base: &str,
    key: &str,
    site: &str,
    section: &str,
    options: &Options<'_>,
    incoming: Option<&[u8]>,
    fields: &[(&str, &str)],
    json_fields: &[(&str, &str)],
    response: &mut [u8],
    output: &'a mut [u8],
) -> Result<&'a [u8], &'static str> {
    if incoming.is_none() && fields.is_empty() && json_fields.is_empty() {
        return Err("no setting changes supplied");
    }
    let reply = request(
        url,
        base,
        key,
        &unifi::session_api::list_settings(site).unwrap(),
        None,
        options,
        response,
    )?;
    if !(200..300).contains(&reply.status) {
        return Err("cannot read setting before update");
    }
    let current = payload::select(
        unifi::session::data(reply.json().map_err(|_| "invalid setting response")?)
            .map_err(|_| "session settings read failed")?,
        "key",
        section,
    )?;
    let incoming = incoming
        .map(Json::parse)
        .transpose()
        .map_err(|_| "invalid setting update JSON")?;
    let mut merged = [0; 131072];
    if fields.iter().any(|(key, _)| {
        ["_id", "site_id", "key"].contains(&key.trim_start_matches(['@', '=', '~']))
    }) || json_fields
        .iter()
        .any(|(key, _)| ["_id", "site_id", "key"].contains(key))
    {
        return Err("setting identity is selected through the command, not a body field");
    }
    let merged =
        payload::merge_objects(current, incoming, &["_id", "site_id", "key"], &mut merged)?;
    let bytes = payload::merge(Some(merged.raw().as_bytes()), fields, json_fields, output)?;
    let final_body = Json::parse(bytes).unwrap();
    if !options.allow_secret_removal {
        check_credentials(current, Some(final_body), false)?;
    }

    Ok(bytes)
}

fn empty(value: Json<'_>) -> bool {
    value.is_null()
        || value.string().is_ok_and(|s| s.chars().next().is_none())
        || value.items().is_ok_and(|mut v| v.next().is_none())
        || value.fields().is_ok_and(|mut v| v.next().is_none())
}
pub(crate) fn check_credentials(
    previous: Json<'_>,
    next: Option<Json<'_>>,
    sensitive: bool,
) -> Result<(), &'static str> {
    if sensitive && !empty(previous) && next.is_none_or(empty) {
        return Err("clearing a stored credential requires --allow-secret-removal");
    }
    if let Ok(fields) = previous.fields() {
        for (key, _) in fields {
            let key: heapless::String<512> = key.decode().map_err(|_| "setting name too long")?;
            let mut lower = heapless::String::<512>::new();
            for c in key.chars() {
                lower.push(c.to_ascii_lowercase()).unwrap();
            }
            let old = previous.required(&key).map_err(|_| "invalid old setting")?;
            check_credentials(
                old,
                next.and_then(|v| v.get(&key).ok().flatten()),
                sensitive || payload::secret(&lower),
            )?;
        }
    } else if let Ok(items) = previous.items() {
        for (index, old) in items.enumerate() {
            check_credentials(
                old,
                next.and_then(|v| v.items().ok()?.nth(index)),
                sensitive,
            )?;
        }
    }
    Ok(())
}
