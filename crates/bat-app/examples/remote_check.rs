//! How the phone remote's tone detector does on recordings, and how often it wakes the chime
//! decoders with no phone playing. Each wake costs the decoders' CPU for 6 s.
//!
//! cargo run -p bat-app --release --example remote_check -- FILE.wav [FILE.wav ...]
//!
//! Files must be at 192 kHz. Prints the seconds of audio and the tones heard for each file, then
//! the totals.

use bat_app::{BatApp, Buffers, InputName};

fn main() {
    let (mut all_s, mut all_tones) = (0.0f64, 0u32);
    for path in std::env::args().skip(1) {
        let Ok(mut reader) = hound::WavReader::open(&path) else {
            eprintln!("{path}: cannot open");
            continue;
        };
        let spec = reader.spec();
        if spec.sample_rate != 192_000 {
            continue;
        }
        let channels = spec.channels as usize;
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().map_while(Result::ok).collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map_while(Result::ok).map(|s| s as f32 * scale).collect()
            }
        };
        let frames = samples.len() / channels;
        let first: Vec<f32> = samples.iter().step_by(channels).copied().collect();
        let second: Vec<f32> = if channels > 1 { samples.iter().skip(1).step_by(channels).copied().collect() } else { first.clone() };
        let mut storage = vec![0.0f32; 192_000];
        let mut buffers = Box::new(Buffers::new());
        let mut app = BatApp::new(192_000.0, &mut storage, &mut buffers);
        app.set_remote_available(true);
        app.set_remote_on(true);
        let name = |short, long| Some(InputName { short, long });
        app.set_inputs([name("1", "First"), if channels > 1 { name("2", "Second") } else { None }]);
        let (mut l, mut r) = (vec![0.0; 1024], vec![0.0; 1024]);
        let mut commands = 0;
        for (a, b) in first.chunks(1024).zip(second.chunks(1024)) {
            app.process(a, b, &mut l[..a.len()], &mut r[..a.len()]);
            commands += app.take_remote_command().is_some() as u32;
        }
        let seconds = frames as f64 / 192_000.0;
        let (tones, read, failed) = app.remote_counts();
        println!("{tones:4} tones  {read:3} chimes read  {failed:3} failed  {commands:2} commands  {seconds:7.1} s  {path}");
        all_s += seconds;
        all_tones += tones;
    }
    println!("total: {all_tones} tones in {:.1} min, {:.2} a minute", all_s / 60.0, all_tones as f64 / (all_s / 60.0).max(1e-9));
}
