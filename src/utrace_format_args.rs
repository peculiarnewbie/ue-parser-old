//! Unreal Insights `FormatArgs` wire decode + printf-style rendering.
//!
//! Layout (from `FFormatArgsTrace` / `FFormatArgsHelper`):
//! `[u8 arg_count][u8 type_code * count][payload…]`
//! Type code: high 2 bits = category, low 6 bits = size in bytes.

use std::fmt::Write as _;

const CATEGORY_SHIFT: u8 = 6;
const SIZE_MASK: u8 = (1 << CATEGORY_SHIFT) - 1;
const CATEGORY_INTEGER: u8 = 1 << CATEGORY_SHIFT;
const CATEGORY_FLOAT: u8 = 2 << CATEGORY_SHIFT;
const CATEGORY_STRING: u8 = 3 << CATEGORY_SHIFT;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FormatArg {
    Integer { value: u64, size: u8 },
    Float(f64),
    String(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FormatArgsDecode {
    pub(crate) args: Vec<FormatArg>,
    pub(crate) malformed: bool,
}

/// Decode typed format-args from the Insights wire blob.
pub(crate) fn decode_format_args(bytes: &[u8]) -> FormatArgsDecode {
    if bytes.is_empty() {
        return FormatArgsDecode::default();
    }
    let count = bytes[0] as usize;
    if count == 0 {
        return FormatArgsDecode::default();
    }
    if bytes.len() < 1 + count {
        return FormatArgsDecode {
            args: Vec::new(),
            malformed: true,
        };
    }
    let type_codes = &bytes[1..1 + count];
    let mut payload = &bytes[1 + count..];
    let mut args = Vec::with_capacity(count);
    for &code in type_codes {
        let category = code & !SIZE_MASK;
        let size = code & SIZE_MASK;
        match category {
            CATEGORY_INTEGER => {
                let Some(value) = read_int_payload(payload, size) else {
                    return FormatArgsDecode {
                        args,
                        malformed: true,
                    };
                };
                payload = &payload[size as usize..];
                args.push(FormatArg::Integer { value, size });
            }
            CATEGORY_FLOAT => {
                let Some(value) = read_float_payload(payload, size) else {
                    return FormatArgsDecode {
                        args,
                        malformed: true,
                    };
                };
                payload = &payload[size as usize..];
                args.push(FormatArg::Float(value));
            }
            CATEGORY_STRING => {
                let Some((text, consumed)) = read_string_payload(payload, size) else {
                    return FormatArgsDecode {
                        args,
                        malformed: true,
                    };
                };
                payload = &payload[consumed..];
                args.push(FormatArg::String(text));
            }
            _ => {
                return FormatArgsDecode {
                    args,
                    malformed: true,
                };
            }
        }
    }
    FormatArgsDecode {
        args,
        malformed: false,
    }
}

/// Render `format` with a FormatArgs blob. Falls back to heuristic string
/// extraction when the blob is not a valid typed stream (legacy log samples).
pub(crate) fn render_format_message(format: &str, format_args_bytes: &[u8]) -> Option<String> {
    if format_args_bytes.is_empty() {
        return None;
    }
    let decoded = decode_format_args(format_args_bytes);
    if !decoded.args.is_empty() && !decoded.malformed {
        return Some(render_with_args(format, &decoded.args));
    }
    // Heuristic fallback used by older log-sample paths: pull embedded strings.
    let strings = heuristic_strings(format_args_bytes);
    if strings.is_empty() {
        if decoded.args.is_empty() && !decoded.malformed {
            // Zero-arg blob: format string alone.
            return Some(format.to_owned());
        }
        return None;
    }
    render_percent_s(format, &strings)
}

pub(crate) fn format_arg_display_strings(args: &[FormatArg]) -> Vec<String> {
    args.iter().map(format_arg_to_string).collect()
}

fn render_with_args(format: &str, args: &[FormatArg]) -> String {
    let mut out = String::new();
    let mut arg_index = 0usize;
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '%' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if i + 1 < chars.len() && chars[i + 1] == '%' {
            out.push('%');
            i += 2;
            continue;
        }
        let Some((spec_end, expected, extra_ints, nothing_printed)) =
            parse_format_specifier(&chars, i)
        else {
            out.push('%');
            i += 1;
            continue;
        };
        let specifier: String = chars[i..spec_end].iter().collect();
        if nothing_printed {
            arg_index = arg_index.saturating_add(extra_ints + 1);
            i = spec_end;
            continue;
        }
        // Consume extra integer args introduced by `*` width/precision.
        for _ in 0..extra_ints {
            arg_index = arg_index.saturating_add(1);
        }
        let Some(arg) = args.get(arg_index) else {
            out.push_str(&specifier);
            i = spec_end;
            continue;
        };
        arg_index += 1;
        out.push_str(&format_one(&specifier, expected, arg));
        i = spec_end;
    }
    out
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedKind {
    Integer,
    Float,
    String,
}

fn parse_format_specifier(
    chars: &[char],
    start: usize,
) -> Option<(usize, ExpectedKind, usize, bool)> {
    // start points at '%'
    let mut i = start + 1;
    if i >= chars.len() {
        return None;
    }
    let mut extra_ints = 0usize;
    // flags
    while i < chars.len() && matches!(chars[i], '-' | '+' | ' ' | '#' | '0') {
        i += 1;
    }
    // width
    if i < chars.len() && chars[i] == '*' {
        extra_ints += 1;
        i += 1;
    } else {
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
    }
    // precision
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        if i < chars.len() && chars[i] == '*' {
            extra_ints += 1;
            i += 1;
        } else {
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    // length
    while i < chars.len() && matches!(chars[i], 'h' | 'l' | 'j' | 'z' | 't' | 'L') {
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let spec = chars[i];
    let (kind, nothing) = match spec {
        'd' | 'i' | 'u' | 'o' | 'x' | 'X' | 'c' | 'p' => (ExpectedKind::Integer, false),
        'n' => (ExpectedKind::Integer, true),
        'f' | 'F' | 'e' | 'E' | 'g' | 'G' | 'a' | 'A' => (ExpectedKind::Float, false),
        's' | 'S' => (ExpectedKind::String, false),
        _ => return None,
    };
    Some((i + 1, kind, extra_ints, nothing))
}

fn format_one(specifier: &str, expected: ExpectedKind, arg: &FormatArg) -> String {
    match (expected, arg) {
        (ExpectedKind::String, FormatArg::String(text)) => text.clone(),
        (ExpectedKind::Integer, FormatArg::Integer { value, size }) => {
            format_integer(specifier, *value, *size)
        }
        (ExpectedKind::Float, FormatArg::Float(value)) => format_float(specifier, *value),
        // Type mismatches: still surface a usable representation.
        (_, other) => format_arg_to_string(other),
    }
}

fn format_integer(specifier: &str, value: u64, size: u8) -> String {
    let last = specifier.chars().last().unwrap_or('d');
    match last {
        'x' => format!("{value:x}"),
        'X' => format!("{value:X}"),
        'o' => format!("{value:o}"),
        'p' => format!("0x{value:x}"),
        'c' => char::from_u32(value as u32)
            .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
            .map(|ch| ch.to_string())
            .unwrap_or_else(|| format!("{value}")),
        'u' => format!("{value}"),
        // signed
        _ => {
            let signed = match size {
                1 => value as i8 as i64,
                2 => value as i16 as i64,
                4 => value as i32 as i64,
                _ => value as i64,
            };
            format!("{signed}")
        }
    }
}

fn format_float(specifier: &str, value: f64) -> String {
    let last = specifier.chars().last().unwrap_or('f');
    match last {
        'e' | 'E' => format!("{value:e}"),
        'g' | 'G' => format!("{value}"),
        _ => {
            let mut out = String::new();
            let _ = write!(&mut out, "{value}");
            out
        }
    }
}

fn format_arg_to_string(arg: &FormatArg) -> String {
    match arg {
        FormatArg::Integer { value, size } => format_integer("%d", *value, *size),
        FormatArg::Float(value) => format_float("%g", *value),
        FormatArg::String(text) => text.clone(),
    }
}

fn render_percent_s(format: &str, args: &[String]) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let mut rendered = format.to_owned();
    for arg in args {
        if let Some(index) = rendered.find("%s") {
            rendered.replace_range(index..index + 2, arg);
        } else {
            return None;
        }
    }
    Some(rendered)
}

fn heuristic_strings(bytes: &[u8]) -> Vec<String> {
    let wide = extract_utf16_strings(bytes);
    if !wide.is_empty() {
        return wide;
    }
    extract_ascii_strings(bytes)
}

fn read_int_payload(payload: &[u8], size: u8) -> Option<u64> {
    let size = size as usize;
    if size == 0 || size > 8 || payload.len() < size {
        return None;
    }
    let mut buf = [0_u8; 8];
    buf[..size].copy_from_slice(&payload[..size]);
    Some(u64::from_le_bytes(buf))
}

fn read_float_payload(payload: &[u8], size: u8) -> Option<f64> {
    match size {
        4 if payload.len() >= 4 => {
            let bits = u32::from_le_bytes(payload[..4].try_into().ok()?);
            Some(f32::from_bits(bits) as f64)
        }
        8 if payload.len() >= 8 => {
            let bits = u64::from_le_bytes(payload[..8].try_into().ok()?);
            Some(f64::from_bits(bits))
        }
        _ => None,
    }
}

fn read_string_payload(payload: &[u8], size: u8) -> Option<(String, usize)> {
    match size {
        1 => {
            let end = payload
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(payload.len());
            let text = String::from_utf8_lossy(&payload[..end]).into_owned();
            let consumed = if end < payload.len() { end + 1 } else { end };
            Some((text, consumed))
        }
        2 => {
            if payload.len() < 2 {
                return None;
            }
            let mut words = Vec::new();
            let mut offset = 0;
            while offset + 2 <= payload.len() {
                let word = u16::from_le_bytes([payload[offset], payload[offset + 1]]);
                offset += 2;
                if word == 0 {
                    break;
                }
                words.push(word);
            }
            let text = String::from_utf16_lossy(&words);
            Some((text, offset))
        }
        4 => {
            // UCS-4 / UTF-32 style terminator walk (rare on Windows traces).
            if payload.len() < 4 {
                return None;
            }
            let mut chars = Vec::new();
            let mut offset = 0;
            while offset + 4 <= payload.len() {
                let unit = u32::from_le_bytes(payload[offset..offset + 4].try_into().ok()?);
                offset += 4;
                if unit == 0 {
                    break;
                }
                if let Some(ch) = char::from_u32(unit) {
                    chars.push(ch);
                }
            }
            Some((chars.into_iter().collect(), offset))
        }
        _ => None,
    }
}

fn extract_utf16_strings(bytes: &[u8]) -> Vec<String> {
    if bytes.len() < 4 || bytes.len() % 2 != 0 {
        return Vec::new();
    }
    let mut strings = Vec::new();
    let mut current = Vec::new();
    for chunk in bytes.chunks_exact(2) {
        let word = u16::from_le_bytes([chunk[0], chunk[1]]);
        if word == 0 {
            if !current.is_empty() {
                if let Ok(text) = String::from_utf16(&current) {
                    if !text.is_empty() {
                        strings.push(text);
                    }
                }
                current.clear();
            }
        } else {
            current.push(word);
        }
    }
    strings
}

fn extract_ascii_strings(bytes: &[u8]) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = Vec::new();
    for &byte in bytes {
        if byte == 0 {
            if current.len() >= 2
                && current
                    .iter()
                    .all(|b: &u8| b.is_ascii_graphic() || *b == b' ')
            {
                strings.push(String::from_utf8_lossy(&current).into_owned());
            }
            current.clear();
        } else {
            current.push(byte);
        }
    }
    strings
}

/// Encode helpers for unit tests / synthetic fixtures.
#[cfg(test)]
pub(crate) mod encode {
    use super::*;

    pub(crate) fn encode_args(parts: &[EncodedPart<'_>]) -> Vec<u8> {
        let count = u8::try_from(parts.len()).expect("arg count fits u8");
        let mut type_codes = Vec::with_capacity(parts.len());
        let mut payload = Vec::new();
        for part in parts {
            match part {
                EncodedPart::Int(value, size) => {
                    type_codes.push(CATEGORY_INTEGER | size);
                    let bytes = value.to_le_bytes();
                    payload.extend_from_slice(&bytes[..*size as usize]);
                }
                EncodedPart::Float32(value) => {
                    type_codes.push(CATEGORY_FLOAT | 4);
                    payload.extend_from_slice(&value.to_bits().to_le_bytes());
                }
                EncodedPart::Float64(value) => {
                    type_codes.push(CATEGORY_FLOAT | 8);
                    payload.extend_from_slice(&value.to_bits().to_le_bytes());
                }
                EncodedPart::Ansi(text) => {
                    type_codes.push(CATEGORY_STRING | 1);
                    payload.extend_from_slice(text.as_bytes());
                    payload.push(0);
                }
                EncodedPart::Wide(text) => {
                    type_codes.push(CATEGORY_STRING | 2);
                    for unit in text.encode_utf16() {
                        payload.extend_from_slice(&unit.to_le_bytes());
                    }
                    payload.extend_from_slice(&0_u16.to_le_bytes());
                }
            }
        }
        let mut out = Vec::with_capacity(1 + type_codes.len() + payload.len());
        out.push(count);
        out.extend_from_slice(&type_codes);
        out.extend_from_slice(&payload);
        out
    }

    pub(crate) enum EncodedPart<'a> {
        Int(u64, u8),
        Float32(f32),
        Float64(f64),
        Ansi(&'a str),
        Wide(&'a str),
    }
}

#[cfg(test)]
mod tests {
    use super::encode::{EncodedPart, encode_args};
    use super::*;

    #[test]
    fn renders_wide_string_and_integer_bookmark() {
        let bytes = encode_args(&[EncodedPart::Wide("MapA"), EncodedPart::Int(42, 4)]);
        let message = render_format_message("Loading %s frame %d", &bytes).unwrap();
        assert_eq!(message, "Loading MapA frame 42");
        let decoded = decode_format_args(&bytes);
        assert_eq!(decoded.args.len(), 2);
        assert!(!decoded.malformed);
    }

    #[test]
    fn renders_hex_and_float() {
        let bytes = encode_args(&[EncodedPart::Int(0xdead, 4), EncodedPart::Float32(1.5)]);
        let message = render_format_message("ptr=%x val=%f", &bytes).unwrap();
        assert_eq!(message, "ptr=dead val=1.5");
        let bytes64 = encode_args(&[EncodedPart::Float64(2.25)]);
        assert_eq!(
            render_format_message("%f", &bytes64).as_deref(),
            Some("2.25")
        );
    }

    #[test]
    fn escapes_percent_percent() {
        let bytes = encode_args(&[EncodedPart::Ansi("ok")]);
        let message = render_format_message("done %% %s", &bytes).unwrap();
        assert_eq!(message, "done % ok");
    }

    #[test]
    fn falls_back_to_heuristic_percent_s() {
        // Not a valid typed stream: raw UTF-16 string payload.
        let mut bytes = Vec::new();
        for unit in "Hello".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        let message = render_format_message("msg=%s", &bytes).unwrap();
        assert_eq!(message, "msg=Hello");
    }

    #[test]
    fn rejects_truncated_typed_stream_without_panic() {
        let decoded = decode_format_args(&[1, CATEGORY_INTEGER | 8, 1, 2]);
        assert!(decoded.malformed);
        assert!(decoded.args.is_empty());
    }
}
