//! Local subset shared with the reference CLI: path.eq/contains/in, with array traversal.
use heapless::{String, Vec};
use unifi::Json;
pub struct Expression<'a> {
    path: Vec<&'a str, 32>,
    op: &'a str,
    values: Vec<String<512>, 32>,
}
impl<'a> Expression<'a> {
    pub fn parse(text: &'a str) -> Result<Self, &'static str> {
        let (head, tail) = text
            .trim()
            .split_once('(')
            .ok_or("filter must be path.eq('value'), contains or in")?;
        let tail = tail
            .strip_suffix(')')
            .ok_or("filter needs a closing parenthesis")?;
        let (path, op) = head.rsplit_once('.').ok_or("filter needs a field path")?;
        if !["eq", "contains", "in"].contains(&op) {
            return Err("local filters support eq, contains and in");
        }
        let mut fields = Vec::new();
        for field in path.split('.').map(str::trim) {
            if field.is_empty() {
                return Err("empty filter path component");
            }
            fields
                .push(field)
                .map_err(|_| "filter path exceeds capacity")?;
        }
        let mut values = Vec::new();
        let mut chars = tail.chars().peekable();
        loop {
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            let Some(quote) = chars.next() else {
                break;
            };
            if quote != '\'' && quote != '"' {
                return Err("filter values must be quoted");
            }
            let mut value = String::new();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == quote {
                    closed = true;
                    break;
                }
                let c = if c == '\\' {
                    chars.next().ok_or("unfinished filter escape")?
                } else {
                    c
                };
                value
                    .push(c.to_ascii_lowercase())
                    .map_err(|_| "filter value exceeds capacity")?;
            }
            if !closed {
                return Err("unfinished quoted filter value");
            }
            values.push(value).map_err(|_| "too many filter values")?;
            while chars.peek().is_some_and(|c| c.is_whitespace()) {
                chars.next();
            }
            match chars.next() {
                None => break,
                Some(',') => {}
                _ => return Err("filter values must be comma separated"),
            }
        }
        if values.is_empty() || op != "in" && values.len() != 1 {
            return Err("filter has the wrong number of values");
        }
        Ok(Self {
            path: fields,
            op,
            values,
        })
    }
    fn scalar(&self, value: Json<'_>) -> bool {
        if let Ok(items) = value.items() {
            return items.into_iter().any(|v| self.scalar(v));
        }
        if value.fields().is_ok() {
            return false;
        }
        self.values.iter().any(|needle| {
            fn matches(
                chars: impl Iterator<Item = char> + Clone,
                needle: &str,
                contains: bool,
            ) -> bool {
                if !contains {
                    return chars.eq(needle.chars());
                }
                let mut chars = chars;
                let count = needle.chars().count();
                loop {
                    if chars.clone().take(count).eq(needle.chars()) {
                        return true;
                    }
                    if chars.next().is_none() {
                        return false;
                    }
                }
            }
            if let Ok(text) = value.string() {
                matches(
                    text.chars().map(|c| c.to_ascii_lowercase()),
                    needle,
                    self.op == "contains",
                )
            } else {
                matches(value.raw().chars(), needle, self.op == "contains")
            }
        })
    }

    fn path(&self, value: Json<'_>, depth: usize) -> bool {
        if depth == self.path.len() {
            return self.scalar(value);
        }
        if let Ok(items) = value.items() {
            return items.into_iter().any(|v| self.path(v, depth));
        }
        value
            .get(self.path[depth])
            .ok()
            .flatten()
            .is_some_and(|v| self.path(v, depth + 1))
    }
    pub fn matches(&self, value: Json<'_>) -> bool {
        self.path(value, 0)
    }
}
