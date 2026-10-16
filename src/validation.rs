//! Allocation-free structural checks shared by generated request validators.
use crate::{Json, JsonError, json::Fields};

/// Compare represented JSON values, including decoded strings and unordered object fields.
/// Integer values retain their i64 precision; other numbers use the crate's f64 representation.
/// Non-finite numeric representations compare by their original JSON text.
/// Repeated object keys use their last value, as in `Json::get`.
pub fn equivalent(a: Json<'_>, b: Json<'_>) -> bool {
    if let (Ok(a), Ok(b)) = (a.string(), b.string()) {
        return a.chars().eq(b.chars());
    }
    if let (Ok(a), Ok(b)) = (a.integer(), b.integer()) {
        return a == b;
    }
    if let (Ok(a), Ok(b)) = (a.number(), b.number())
        && a.is_finite()
        && b.is_finite()
    {
        return a == b;
    }
    if let (Ok(a), Ok(b)) = (a.items(), b.items()) {
        return a.clone().count() == b.clone().count() && a.zip(b).all(|(a, b)| equivalent(a, b));
    }
    if let (Ok(a), Ok(b)) = (a.fields(), b.fields()) {
        return object_subset(a.clone(), b.clone()) && object_subset(b, a);
    }
    a.raw() == b.raw()
}

fn object_subset(mut a: Fields<'_>, b: Fields<'_>) -> bool {
    while let Some((key, value)) = a.next() {
        // An occurrence followed by the same decoded key does not affect the value.
        if a.clone().any(|(k, _)| k.chars().eq(key.chars())) {
            continue;
        }
        if !b
            .clone()
            .filter(|(k, _)| k.chars().eq(key.chars()))
            .last()
            .is_some_and(|(_, other)| equivalent(value, other))
        {
            return false;
        }
    }
    true
}

pub fn unique(value: Json<'_>) -> Result<(), JsonError> {
    let items = value.items()?;
    for (i, item) in items.clone().enumerate() {
        if items
            .clone()
            .take(i)
            .any(|previous| equivalent(previous, item))
        {
            return Err(JsonError::Type);
        }
    }
    Ok(())
}
