//! Bounded Go-style duration syntax used by Unified's subscription option.
use core::time::Duration;
pub fn parse(value: &str) -> Result<Duration, &'static str> {
    const ERROR: &str =
        "keep-alive interval must be positive and at most 1h (for example 250ms, 30s or 1m30s)";
    let mut rest = value.strip_prefix('+').unwrap_or(value);
    let mut total = 0u128;
    let mut segments = 0;
    if rest.is_empty() || rest.len() > 128 {
        return Err(ERROR);
    }
    while !rest.is_empty() {
        let length = rest
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b'.')
            .count();
        let number = &rest[..length];
        rest = &rest[length..];
        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        if number.is_empty()
            || whole.is_empty() && fraction.is_empty()
            || fraction.len() > 18
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(ERROR);
        }
        let whole = if whole.is_empty() {
            0
        } else {
            whole.parse::<u128>().map_err(|_| ERROR)?
        };
        let fraction = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u128>().map_err(|_| ERROR)?
        };
        let denominator = 10u128.pow(number.split_once('.').map_or(0, |(_, f)| f.len() as u32));
        let multiplier = if rest.is_empty() && segments == 0 {
            1_000_000_000
        } else {
            let units = [
                ("ns", 1),
                ("us", 1_000),
                ("µs", 1_000),
                ("μs", 1_000),
                ("ms", 1_000_000),
                ("s", 1_000_000_000),
                ("m", 60_000_000_000),
                ("h", 3_600_000_000_000),
            ];
            let (unit, multiplier) = units
                .into_iter()
                .find(|(unit, _)| rest.starts_with(unit))
                .ok_or(ERROR)?;
            rest = &rest[unit.len()..];
            multiplier
        };
        let nanos = whole
            .checked_mul(multiplier)
            .and_then(|v| v.checked_add(fraction * multiplier / denominator))
            .ok_or(ERROR)?;
        total = total
            .checked_add(nanos)
            .filter(|v| *v <= 3_600_000_000_000)
            .ok_or(ERROR)?;
        segments += 1;
    }
    if total == 0 {
        return Err(ERROR);
    }
    Ok(Duration::from_nanos(total as u64))
}
