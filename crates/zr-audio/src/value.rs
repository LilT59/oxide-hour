//! INI value parsing for the audio tables. These mirror the engine helpers the
//! field tables use (`scanReal`, `scanInt`, `scanPercentToReal`,
//! `parseSoundsList`, `parseBitString32`, `parseIndexList`,
//! `parseDurationUnsignedInt`) and are tolerant of trailing comments/`%`.

use crate::AudioPriority;

/// Longest leading numeric prefix (sign, digits, `.`, exponent), for `sscanf("%f")`.
fn leading_number(s: &str) -> &str {
    let s = s.trim_start();
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E')))
        .unwrap_or(s.len());
    &s[..end]
}

/// `scanReal`: `sscanf("%f")`.
pub fn parse_real(s: &str) -> f32 {
    let n = leading_number(s);
    if n.is_empty() || n == "+" || n == "-" {
        return 0.0;
    }
    n.parse().unwrap_or(0.0)
}

/// `scanInt`: leading integer (a fractional part is truncated).
pub fn parse_int(s: &str) -> i32 {
    parse_real(s).trunc() as i32
}

/// `scanPercentToReal`: `scanReal / 100`.
pub fn parse_percent(s: &str) -> f32 {
    parse_real(s) / 100.0
}

/// `parseBool` (`INI::parseBool`, `stricmp` against `yes`/`true`/`1`).
pub fn parse_bool(s: &str) -> bool {
    let t = s.trim();
    t.eq_ignore_ascii_case("yes") || t.eq_ignore_ascii_case("true") || t == "1"
}

/// `parseAsciiString`: trim surrounding whitespace and optional quotes.
pub fn parse_string(s: &str) -> String {
    let t = s.trim();
    let t = t.strip_prefix('"').unwrap_or(t);
    let t = t.strip_suffix('"').unwrap_or(t);
    t.trim().to_owned()
}

/// `parseSoundsList`: split on `" \t,="`.
pub fn parse_sounds_list(s: &str) -> Vec<String> {
    s.split([' ', '\t', ',', '=']).filter(|t| !t.is_empty()).map(str::to_owned).collect()
}

/// Tokens for the bit-string lists (`getNextToken` separators: space, tab, `=`).
fn bit_tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split([' ', '\t', '=']).filter(|t| !t.is_empty())
}

/// `parseBitString32`: `NONE`, plain names, or `+NAME`/`-NAME` ops.
pub fn parse_bits(s: &str, names: &[&str]) -> u32 {
    let index = |name: &str| names.iter().position(|n| n.eq_ignore_ascii_case(name));
    let mut bits = 0u32;
    let mut normal = false;
    let mut add_or_sub = false;
    for token in bit_tokens(s) {
        if token.eq_ignore_ascii_case("NONE") {
            bits = 0;
            break;
        }
        if let Some(name) = token.strip_prefix('+') {
            if let Some(i) = index(name) {
                bits |= 1 << i;
            }
            add_or_sub = true;
        } else if let Some(name) = token.strip_prefix('-') {
            if let Some(i) = index(name) {
                bits &= !(1 << i);
            }
            add_or_sub = true;
        } else {
            if !normal && !add_or_sub {
                bits = 0;
            }
            if let Some(i) = index(token) {
                bits |= 1 << i;
            }
            normal = true;
        }
    }
    bits
}

/// `parseIndexList` against `theAudioPriorityNames`.
pub fn parse_priority(s: &str) -> AudioPriority {
    let t = s.trim();
    for (name, p) in [
        ("LOWEST", AudioPriority::Lowest),
        ("LOW", AudioPriority::Low),
        ("NORMAL", AudioPriority::Normal),
        ("HIGH", AudioPriority::High),
        ("CRITICAL", AudioPriority::Critical),
    ] {
        if t.eq_ignore_ascii_case(name) {
            return p;
        }
    }
    AudioPriority::Lowest
}

/// `parseDelay`: `min max` in milliseconds.
pub fn parse_delay(s: &str) -> (i32, i32) {
    let mut it = bit_tokens(s);
    let min = it.next().map(parse_int).unwrap_or(0);
    let max = it.next().map(parse_int).unwrap_or(min);
    (min, max.max(min))
}

/// `parsePitchShift`: `min max` percent, stored as multipliers (`1 + p/100`).
pub fn parse_pitch_shift(s: &str) -> (f32, f32) {
    let mut it = bit_tokens(s);
    let min = it.next().map(parse_real).unwrap_or(0.0);
    let max = it.next().map(parse_real).unwrap_or(min);
    (1.0 + min / 100.0, 1.0 + max.max(min) / 100.0)
}

/// `parseDurationUnsignedInt`: `ceil(ms * 30 / 1000)` frames.
pub fn parse_duration_frames(s: &str) -> u32 {
    let frames = parse_real(s) * 30.0 / 1000.0;
    if frames <= 0.0 {
        0
    } else {
        frames.ceil() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percents_and_trailing_comments() {
        assert_eq!(parse_percent("80"), 0.8);
        assert_eq!(parse_percent("80%"), 0.8);
        assert_eq!(parse_percent("-10%"), -0.1);
        assert_eq!(parse_percent("33.3%"), 0.333);
        assert_eq!(parse_real("175.00"), 175.0);
    }

    #[test]
    fn delay_and_pitch() {
        assert_eq!(parse_delay("-10  10"), (-10, 10));
        assert_eq!(parse_delay("5"), (5, 5));
        assert_eq!(parse_pitch_shift("-10 10"), (0.9, 1.1));
        assert_eq!(parse_pitch_shift("-0 0"), (1.0, 1.0));
    }

    #[test]
    fn bit_strings_support_none_and_ops() {
        let names = &["UI", "WORLD", "LOOP", "RANDOM", "ALL"];
        assert_eq!(parse_bits("ui player", names), 1);
        assert_eq!(parse_bits("world loop random all", names), 2 | 4 | 8 | 16);
        assert_eq!(parse_bits("NONE", names), 0);
        assert_eq!(parse_bits("world +loop -world", names), 4);
    }

    #[test]
    fn sound_lists_split_on_comma_space_equal() {
        assert_eq!(parse_sounds_list("a, b c=d"), vec!["a", "b", "c", "d"]);
        assert_eq!(parse_duration_frames("1000"), 30);
        assert_eq!(parse_duration_frames("2000"), 60);
    }
}
