//! Text helpers shared by the lyric, pack and library code: UTF-8 checks, TIS-620 (Thai) decoding,
//! and terminal display width.

/// Which byte encoding a lyric file uses. Thai karaoke files are usually TIS-620.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Tis620,
    Latin1,
}

/// The UTF-8 check the C++ build uses. It is deliberately looser than `str::from_utf8` (it accepts
/// overlong forms and surrogates), so the same files are read the same way by both builds.
pub fn is_valid_utf8(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let extra = if c < 0x80 {
            0
        } else if c >> 5 == 0x6 {
            1
        } else if c >> 4 == 0xE {
            2
        } else if c >> 3 == 0x1E {
            3
        } else {
            return false;
        };
        if i + extra >= bytes.len() {
            return false;
        }
        if bytes[i + 1..=i + extra].iter().any(|&b| b & 0xC0 != 0x80) {
            return false;
        }
        i += extra + 1;
    }
    true
}

fn push_utf8(out: &mut Vec<u8>, cp: u32) {
    if cp < 0x80 {
        out.push(cp as u8);
    } else if cp < 0x800 {
        out.push((0xC0 | (cp >> 6)) as u8);
        out.push((0x80 | (cp & 0x3F)) as u8);
    } else {
        out.push((0xE0 | (cp >> 12)) as u8);
        out.push((0x80 | ((cp >> 6) & 0x3F)) as u8);
        out.push((0x80 | (cp & 0x3F)) as u8);
    }
}

/// UTF-8 bytes for one byte of text in `enc`. Bytes TIS-620 leaves unassigned produce nothing.
pub fn decode_byte(c: u8, enc: Encoding) -> Vec<u8> {
    let mut out = Vec::with_capacity(3);
    if c < 0x80 {
        out.push(c);
    } else if enc == Encoding::Tis620 && (0xA1..=0xFB).contains(&c) {
        push_utf8(&mut out, 0x0E01 + u32::from(c - 0xA1));
    } else if enc == Encoding::Latin1 {
        push_utf8(&mut out, u32::from(c));
    } else if enc == Encoding::Utf8 {
        out.push(c);
    }
    out
}

/// Converts karaoke text to UTF-8 bytes. UTF-8 input is returned unchanged.
pub fn decode_bytes(bytes: &[u8], enc: Encoding) -> Vec<u8> {
    if enc == Encoding::Utf8 {
        return bytes.to_vec();
    }
    bytes.iter().flat_map(|&c| decode_byte(c, enc)).collect()
}

/// Converts karaoke text to a `String`. Invalid sequences (only possible if a file is mixed) become U+FFFD.
pub fn decode(bytes: &[u8], enc: Encoding) -> String {
    String::from_utf8_lossy(&decode_bytes(bytes, enc)).into_owned()
}

/// Decodes text that is UTF-8 as-is, and reads anything else as TIS-620.
pub fn thai_to_utf8(bytes: &[u8]) -> String {
    decode(bytes, if is_valid_utf8(bytes) { Encoding::Utf8 } else { Encoding::Tis620 })
}

/// Number of terminal columns a string occupies: Thai above/below vowels and tone marks take none.
pub fn display_width(s: &str) -> usize {
    s.chars()
        .filter(|&c| {
            let cp = c as u32;
            let thai_mark = cp == 0x0E31 || (0x0E34..=0x0E3A).contains(&cp) || (0x0E47..=0x0E4E).contains(&cp);
            let combining = (0x0300..=0x036F).contains(&cp);
            !thai_mark && !combining
        })
        .count()
}

/// Trims spaces, tabs and line breaks from both ends (the set the C++ code trims).
pub fn trim(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

/// NCN titles usually repeat the key, e.g. "คิดถึง ( Em )". Returns the title without that suffix.
pub fn strip_key_suffix(title: &str, key: &str) -> String {
    let t = trim(title);
    let k = trim(key);
    if k.is_empty() || t.is_empty() || !t.ends_with(')') {
        return t.to_string();
    }
    let Some(open) = t.rfind('(') else {
        return t.to_string();
    };
    if open == 0 {
        return t.to_string();
    }
    // Both brackets are ASCII, so the slices fall on character boundaries.
    if trim(&t[open + 1..t.len() - 1]) != k {
        return t.to_string();
    }
    trim(&t[..open]).to_string()
}

/// True if `needle` occurs in `hay` (byte search, so it works on text with mixed encodings).
pub fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_check_matches_the_cpp_rules() {
        assert!(is_valid_utf8("คาราโอเกะ".as_bytes()));
        assert!(is_valid_utf8(b""));
        assert!(!is_valid_utf8(&[0xE0, 0x80])); // truncated 3-byte sequence
        assert!(!is_valid_utf8(&[0xC3, 0x28])); // bad continuation byte
        assert!(!is_valid_utf8(&[0xFF]));
    }

    #[test]
    fn tis620_maps_thai_letters_and_drops_unassigned_bytes() {
        // TIS-620 0xA1 is "ก" (U+0E01); 0x80 is unassigned and is dropped.
        assert_eq!(decode(&[0xA1, 0x80, b'a'], Encoding::Tis620), "กa");
        assert_eq!(decode(&[0xE9], Encoding::Latin1), "é");
    }

    #[test]
    fn display_width_skips_thai_marks() {
        assert_eq!(display_width("กิ"), 1); // ิ (U+0E34) takes no column
        assert_eq!(display_width("abc"), 3);
    }

    #[test]
    fn key_suffix_is_removed_only_when_it_matches() {
        assert_eq!(strip_key_suffix("คิดถึง ( Em )", "Em"), "คิดถึง");
        assert_eq!(strip_key_suffix("คิดถึง ( Am )", "Em"), "คิดถึง ( Am )");
        assert_eq!(strip_key_suffix("(Em)", "Em"), "(Em)");
    }
}
