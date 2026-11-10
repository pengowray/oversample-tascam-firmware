//! Reading metadata back: where a `guan` chunk or a `bext` chunk's CodingHistory is in a file's
//! bytes, and the `key: value` fields of GUANO text.

use core::ops::Range;

/// Offset in a `bext` chunk's body of CodingHistory, the free text after the fixed fields.
const BEXT_CODING_HISTORY: usize = 602;

/// The body of the first chunk called `id` in `bytes`, walking from a chunk header at `at`, as
/// far as `bytes` holds it: a body that runs past the end of `bytes` is cut there. None if no
/// chunk called `id` starts in `bytes`, or if a `data` chunk comes first (unless `id` is `data`).
pub fn find_chunk(bytes: &[u8], mut at: usize, id: &[u8; 4]) -> Option<Range<usize>> {
    loop {
        let this = bytes.get(at..at + 4)?;
        let size = u32::from_le_bytes(bytes.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let body = at + 8;
        if this == id {
            return Some(body..body.saturating_add(size).min(bytes.len()));
        }
        if this == b"data" {
            return None;
        }
        // Chunks start on even offsets.
        at = body.checked_add(size)?.checked_add(size & 1)?;
    }
}

/// The CodingHistory text of the `bext` chunk in a WAV file's first bytes, up to its first NUL,
/// or None if there is no `bext` chunk before the audio or its CodingHistory is empty.
pub fn coding_history(header: &[u8]) -> Option<&[u8]> {
    if header.get(0..4)? != b"RIFF" || header.get(8..12)? != b"WAVE" {
        return None;
    }
    let body = find_chunk(header, 12, b"bext")?;
    let text = header.get(body.start + BEXT_CODING_HISTORY..body.end)?;
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    let text = text[..end].trim_ascii();
    (!text.is_empty()).then_some(text)
}

/// The text of a `guan` chunk before the audio, in a WAV file's first bytes.
pub fn guano_in_header(header: &[u8]) -> Option<Range<usize>> {
    if header.get(0..4)? != b"RIFF" || header.get(8..12)? != b"WAVE" {
        return None;
    }
    find_chunk(header, 12, b"guan")
}

/// The fields of GUANO text, as (key, value) with spaces trimmed, in order. Lines without `:`
/// and lines that are not UTF-8 are skipped. The key is everything before the first `:`, so a
/// namespaced key such as `TASCAM|Gain` keeps its namespace.
pub fn fields(text: &[u8]) -> impl Iterator<Item = (&str, &str)> {
    text.split(|&b| b == b'\n').filter_map(|line| {
        let line = core::str::from_utf8(line).ok()?.trim();
        let (key, value) = line.split_once(':')?;
        let key = key.trim();
        (!key.is_empty()).then(|| (key, value.trim()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChunkWriter;
    use crate::header::tests::x8_header;
    use std::vec::Vec;

    const XRI: &[u8] = b"T=XRI 008a;XRI VER=0002;MAKER=TASCAM;MODEL=X8;FW VER=2.03;SOURCE=3:EXT MIC,4:EXT MIC;\
INPUT GAIN=3:LOW,4:LOW;LEVEL=3:20,4:20;AUTO_LEVEL=3:OFF,4:OFF;,";

    #[test]
    fn coding_history_of_an_x8_take() {
        let mut bytes = x8_header(192_000 * 8, 2, 32, b"2026-03-15", b"21:04:00");
        let at = 12 + 8 + BEXT_CODING_HISTORY;
        bytes[at..at + XRI.len()].copy_from_slice(XRI);
        assert_eq!(coding_history(&bytes), Some(XRI));
    }

    #[test]
    fn no_coding_history() {
        let bytes = x8_header(192_000 * 8, 2, 32, b"2026-03-15", b"21:04:00");
        assert_eq!(coding_history(&bytes), None);
        assert_eq!(coding_history(&bytes[..12 + 8 + BEXT_CODING_HISTORY]), None);
        assert_eq!(coding_history(b"RIFF\0\0\0\0WAVEfmt "), None);
    }

    #[test]
    fn a_guano_chunk_after_the_audio() {
        let mut out = [0u8; 256];
        let mut writer = ChunkWriter::new(&mut out, 1001);
        writer.field("Make", "TASCAM").field("Loc Position", "-36.123456 146.000007");
        let n = writer.finish().unwrap();
        // The tail as read from the sector before the chunk, which starts 1 byte in after its pad.
        let mut tail = Vec::from([0u8; 7]);
        tail.extend_from_slice(&out[..n]);
        let body = find_chunk(&tail, 7 + 1, b"guan").unwrap();
        let found: Vec<_> = fields(&tail[body]).collect();
        assert_eq!(found, [("GUANO|Version", "1.0"), ("Make", "TASCAM"), ("Loc Position", "-36.123456 146.000007")]);
    }

    #[test]
    fn a_guano_chunk_before_the_audio() {
        let mut bytes = Vec::from(*b"RIFF\0\0\0\0WAVE");
        bytes.extend_from_slice(b"guan\x0a\0\0\0Make: Pe\r\n");
        bytes.extend_from_slice(b"data\x10\0\0\0");
        let body = guano_in_header(&bytes).unwrap();
        assert_eq!(fields(&bytes[body]).collect::<Vec<_>>(), [("Make", "Pe")]);
        // One after the data chunk is not found by walking the header.
        let mut late = Vec::from(*b"RIFF\0\0\0\0WAVEdata\x02\0\0\0\0\0");
        late.extend_from_slice(b"guan\x02\0\0\0a:");
        assert_eq!(guano_in_header(&late), None);
    }

    #[test]
    fn a_chunk_cut_short_ends_with_the_bytes() {
        let bytes = b"JUNK\x02\0\0\0..guan\x00\x10\0\0Make: X";
        assert_eq!(find_chunk(bytes, 0, b"guan"), Some(18..bytes.len()));
        assert_eq!(find_chunk(bytes, 0, b"bext"), None);
    }

    #[test]
    fn fields_skip_lines_that_are_not_fields() {
        let text = "GUANO|Version: 1.0\n\nno colon here\n: no key\nNote: a: b \n\u{e9}: ok\n".as_bytes();
        let mut bad = Vec::from(text);
        bad.extend_from_slice(b"Bad: \xff\n");
        let found: Vec<_> = fields(&bad).collect();
        assert_eq!(found, [("GUANO|Version", "1.0"), ("Note", "a: b"), ("\u{e9}", "ok")]);
    }
}
