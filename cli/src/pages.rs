use crate::{Options, Url, request};
use unifi::{Call, Json};

fn parameter<'a>(call: &'a Call, name: &str) -> Option<&'a str> {
    call.path().split_once('?').and_then(|(_, q)| {
        q.split('&').find_map(|p| {
            p.split_once('=')
                .filter(|(key, _)| *key == name)
                .map(|(_, value)| value)
        })
    })
}
#[allow(clippy::too_many_arguments)]
pub fn collect<'a>(
    url: &Url<'_>,
    base: &str,
    key: &str,
    call: &Call,
    options: &Options<'_>,
    response: &mut [u8],
    output: &'a mut [u8],
) -> Result<&'a [u8], &'static str> {
    let offset_pages = call.operation.query.iter().any(|q| q.name == "offset");
    let token_pages = call.operation.query.iter().any(|q| q.name == "nextToken");
    let mut offset = parameter(call, "offset")
        .unwrap_or("0")
        .parse::<u64>()
        .map_err(|_| "offset must be numeric")?;
    let limit = parameter(call, "limit")
        .unwrap_or("200")
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or("limit must be positive")?;
    let mut next = None::<heapless::String<512>>;
    let mut used = 0;
    append(output, &mut used, b"[")?;
    let mut first = true;
    loop {
        let mut page = if offset_pages {
            call.clone()
                .page(offset, limit)
                .map_err(|_| "pagination URL exceeds capacity")?
        } else {
            call.clone()
        };
        if let Some(token) = &next {
            page = page
                .set_query("nextToken", token)
                .map_err(|_| "cloud token exceeds capacity")?;
        }
        let response = request(url, base, key, &page, None, options, response)?;
        if !(200..300).contains(&response.status) {
            return Err("pagination request failed; inspect the operation without --all");
        }
        let json = response.json().map_err(|_| "invalid pagination response")?;
        let records = if json.get("meta").ok().flatten().is_some() {
            unifi::session::data(json).map_err(|_| "session API returned an error")?
        } else {
            json.get("data").ok().flatten().unwrap_or(json)
        };
        let mut count = 0;
        for item in records
            .items()
            .map_err(|_| "--all requires a list response")?
        {
            if !first {
                append(output, &mut used, b",")?;
            }
            first = false;
            append(output, &mut used, item.raw().as_bytes())?;
            count += 1;
        }
        if offset_pages {
            offset += count;
            let total = json
                .required("totalCount")
                .and_then(Json::unsigned)
                .map_err(|_| "pagination totalCount missing")?;
            if offset >= total {
                break;
            }
            if count == 0 {
                return Err("pagination did not advance");
            }
        } else if token_pages {
            let token = json
                .optional("nextToken")
                .map_err(|_| "invalid cloud pagination")?
                .map(|v| v.string().and_then(|s| s.decode()))
                .transpose()
                .map_err(|_| "invalid cloud token")?
                .filter(|s: &heapless::String<512>| !s.is_empty());
            if token.is_none() {
                break;
            }
            if token == next {
                return Err("cloud pagination did not advance");
            }
            next = token;
        } else {
            break;
        }
    }
    append(output, &mut used, b"]")?;
    Ok(&output[..used])
}
fn append(output: &mut [u8], used: &mut usize, bytes: &[u8]) -> Result<(), &'static str> {
    let end = used
        .checked_add(bytes.len())
        .ok_or("combined response exceeds capacity")?;
    output
        .get_mut(*used..end)
        .ok_or("combined response exceeds capacity; use explicit pages")?
        .copy_from_slice(bytes);
    *used = end;
    Ok(())
}
