//! Base64, because the bridge is a line of JSON and mail is bytes.
//!
//! Anything binary — a raw message, an attachment — crosses as `{"base64": "..."}`, so this is the
//! one thing between the wire and a file. Thirty lines rather than a dependency: the alphabet is
//! fixed, the padding is fixed, and the only decision is what to do with input that is not base64
//! at all, which is to refuse it rather than to guess.

/// Decodes standard base64. Whitespace anywhere is ignored, since the encoders that wrap at 76
/// columns are the ones this meets. Returns `None` for anything else.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits = 0;
    let mut padding = 0;
    for character in text.chars() {
        if character.is_ascii_whitespace() {
            continue;
        }
        if character == '=' {
            padding += 1;
            continue;
        }
        // Padding is the end of the data; anything after it is not base64.
        if padding > 0 {
            return None;
        }
        let value = match character {
            'A'..='Z' => character as u32 - 'A' as u32,
            'a'..='z' => character as u32 - 'a' as u32 + 26,
            '0'..='9' => character as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    // Whatever is left over has to be zero: a trailing group that encodes bits nobody asked for is
    // a corrupt payload, not a short one.
    if bits >= 6 || (accumulator & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

/// Encodes standard base64, padded.
pub fn encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let packed = (u32::from(block[0]) << 16) | (u32::from(block[1]) << 8) | u32::from(block[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(packed >> (18 - 6 * index)) as usize & 0x3f] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_round_trips_every_length_of_tail() {
        for text in ["", "a", "ab", "abc", "abcd", "hello from greenmail", "\u{1f600} unicode"] {
            let encoded = encode(text.as_bytes());
            assert_eq!(decode(&encoded).as_deref(), Some(text.as_bytes()), "{text:?} -> {encoded}");
        }
    }

    #[test]
    fn it_matches_what_everything_else_produces() {
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(encode(b"Ma"), "TWE=");
        assert_eq!(encode(b"M"), "TQ==");
        assert_eq!(decode("TWFu").as_deref(), Some(&b"Man"[..]));
        assert_eq!(decode("TQ==").as_deref(), Some(&b"M"[..]));
    }

    #[test]
    fn a_line_wrapped_payload_decodes_and_a_corrupt_one_does_not() {
        assert_eq!(decode("TWFu\r\nTWFu").as_deref(), Some(&b"ManMan"[..]));
        assert_eq!(decode("TWF!"), None, "not the alphabet");
        assert_eq!(decode("TQ==TQ=="), None, "data after the padding");
        assert_eq!(decode("TQ"), Some(b"M".to_vec()), "padding is optional");
    }
}
