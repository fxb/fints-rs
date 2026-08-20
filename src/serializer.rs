//! FinTS wire format serializer.
//!
//! Converts structured segment data back into the FinTS wire format:
//! - Segments terminated by `'`
//! - DEGs separated by `+`
//! - DEs separated by `:`
//! - Special characters escaped with `?`
//! - Binary data prefixed with `@len@`

use crate::error::FinTSError;
use crate::parser::{DataElement, DEG};

/// Escape a text string for the FinTS wire format.
/// Characters `+`, `:`, `'`, `@`, `?` are prefixed with `?`.
/// The output is ISO-8859-1 encoded bytes.
///
/// Returns an error if the text contains characters outside the ISO-8859-1 range
/// (code points > 255), since these cannot be represented in the FinTS wire format.
pub fn escape_text(text: &str) -> Result<Vec<u8>, FinTSError> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '+' | ':' | '\'' | '@' | '?' => {
                out.push(b'?');
                out.push(ch as u8);
            }
            _ => {
                if (ch as u32) <= 255 {
                    out.push(ch as u8);
                } else {
                    return Err(FinTSError::Serialize(format!(
                        "Character '{}' (U+{:04X}) is outside ISO-8859-1 range",
                        ch, ch as u32
                    )));
                }
            }
        }
    }
    Ok(out)
}

/// Serialize a binary data element: `@<len>@<bytes>`.
pub fn serialize_binary(data: &[u8]) -> Vec<u8> {
    let prefix = format!("@{}@", data.len());
    let mut out = Vec::with_capacity(prefix.len() + data.len());
    out.extend_from_slice(prefix.as_bytes());
    out.extend_from_slice(data);
    out
}

/// Serialize a single data element.
pub fn serialize_de(de: &DataElement) -> Result<Vec<u8>, FinTSError> {
    match de {
        DataElement::Empty => Ok(Vec::new()),
        DataElement::Text(s) => escape_text(s),
        DataElement::Binary(b) => Ok(serialize_binary(b)),
    }
}

/// Serialize a DEG (colon-separated data elements).
/// Trailing empty elements are stripped.
pub fn serialize_deg(deg: &DEG) -> Result<Vec<u8>, FinTSError> {
    // Find last non-empty element
    let last_non_empty = deg
        .0
        .iter()
        .rposition(|de| !de.is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);

    let trimmed = &deg.0[..last_non_empty];
    let mut out = Vec::new();
    for (i, de) in trimmed.iter().enumerate() {
        if i > 0 {
            out.push(b':');
        }
        out.extend(serialize_de(de)?);
    }
    Ok(out)
}

/// Serialize a complete segment (plus-separated DEGs, terminated by `'`).
/// Trailing empty DEGs are stripped.
pub fn serialize_segment(degs: &[DEG]) -> Result<Vec<u8>, FinTSError> {
    // Find last non-empty DEG
    let last_non_empty = degs
        .iter()
        .rposition(|deg| !deg.is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);

    let trimmed = &degs[..last_non_empty];
    let mut out = Vec::new();
    for (i, deg) in trimmed.iter().enumerate() {
        if i > 0 {
            out.push(b'+');
        }
        out.extend(serialize_deg(deg)?);
    }
    out.push(b'\'');
    Ok(out)
}

/// Serialize a list of segments into a complete FinTS message.
#[allow(dead_code)]
pub fn serialize_message(segments: &[Vec<DEG>]) -> Result<Vec<u8>, FinTSError> {
    let mut out = Vec::new();
    for seg_degs in segments {
        out.extend(serialize_segment(seg_degs)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::DataElement;

    #[test]
    fn test_escape_text_no_specials() {
        assert_eq!(escape_text("Hello").unwrap(), b"Hello");
    }

    #[test]
    fn test_escape_text_with_specials() {
        assert_eq!(escape_text("A+B:C'D@E?F").unwrap(), b"A?+B?:C?'D?@E??F");
    }

    #[test]
    fn test_escape_text_rejects_non_iso8859() {
        assert!(escape_text("Hello 🌍").is_err());
        assert!(escape_text("Ω").is_err());
    }

    #[test]
    fn test_escape_text_allows_iso8859_extended() {
        // German umlauts and other ISO-8859-1 chars should pass
        assert!(escape_text("Ä Ö Ü ä ö ü ß").is_ok());
        assert!(escape_text("café résumé").is_ok());
    }

    #[test]
    fn test_serialize_binary() {
        assert_eq!(serialize_binary(b"HI"), b"@2@HI");
    }

    #[test]
    fn test_serialize_deg_strips_trailing_empty() {
        let deg = DEG(vec![
            DataElement::Text("A".into()),
            DataElement::Text("B".into()),
            DataElement::Empty,
            DataElement::Empty,
        ]);
        assert_eq!(serialize_deg(&deg).unwrap(), b"A:B");
    }

    #[test]
    fn test_serialize_deg_preserves_middle_empty() {
        let deg = DEG(vec![
            DataElement::Text("A".into()),
            DataElement::Empty,
            DataElement::Text("C".into()),
        ]);
        assert_eq!(serialize_deg(&deg).unwrap(), b"A::C");
    }

    #[test]
    fn test_serialize_segment() {
        let degs = vec![
            DEG(vec![
                DataElement::Text("HNHBS".into()),
                DataElement::Text("5".into()),
                DataElement::Text("1".into()),
            ]),
            DEG(vec![DataElement::Text("2".into())]),
        ];
        assert_eq!(serialize_segment(&degs).unwrap(), b"HNHBS:5:1+2'");
    }

    #[test]
    fn test_round_trip() {
        let input = b"HNHBS:5:1+2'";
        let segments = crate::parser::parse_message(input).unwrap();
        let seg = &segments[0];
        let output = serialize_segment(&seg.degs).unwrap();
        assert_eq!(output, input.to_vec());
    }

    #[test]
    fn test_round_trip_with_escape() {
        let input = b"TEST:1:1+Hello?+World'";
        let segments = crate::parser::parse_message(input).unwrap();
        let seg = &segments[0];
        let output = serialize_segment(&seg.degs).unwrap();
        assert_eq!(output, input.to_vec());
    }
}
