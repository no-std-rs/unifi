use crate::{Options, Url, request};
use heapless::String;
use unifi::{Call, Json};

pub fn mac(text: &str) -> Result<String<17>, &'static str> {
    let mut digits = heapless::Vec::<u8, 12>::new();
    for b in text.bytes() {
        if b.is_ascii_hexdigit() {
            digits
                .push(b.to_ascii_lowercase())
                .map_err(|_| "invalid MAC address")?;
        } else if !b":-.".contains(&b) {
            return Err("invalid MAC address");
        }
    }
    if digits.len() != 12 {
        return Err("invalid MAC address");
    }
    let mut out = String::new();
    for (i, b) in digits.into_iter().enumerate() {
        if i > 0 && i % 2 == 0 {
            out.push(':').unwrap();
        }
        out.push(b as char).unwrap();
    }
    Ok(out)
}
pub fn uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
fn matches(value: Json<'_>, wanted: &str) -> bool {
    [
        "id",
        "_id",
        "name",
        "hostname",
        "mac",
        "macAddress",
        "ip",
        "ipAddress",
    ]
    .iter()
    .any(|key| {
        value
            .get(key)
            .ok()
            .flatten()
            .and_then(|v| v.string().ok())
            .is_some_and(|text| {
                text.chars()
                    .map(|c| c.to_ascii_lowercase())
                    .eq(wanted.chars().map(|c| c.to_ascii_lowercase()))
                    || {
                        let candidate: Result<String<512>, _> = text.decode();
                        candidate
                            .ok()
                            .and_then(|s| mac(&s).ok())
                            .zip(mac(wanted).ok())
                            .is_some_and(|(a, b)| a == b)
                    }
            })
    })
}
#[allow(clippy::too_many_arguments)]
pub fn resolve(
    url: &Url<'_>,
    base: &str,
    key: &str,
    call: Call,
    selector: &str,
    field: &str,
    options: &Options<'_>,
    buffer: &mut [u8],
) -> Result<String<512>, &'static str> {
    let paged = call.operation.query.iter().any(|q| q.name == "offset");
    let mut offset = 0;
    let mut found = None;
    loop {
        let call = if paged {
            call.clone()
                .page(offset, 200)
                .map_err(|_| "invalid resource pagination")?
        } else {
            call.clone()
        };
        let response = request(url, base, key, &call, None, options, buffer)?;
        if !(200..300).contains(&response.status) {
            return Err("resource lookup failed; inspect the list command");
        }
        let json = response.json().map_err(|_| "invalid resource list")?;
        let records = if json.get("meta").ok().flatten().is_some() {
            unifi::session::data(json).map_err(|_| "session resource lookup failed")?
        } else {
            json.get("data").ok().flatten().unwrap_or(json)
        };
        let mut count = 0;
        for item in records
            .items()
            .map_err(|_| "resource list is not an array")?
        {
            count += 1;
            if matches(item, selector) {
                if found.is_some() {
                    return Err("resource selector is ambiguous; use its exact ID or MAC");
                }
                found = Some(
                    item.required(field)
                        .and_then(Json::string)
                        .and_then(|v| v.decode())
                        .map_err(|_| "resource identifier is missing or too long")?,
                );
            }
        }
        if !paged {
            break;
        }
        offset += count;
        let total = json
            .required("totalCount")
            .and_then(Json::unsigned)
            .map_err(|_| "invalid resource pagination")?;
        if offset >= total {
            break;
        }
        if count == 0 {
            return Err("resource pagination did not advance");
        }
    }
    found.ok_or("resource not found; inspect the list command")
}
