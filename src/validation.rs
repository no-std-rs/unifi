//! Allocation-free structural checks shared by generated request validators.
use crate::{Json, JsonError};

/// Compare represented JSON values, including decoded strings and unordered object fields.
/// Integer values retain their i64 precision; other numbers use the crate's f64 representation.
pub fn equivalent(a: Json<'_>, b: Json<'_>) -> bool {
    if let (Ok(a), Ok(b)) = (a.string(), b.string()) {
        return a.chars().eq(b.chars());
    }
    if let (Ok(a), Ok(b)) = (a.integer(), b.integer()) {
        return a == b;
    }
    if let (Ok(a), Ok(b)) = (a.number(), b.number()) {
        return a.is_finite() && b.is_finite() && a == b;
    }
    if let (Ok(a), Ok(b)) = (a.items(), b.items()) {
        return a.clone().count() == b.clone().count() && a.zip(b).all(|(a, b)| equivalent(a, b));
    }
    if let (Ok(a), Ok(b)) = (a.fields(), b.fields()) {
        return a.clone().count() == b.clone().count()
            && a.into_iter().all(|(key, value)| {
                b.clone()
                    .find(|(k, _)| k.chars().eq(key.chars()))
                    .is_some_and(|(_, other)| equivalent(value, other))
            });
    }
    a.raw() == b.raw()
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
