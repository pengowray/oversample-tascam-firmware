//! The GUANO metadata the patch adds to the WAV files of takes from a bat tile (docs/HOMEBREW.md,
//! "GUANO metadata"). This part has no firmware addresses, so the host tests can run it.

use guano::{ChunkWriter, Header};

/// What goes in the metadata besides the header's figures.
#[derive(Clone, Copy, Debug)]
pub struct Recorder<'a> {
    /// The stock firmware's version, as the X8 shows it.
    pub firmware: &'a str,
    /// The patch's build number, or 0 when unknown.
    pub patch_build: u32,
    /// The file's name, from [`guano::file_name`].
    pub file_name: Option<&'a str>,
}

/// Writes the bytes to add at [`Header::data_end`] into `out`: a pad byte if the audio has an odd
/// number of bytes, then the `guan` chunk. Returns how many bytes, or None if `out` is too small.
///
/// `Timestamp` is the start of the recording: the `bext` origination time less the length. The X8
/// rewrites the header, `bext` included, as it closes the file, so the origination time is the end
/// of the recording, to the second. The X8 does not know its time zone, so there is no UTC offset.
pub fn write_chunk(header: &Header, recorder: &Recorder, out: &mut [u8]) -> Option<usize> {
    let mut chunk = ChunkWriter::new(out, header.data_end()?);
    let length_us = header.length_us();
    let started = header.origination.zip(length_us).and_then(|(end, us)| end.minus_seconds((us + 500_000) / 1_000_000));
    if let Some(started) = started {
        chunk.field("Timestamp", started);
    }
    if let Some(us) = length_us {
        chunk.field("Length", format_args!("{}.{:06}", us / 1_000_000, us % 1_000_000));
    }
    chunk.field("Samplerate", header.sample_rate).field("Make", "TASCAM").field("Model", "Portacapture X8");
    if recorder.patch_build == 0 {
        chunk.field("Firmware Version", format_args!("{} with bat patch", recorder.firmware));
    } else {
        chunk.field("Firmware Version", format_args!("{} with bat patch B{}", recorder.firmware, recorder.patch_build));
    }
    if let Some(name) = recorder.file_name {
        chunk.field("Original Filename", name);
    }
    chunk.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guano::DateTime;
    use std::string::String;

    const RECORDER: Recorder = Recorder { firmware: "V2.03", patch_build: 242, file_name: Some("261106_0090_3-4.wav") };

    /// A header as the X8 writes one, 5548 bytes before the audio.
    fn header(data_size: u32, block_align: u16, date: &[u8], time: &[u8]) -> Header {
        Header {
            riff_size: 5548 + data_size - 8,
            sample_rate: 192_000,
            block_align,
            data_start: 5548,
            data_size,
            origination: DateTime::parse(date, time),
        }
    }

    fn text(header: &Header, recorder: &Recorder) -> String {
        let mut out = [0u8; 1024];
        let n = write_chunk(header, recorder, &mut out).unwrap();
        let pad = (header.data_size & 1) as usize;
        assert_eq!(&out[pad..pad + 4], b"guan");
        String::from_utf8(out[pad + 8..n].to_vec()).unwrap()
    }

    /// 261106_0090_3-4.wav: 89,210,880 bytes of stereo float, header written at 19:08:08.
    #[test]
    fn a_take_from_the_x8() {
        assert_eq!(
            text(&header(89_210_880, 8, b"2026-11-06", b"19:08:08"), &RECORDER),
            "GUANO|Version: 1.0\n\
             Timestamp: 2026-11-06T19:07:10\n\
             Length: 58.080000\n\
             Samplerate: 192000\n\
             Make: TASCAM\n\
             Model: Portacapture X8\n\
             Firmware Version: V2.03 with bat patch B242\n\
             Original Filename: 261106_0090_3-4.wav\n"
        );
    }

    #[test]
    fn the_start_can_fall_on_the_day_before() {
        let t = text(&header(120 * 192_000 * 8, 8, b"2024-03-01", b"00:00:30"), &RECORDER);
        assert!(t.contains("Timestamp: 2024-02-29T23:58:30\n"), "{t}");
    }

    #[test]
    fn an_unreadable_clock_leaves_the_timestamp_out() {
        let t = text(&header(192_000 * 8, 8, b"2026-13-01", b"12:00:00"), &RECORDER);
        assert!(t.starts_with("GUANO|Version: 1.0\nLength: 1.000000\n"), "{t}");
    }

    /// 24-bit mono has 3 bytes a frame, so an odd number of frames leaves the audio odd-sized.
    #[test]
    fn odd_sized_audio_gets_a_pad_byte() {
        let header = header(3 * 192_001, 3, b"2026-11-06", b"19:08:08");
        let mut out = [0xffu8; 1024];
        let n = write_chunk(&header, &RECORDER, &mut out).unwrap();
        assert_eq!(out[0], 0);
        assert_eq!(&out[1..5], b"guan");
        assert_eq!((header.data_end().unwrap() as usize + n) % 2, 0);
        assert!(text(&header, &RECORDER).contains("Length: 1.000005\n"));
    }

    #[test]
    fn without_a_build_or_name() {
        let recorder = Recorder { firmware: "V2.03", patch_build: 0, file_name: None };
        let t = text(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &recorder);
        assert!(t.contains("Firmware Version: V2.03 with bat patch\n"));
        assert!(!t.contains("Original Filename"));
    }

    #[test]
    fn too_small_a_buffer_is_refused() {
        assert!(write_chunk(&header(192_000 * 8, 8, b"2026-11-06", b"19:08:08"), &RECORDER, &mut [0u8; 64]).is_none());
    }

    /// Does what the patch does at the end of a take, on a copy of a real file:
    /// `X8_WAV=in.wav X8_WAV_OUT=out.wav cargo test -p x8-patch -- --ignored append_to_a_file`
    #[test]
    #[ignore]
    fn append_to_a_file() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let (Ok(input), Ok(output)) = (std::env::var("X8_WAV"), std::env::var("X8_WAV_OUT")) else { return };
        std::fs::copy(&input, &output).unwrap();
        let mut file = std::fs::OpenOptions::new().read(true).write(true).open(&output).unwrap();
        let size = u32::try_from(file.metadata().unwrap().len()).unwrap();
        let mut head = [0u8; 0x2000];
        let n = file.read(&mut head).unwrap();
        let header = guano::read_header(&head[..n]).unwrap();
        assert_eq!(header.riff_size + 8, size);
        assert_eq!(header.data_end(), Some(size));
        let name = std::path::Path::new(&input).file_name().unwrap().to_str().unwrap();
        let path = std::format!("{name}\0");
        let recorder = Recorder { firmware: "V2.03", patch_build: 242, file_name: guano::file_name(path.as_bytes()) };
        let mut out = [0u8; 0x400];
        let added = write_chunk(&header, &recorder, &mut out).unwrap();
        file.seek(SeekFrom::Start(u64::from(size))).unwrap();
        file.write_all(&out[..added]).unwrap();
        file.seek(SeekFrom::Start(4)).unwrap();
        file.write_all(&(header.riff_size + added as u32).to_le_bytes()).unwrap();
    }
}
