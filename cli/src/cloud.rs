//! Build ISP query site objects from explicit pairs or paginated fleet discovery.
use crate::{
    buffer::Buffer,
    compat,
    flows::{self, Remote},
    pages, payload,
};
use core::fmt::Write;
use unifi::{Json, site_manager};
fn equals(value: Json<'_>, field: &str, wanted: &str) -> bool {
    value
        .get(field)
        .ok()
        .flatten()
        .and_then(|v| v.string().ok())
        .is_some_and(|v| {
            v.chars()
                .map(|c| c.to_ascii_lowercase())
                .eq(wanted.chars().map(|c| c.to_ascii_lowercase()))
        })
}
pub fn metrics<'a>(
    remote: &mut Remote<'_, '_>,
    selectors: &str,
    incoming: Option<&[u8]>,
    output: &'a mut [u8],
) -> Result<&'a [u8], &'static str> {
    let base = incoming
        .map(Json::parse)
        .transpose()
        .map_err(|_| "invalid metric query JSON")?
        .unwrap_or(Json::parse(b"{}").unwrap());
    if base
        .get("sites")
        .map_err(|_| "metric query body must be an object")?
        .is_some()
    {
        return Err("sites supplied both as JSON and --sites");
    }
    let mut site_buffer = [0; 131072];
    let mut array_buffer = [0; 131072];
    let selected = if selectors.trim_start().starts_with('[') {
        Json::parse(selectors.as_bytes())
            .map_err(|_| "--sites JSON must be an array of hostId/siteId objects")?
    } else {
        let values = compat::csv(selectors)?;
        if values.is_empty() {
            return Err("--sites requires site IDs, HOST/SITE pairs or a JSON array");
        }
        let console = remote.options.console;
        let discovered = if values.iter().any(|v| !v.contains('/')) {
            Some(
                Json::parse(pages::collect(
                    remote.url,
                    remote.base,
                    remote.key,
                    &site_manager::list_sites().unwrap(),
                    remote.options,
                    remote.response,
                    &mut site_buffer,
                )?)
                .unwrap(),
            )
        } else {
            None
        };
        let mut array = Buffer::new(&mut array_buffer);
        array.write_char('[').unwrap();
        for (index, selector) in values.iter().enumerate() {
            let host;
            let site;
            let (host_id, site_id) = if let Some(pair) = selector.split_once('/') {
                if pair.0.is_empty() || pair.1.is_empty() {
                    return Err("explicit metric site selectors use HOST/SITE");
                }
                pair
            } else {
                let mut found = None;
                for row in discovered.unwrap().items().unwrap() {
                    if console.is_some_and(|id| !equals(row, "hostId", id)) {
                        continue;
                    }
                    let meta = row.get("meta").ok().flatten();
                    if equals(row, "siteId", selector)
                        || meta.is_some_and(|m| {
                            equals(m, "name", selector)
                                || equals(m, "desc", selector)
                                || equals(m, "description", selector)
                        })
                    {
                        if found.is_some() {
                            return Err("site selector is ambiguous; supply HOST/SITE");
                        }
                        found = Some(row);
                    }
                }
                let row = found.ok_or("metric site not found in fleet discovery")?;
                host = flows::owned::<512>(row, "hostId")?;
                site = flows::owned::<512>(row, "siteId")?;
                (&host[..], &site[..])
            };
            if index > 0 {
                array
                    .write_char(',')
                    .map_err(|_| "metric sites exceed capacity")?;
            }
            let mut item = [0; 4096];
            let mut w = unifi::json::ObjectWriter::new(&mut item).unwrap();
            w.field("hostId", &host_id)
                .map_err(|_| "metric host ID exceeds capacity")?;
            w.field("siteId", &site_id)
                .map_err(|_| "metric site ID exceeds capacity")?;
            array
                .write_str(
                    core::str::from_utf8(w.finish().map_err(|_| "metric site exceeds capacity")?)
                        .unwrap(),
                )
                .map_err(|_| "metric sites exceed capacity")?;
        }
        array
            .write_char(']')
            .map_err(|_| "metric sites exceed capacity")?;
        Json::parse(array.finish()).unwrap()
    };
    if selected
        .items()
        .map_err(|_| "--sites JSON must be an array")?
        .next()
        .is_none()
    {
        return Err("--sites requires at least one site");
    }
    for row in selected.items().unwrap() {
        flows::owned::<512>(row, "hostId")?;
        flows::owned::<512>(row, "siteId")?;
    }
    let mut bytes = [0; 131072];
    let mut body = unifi::json::ObjectWriter::new(&mut bytes).unwrap();
    body.raw_field("sites", selected)
        .map_err(|_| "metric query exceeds capacity")?;
    let result = payload::merge_objects(
        base,
        Some(Json::parse(body.finish().map_err(|_| "metric query exceeds capacity")?).unwrap()),
        &[],
        output,
    )?;
    site_manager::models::QueryIspMetricsRequest::validate(result)
        .map_err(|_| "metric query does not match the pinned schema")?;
    Ok(result.raw().as_bytes())
}
