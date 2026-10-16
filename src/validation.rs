//! Allocation-free structural checks shared by generated request validators.
use crate::{Json, JsonError, json::Fields};

/// Compare represented JSON values, including decoded strings and unordered object fields.
/// Numbers compare as exact decimals, without floating-point rounding or bounded exponents.
/// Repeated object keys use their last value, as in `Json::get`.
pub fn equivalent(a: Json<'_>, b: Json<'_>) -> bool {
    if let (Ok(a), Ok(b)) = (a.string(), b.string()) {
        return a.chars().eq(b.chars());
    }
    if let (Ok(a), Ok(b)) = (a.integer(), b.integer()) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (Number::parse(a.raw()), Number::parse(b.raw())) {
        return a.negative == b.negative
            && a.digits
                .bytes()
                .filter(|b| *b != b'.')
                .eq(b.digits.bytes().filter(|b| *b != b'.'))
            && exponents_equal(a.exponent, b.exponent, b.shift - a.shift);
    }
    if let (Ok(a), Ok(b)) = (a.items(), b.items()) {
        return a.clone().count() == b.clone().count() && a.zip(b).all(|(a, b)| equivalent(a, b));
    }
    if let (Ok(a), Ok(b)) = (a.fields(), b.fields()) {
        return object_subset(a.clone(), b.clone()) && object_subset(b, a);
    }
    a.raw() == b.raw()
}

// A nonzero value is its significant decimal digits times 10^(exponent + shift).
// All slices borrow validated JSON; even an arbitrarily long exponent needs no buffer.
struct Number<'a> {
    negative: bool,
    digits: &'a str,
    exponent: &'a str,
    shift: i128,
}

impl<'a> Number<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        let negative = text.starts_with('-');
        let text = text.strip_prefix('-').unwrap_or(text);
        if !text.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            return None;
        }
        let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
        let digits = mantissa.trim_matches(['0', '.']);
        if digits.is_empty() {
            return Some(Self {
                negative: false,
                digits,
                exponent: "0",
                shift: 0,
            });
        }
        let fractional = mantissa
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        let trailing = mantissa
            .bytes()
            .rev()
            .filter(|b| *b != b'.')
            .take_while(|b| *b == b'0')
            .count();
        Some(Self {
            negative,
            digits,
            exponent,
            shift: trailing as i128 - fractional as i128,
        })
    }
}

// Compare exponent a - exponent b with a small shift derived only from slice lengths.
fn exponents_equal(a: &str, b: &str, difference: i128) -> bool {
    fn parts(text: &str) -> (bool, &str) {
        let digits = text.trim_start_matches(['+', '-']).trim_start_matches('0');
        (text.starts_with('-') && !digits.is_empty(), digits)
    }
    let (negative_a, a) = parts(a);
    let (negative_b, b) = parts(b);
    if negative_a != negative_b {
        // Opposite signs cannot cancel, so overflow proves the difference is too large.
        let magnitude = |digits: &str| {
            digits.bytes().try_fold(0u128, |n, digit| {
                n.checked_mul(10)?.checked_add(u128::from(digit - b'0'))
            })
        };
        return negative_a == (difference < 0)
            && magnitude(a)
                .zip(magnitude(b))
                .and_then(|(a, b)| a.checked_add(b))
                == Some(difference.unsigned_abs());
    }
    let order = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
    if order == core::cmp::Ordering::Equal {
        return difference == 0;
    }
    let a_smaller = order == core::cmp::Ordering::Less;
    if (a_smaller ^ negative_a) != (difference < 0) {
        return false;
    }
    let (larger, smaller) = if a_smaller { (b, a) } else { (a, b) };
    let mut smaller = smaller.bytes().rev();
    let mut expected = difference.unsigned_abs();
    let mut borrow = 0i16;
    // Subtract decimal digits from right to left, comparing directly with the shift.
    // A shared exponent prefix may be arbitrarily long, without overflowing a counter.
    for digit in larger.bytes().rev() {
        let mut digit = i16::from(digit - b'0')
            - smaller.next().map_or(0, |digit| i16::from(digit - b'0'))
            - borrow;
        borrow = i16::from(digit < 0);
        if digit < 0 {
            digit += 10;
        }
        if digit as u128 != expected % 10 {
            return false;
        }
        expected /= 10;
    }
    expected == 0 && borrow == 0
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
