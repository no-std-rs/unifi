//! Strip JSONC comments and trailing commas without changing string contents.
pub fn parse<'a>(input: &[u8], output: &'a mut [u8]) -> Result<unifi::Json<'a>, &'static str> {
    if input.len() > output.len() {
        return Err("JSONC file exceeds capacity");
    }
    let mut i = if input.starts_with(&[0xef, 0xbb, 0xbf]) {
        3
    } else {
        0
    };
    let mut used = 0;
    let mut quoted = false;
    let mut escaped = false;
    while i < input.len() {
        let b = input[i];
        if !quoted && input[i..].starts_with(b"//") {
            while i < input.len() && input[i] != b'\n' {
                i += 1;
            }
            output[used] = b' ';
            used += 1;
            continue;
        }
        if !quoted && input[i..].starts_with(b"/*") {
            i += 2;
            while i + 1 < input.len() && !input[i..].starts_with(b"*/") {
                i += 1;
            }
            if i + 1 >= input.len() {
                return Err("unterminated JSONC comment");
            }
            i += 2;
            output[used] = b' ';
            used += 1;
            continue;
        }
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
        }
        output[used] = b;
        used += 1;
        i += 1;
    }
    quoted = false;
    escaped = false;
    for i in 0..used {
        let b = output[i];
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
        } else if b == b','
            && output[i + 1..used]
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| b"]}".contains(b))
        {
            output[i] = b' ';
        }
    }
    unifi::Json::parse(&output[..used]).map_err(|_| "invalid JSONC document")
}
