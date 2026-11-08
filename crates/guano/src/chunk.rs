use core::fmt::{self, Write};

/// Builds a `guan` chunk in a byte buffer, to be written at a given file offset, normally the end
/// of the audio. The `GUANO|Version` field goes first. [`finish`](Self::finish) pads the text with
/// a line break to an even length, as the spec asks, and fills in the chunk's size.
pub struct ChunkWriter<'a> {
    out: &'a mut [u8],
    /// Where the text starts in `out`: after the pad byte, if there is one, and the chunk header.
    text_start: usize,
    /// Bytes of `out` used so far.
    len: usize,
    /// A field did not fit in `out`.
    full: bool,
}

/// The bytes after `len` in a buffer, for `write!`.
struct Tail<'b> {
    out: &'b mut [u8],
    len: &'b mut usize,
}

impl Write for Tail<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        self.out.get_mut(*self.len..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
        *self.len = end;
        Ok(())
    }
}

impl<'a> ChunkWriter<'a> {
    /// Starts a chunk that will be written at file offset `at`. RIFF chunks start on even offsets,
    /// so for an odd `at` the bytes start with a pad byte.
    pub fn new(out: &'a mut [u8], at: u32) -> Self {
        let text_start = (at & 1) as usize + 8;
        let full = out.len() < text_start;
        let mut writer = Self { out, text_start, len: text_start, full };
        writer.field("GUANO|Version", "1.0");
        writer
    }

    /// Adds a `key: value` line. A field whose key has a colon or a line break, or whose value has
    /// a line break, is left out, because GUANO has no way to escape them.
    pub fn field(&mut self, key: &str, value: impl fmt::Display) -> &mut Self {
        if self.full {
            return self;
        }
        let line_start = self.len;
        let written = writeln!(Tail { out: self.out, len: &mut self.len }, "{key}: {value}").is_ok();
        let breaks = |text: &[u8]| text.iter().any(|&b| b == b'\n' || b == b'\r');
        let value_start = line_start + key.len() + 2;
        if !written {
            self.len = line_start;
            self.full = true;
        } else if key.contains(':') || breaks(key.as_bytes()) || breaks(&self.out[value_start..self.len - 1]) {
            self.len = line_start;
        }
        self
    }

    /// Finishes the chunk. Returns how many bytes of the buffer to write at the offset given to
    /// [`new`](Self::new), or None if a field did not fit.
    pub fn finish(mut self) -> Option<usize> {
        if self.full {
            return None;
        }
        if (self.len - self.text_start) % 2 == 1 {
            Tail { out: self.out, len: &mut self.len }.write_str("\n").ok()?;
        }
        let pad = self.text_start - 8;
        if pad == 1 {
            self.out[0] = 0;
        }
        self.out[pad..pad + 4].copy_from_slice(b"guan");
        let size = u32::try_from(self.len - self.text_start).ok()?;
        self.out[pad + 4..pad + 8].copy_from_slice(&size.to_le_bytes());
        Some(self.len)
    }
}

/// The part of a stored path after the last `/`, `\` or `:`, up to a NUL, if it is printable ASCII.
/// For `Original Filename`: anything else is better left out than written with a line break or as
/// invalid UTF-8.
pub fn file_name(path: &[u8]) -> Option<&str> {
    let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    let path = &path[..end];
    let start = path.iter().rposition(|&b| matches!(b, b'/' | b'\\' | b':')).map_or(0, |i| i + 1);
    let name = &path[start..];
    if name.is_empty() || !name.iter().all(|&b| (0x20..0x7f).contains(&b)) {
        return None;
    }
    core::str::from_utf8(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;

    /// The chunk's text, after checking its header, size and padding.
    fn text(out: &[u8], n: usize, at: u32) -> String {
        let pad = (at & 1) as usize;
        if pad == 1 {
            assert_eq!(out[0], 0);
        }
        assert_eq!(&out[pad..pad + 4], b"guan");
        let size = u32::from_le_bytes(out[pad + 4..pad + 8].try_into().unwrap()) as usize;
        assert_eq!(n, pad + 8 + size);
        assert_eq!(size % 2, 0, "the text is padded to an even length");
        assert_eq!((at as usize + n) % 2, 0, "the file ends on an even offset");
        String::from_utf8(out[pad + 8..n].to_vec()).unwrap()
    }

    #[test]
    fn version_comes_first_and_the_text_is_padded() {
        let mut out = [0u8; 256];
        let mut writer = ChunkWriter::new(&mut out, 1000);
        writer.field("Samplerate", 192_000).field("Make", "TASCAM");
        let n = writer.finish().unwrap();
        assert_eq!(text(&out, n, 1000), "GUANO|Version: 1.0\nSamplerate: 192000\nMake: TASCAM\n\n");
    }

    #[test]
    fn an_odd_offset_gets_a_pad_byte() {
        let mut out = [0xffu8; 256];
        let mut writer = ChunkWriter::new(&mut out, 1001);
        writer.field("Make", "TASCAM");
        let n = writer.finish().unwrap();
        assert_eq!(text(&out, n, 1001), "GUANO|Version: 1.0\nMake: TASCAM\n");
    }

    #[test]
    fn fields_that_would_break_the_format_are_left_out() {
        let mut out = [0u8; 256];
        let mut writer = ChunkWriter::new(&mut out, 0);
        writer
            .field("Note", "two\nlines")
            .field("Note", "carriage\rreturn")
            .field("Bad: key", 1)
            .field("Bad\nkey", 1)
            .field("Make", "TASCAM");
        let n = writer.finish().unwrap();
        assert_eq!(text(&out, n, 0), "GUANO|Version: 1.0\nMake: TASCAM\n");
    }

    #[test]
    fn too_small_a_buffer_is_refused() {
        let mut out = [0u8; 40];
        let mut writer = ChunkWriter::new(&mut out, 0);
        writer.field("Firmware Version", "a value too long for the buffer");
        assert_eq!(writer.finish(), None);
        assert_eq!(ChunkWriter::new(&mut [0u8; 7], 0).finish(), None);
        assert_eq!(ChunkWriter::new(&mut [0u8; 20], 0).finish(), None);
    }

    #[test]
    fn file_names_from_stored_paths() {
        assert_eq!(file_name(b"0:/MUSIC/261106_0090_3-4.wav\0junk"), Some("261106_0090_3-4.wav"));
        assert_eq!(file_name(b"A:\\261106_0090_MIX.wav\0"), Some("261106_0090_MIX.wav"));
        assert_eq!(file_name(b"261106_0090_5.wav\0"), Some("261106_0090_5.wav"));
        assert_eq!(file_name(b"0:/MUSIC/\0"), None);
        assert_eq!(file_name(b"bad\nname.wav\0"), None);
        assert_eq!(file_name(b"\xe9t\xe9.wav\0"), None);
    }
}
