//! Runs [`bat_app::BatApp`] in the browser for the emulator page in `web/`.
//!
//! The page owns the audio graph and the canvas. It passes each block of
//! audio through [`Emulator::process`], forwards button, wheel and touch
//! events, and after [`Emulator::render`] copies [`Emulator::frame_ptr`]
//! (RGBA, 320 × 480) out of the WebAssembly memory into the canvas.

use bat_app::{BatApp, Buffers, Button, HEIGHT, Mode, Rgb565Buffer, WIDTH};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Emulator {
    app: BatApp<'static>,
    rgb565: Vec<u16>,
    rgba: Vec<u8>,
}

#[wasm_bindgen]
impl Emulator {
    #[wasm_bindgen(constructor)]
    pub fn new(sample_rate: f32) -> Emulator {
        // One second for the time expander, kept for the life of the page.
        let storage: &'static mut [f32] = Box::leak(vec![0.0f32; sample_rate as usize].into_boxed_slice());
        Emulator {
            app: BatApp::new(sample_rate, storage, Box::leak(Box::new(Buffers::new()))),
            rgb565: vec![0; WIDTH * HEIGHT],
            rgba: vec![255; WIDTH * HEIGHT * 4],
        }
    }

    /// Offers the settings the X8 patch offers, for previewing its settings pages.
    pub fn x8_host(&mut self) {
        self.app.set_mix_file_available(true);
        self.app.set_remote_available(true);
        self.app.set_guano_available(true);
        self.app.set_pause_option_available(true);
        self.app.set_power_option_available(true);
        self.app.set_debug_available(true);
    }

    pub fn width() -> usize {
        WIDTH
    }

    pub fn height() -> usize {
        HEIGHT
    }

    /// Inputs 3 and 4 in, headphone left and right out; all the same length.
    pub fn process(&mut self, in3: &[f32], in4: &[f32], out_l: &mut [f32], out_r: &mut [f32]) {
        self.app.process(in3, in4, out_l, out_r);
    }

    /// `home`, `play`, `rec` or `mark`.
    pub fn button(&mut self, name: &str) {
        let button = match name {
            "home" => Button::Home,
            "play" => Button::Play,
            "rec" => Button::Rec,
            "mark" => Button::Mark,
            _ => return,
        };
        self.app.button(button);
    }

    pub fn wheel(&mut self, steps: i32) {
        self.app.wheel(steps);
    }

    /// A tap at screen pixel (x, y).
    pub fn touch(&mut self, x: i32, y: i32) {
        self.app.touch(x, y);
    }

    /// Select mode 0 to 4 (HET, COMB, FD, TE, PITCH).
    pub fn set_mode(&mut self, index: usize) {
        if let Some(&mode) = Mode::ALL.get(index) {
            self.app.set_mode(mode);
        }
    }

    pub fn mode_label(&self) -> String {
        self.app.mode().label().to_string()
    }

    /// Whether a tap at screen pixel (x, y) would do something.
    pub fn is_tappable(&self, x: i32, y: i32) -> bool {
        self.app.is_tappable(x, y)
    }

    pub fn is_recording(&self) -> bool {
        self.app.is_recording()
    }

    pub fn set_clock(&mut self, seconds_since_midnight: u32) {
        self.app.set_clock(seconds_since_midnight);
    }

    /// Draw the screen into the RGBA frame.
    pub fn render(&mut self) {
        self.app.render(&mut Rgb565Buffer::new(&mut self.rgb565, WIDTH, HEIGHT));
        for (px, &c) in self.rgba.as_chunks_mut::<4>().0.iter_mut().zip(&self.rgb565) {
            let (r, g, b) = ((c >> 11) & 0x1f, (c >> 5) & 0x3f, c & 0x1f);
            px[0] = ((r << 3) | (r >> 2)) as u8;
            px[1] = ((g << 2) | (g >> 4)) as u8;
            px[2] = ((b << 3) | (b >> 2)) as u8;
        }
    }

    /// Address of the RGBA frame in the WebAssembly memory.
    pub fn frame_ptr(&self) -> *const u8 {
        self.rgba.as_ptr()
    }
}
