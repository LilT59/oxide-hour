//! Small value helpers shared by the image/`.wnd` parsers.

/// Trimmed text with optional surrounding quotes removed.
pub fn trimmed(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix('"').unwrap_or(t);
    let t = t.strip_suffix('"').unwrap_or(t);
    t.trim().to_owned()
}

/// Leading integer (`sscanf("%d")`); a fractional part is truncated.
pub fn int(s: &str) -> i32 {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '+'))
        .unwrap_or(s.len());
    s[..end].parse().unwrap_or(0)
}

/// Every integer in `s`, in order (labels and punctuation skipped).
pub fn ints(s: &str) -> Vec<i32> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit()
            || ((c == b'-' || c == b'+') && i + 1 < b.len() && b[i + 1].is_ascii_digit())
        {
            let start = i;
            if c == b'-' || c == b'+' {
                i += 1;
            }
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            out.push(s[start..i].parse().unwrap_or(0));
        } else {
            i += 1;
        }
    }
    out
}

/// Whitespace/comma-separated words (INI list fields).
pub fn words(s: &str) -> Vec<String> {
    s.split([' ', '\t', ',', '=']).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// `parseBitString32`: `NONE`/`NULL`, `+`- or whitespace-separated names, optional `-name`.
pub fn bits(s: &str, names: &[&str]) -> u32 {
    let mut out = 0;
    for tok in s.split(|c: char| c.is_whitespace() || c == '+' || c == '=') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        if tok.eq_ignore_ascii_case("NONE") || tok.eq_ignore_ascii_case("NULL") {
            out = 0;
            continue;
        }
        if let Some(name) = tok.strip_prefix('-') {
            if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                out &= !(1 << i);
            }
            continue;
        }
        let name = tok.strip_prefix('+').unwrap_or(tok);
        if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
            out |= 1 << i;
        }
    }
    out
}
