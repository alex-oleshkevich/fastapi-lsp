use tower_lsp_server::ls_types::Position;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16,
}

impl Encoding {
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        if s == "utf-8" { Encoding::Utf8 } else { Encoding::Utf16 }
    }
}

/// Convert a byte offset in `src` to an LSP Position using the negotiated encoding.
pub fn offset_to_position(src: &[u8], offset: usize, enc: Encoding) -> Position {
    let slice = &src[..offset.min(src.len())];
    let mut line = 0u32;
    let mut last_newline = 0;

    for (i, &b) in slice.iter().enumerate() {
        if b == b'\n' {
            line += 1;
            last_newline = i + 1;
        }
    }

    let after_newline = &slice[last_newline..];
    let character = match enc {
        Encoding::Utf8 => after_newline.len() as u32,
        Encoding::Utf16 => {
            let s = std::str::from_utf8(after_newline).unwrap_or("");
            s.encode_utf16().count() as u32
        }
    };

    Position::new(line, character)
}

/// Convert an LSP Position to a byte offset in `src` using the negotiated encoding.
pub fn position_to_offset(src: &[u8], pos: Position, enc: Encoding) -> usize {
    let mut line = 0u32;
    let mut line_start = 0;

    for (i, &b) in src.iter().enumerate() {
        if line == pos.line {
            let rest = &src[line_start..];
            return line_start + char_to_byte(rest, pos.character, enc);
        }
        if b == b'\n' {
            line += 1;
            line_start = i + 1;
        }
    }

    // pos.line >= number of lines: clamp to end
    if line == pos.line {
        let rest = &src[line_start..];
        return line_start + char_to_byte(rest, pos.character, enc);
    }
    src.len()
}

fn char_to_byte(line_bytes: &[u8], character: u32, enc: Encoding) -> usize {
    match enc {
        Encoding::Utf8 => {
            let Ok(s) = std::str::from_utf8(line_bytes) else {
                // Invalid UTF-8 (shouldn't happen for Rust Strings): clamp to byte length
                return (character as usize).min(line_bytes.len());
            };
            // Snap backward to codepoint start — backward preserves the codepoint the
            // client pointed into; forward would silently skip it
            let mut i = (character as usize).min(s.len());
            while i > 0 && !s.is_char_boundary(i) {
                i -= 1;
            }
            i
        }
        Encoding::Utf16 => {
            let s = std::str::from_utf8(line_bytes).unwrap_or("");
            let mut utf16 = 0u32;
            for (i, c) in s.char_indices() {
                if utf16 >= character {
                    return i;
                }
                utf16 += c.len_utf16() as u32;
            }
            s.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_roundtrip() {
        let src = b"hello\nworld\n";
        let pos = Position::new(1, 3);
        let off = position_to_offset(src, pos, Encoding::Utf8);
        assert_eq!(off, 9); // 'l' in "world"
        assert_eq!(offset_to_position(src, off, Encoding::Utf8), pos);
    }

    #[test]
    fn utf16_surrogate_pair() {
        // U+1F600 (emoji) encodes as 2 UTF-16 code units
        let src = "a\u{1F600}b".as_bytes();
        // In UTF-16 encoding: 'a'=0, emoji=1+2=3, 'b'=3
        let pos_b_utf16 = Position::new(0, 3);
        let off = position_to_offset(src, pos_b_utf16, Encoding::Utf16);
        // "a\u{1F600}" = 1 + 4 = 5 bytes, so 'b' is at byte 5
        assert_eq!(off, 5);
    }

    #[test]
    fn utf8_emoji() {
        // U+1F600 = 4 bytes in UTF-8
        let src = "a\u{1F600}b".as_bytes();
        // UTF-8 character position: 'b' is at char offset 2 (0-indexed)
        // but Position.character is byte offset in UTF-8 mode
        let pos_b = Position::new(0, 5); // 'a'=1 byte, emoji=4 bytes, total=5
        let off = position_to_offset(src, pos_b, Encoding::Utf8);
        assert_eq!(off, 5);
    }

    #[test]
    fn utf8_mid_codepoint_snaps_to_boundary() {
        // "a\u{1F600}b": byte layout = [0x61, 0xF0, 0x9F, 0x98, 0x80, 0x62]
        // Bytes 2, 3, 4 are continuation bytes inside the emoji.
        // A cursor at byte 2, 3, or 4 should snap backward to byte 1 (start of emoji).
        let src = "a\u{1F600}b".as_bytes();
        for mid in 2u32..=4u32 {
            let off = position_to_offset(src, Position::new(0, mid), Encoding::Utf8);
            assert_eq!(off, 1, "byte {mid} should snap to emoji start (byte 1)");
            // Verify the returned offset is actually a char boundary
            let text = std::str::from_utf8(src).unwrap();
            assert!(text.is_char_boundary(off), "offset {off} must be a char boundary");
        }
    }

    #[test]
    fn utf8_mid_codepoint_at_byte_zero_snaps_to_zero() {
        // Emoji at byte 0 — the while loop's i>0 guard must stop at 0, not underflow
        let src = "\u{1F600}x".as_bytes();
        for mid in 1u32..=3u32 {
            let off = position_to_offset(src, Position::new(0, mid), Encoding::Utf8);
            assert_eq!(off, 0, "byte {mid} inside emoji-at-0 should snap to 0");
        }
    }

    #[test]
    fn inverted_range_does_not_panic() {
        // After sorting start/end in server.rs, position_to_offset must return
        // values that, when min/max-ed, produce a valid replace_range call.
        let src = "hello world".as_bytes();
        let start = position_to_offset(src, Position::new(0, 8), Encoding::Utf8);
        let end = position_to_offset(src, Position::new(0, 3), Encoding::Utf8);
        // Simulate the guard added in server.rs
        let (s, e) = (start.min(end), start.max(end));
        let mut text = "hello world".to_string();
        text.replace_range(s..e, "X"); // must not panic
        assert_eq!(text, "helXrld"); // replaced "lo wo"
    }
}
