//! Colours.
//!
//! The interface colours are the ones the X8's own firmware uses (found as
//! colour strings in its resources), so the bat app looks like the stock apps.
//! The waterfall uses the Inferno colour map, as Oversample does for
//! ultrasound (its high-frequency view); the table is copied from Oversample's
//! `oversample-core` (MIT OR Apache-2.0), originally from matplotlib.

use embedded_graphics::pixelcolor::Rgb565;

const fn rgb(hex: u32) -> Rgb565 {
    Rgb565::new(((hex >> 19) & 0x1f) as u8, ((hex >> 10) & 0x3f) as u8, ((hex >> 3) & 0x1f) as u8)
}

pub const BACKGROUND: Rgb565 = rgb(0x000000);
/// Status bar and other raised panels.
pub const PANEL: Rgb565 = rgb(0x353535);
/// Unselected buttons.
pub const BUTTON: Rgb565 = rgb(0x444444);
/// Lines, scale marks and borders.
pub const LINE: Rgb565 = rgb(0x606060);
pub const TEXT: Rgb565 = rgb(0xFFFFFF);
/// Labels and secondary values.
pub const TEXT_MUTED: Rgb565 = rgb(0xC1C1C1);
/// The selected mode button and the heterodyne marker: the X8's blue.
pub const ACCENT: Rgb565 = rgb(0x0092E5);
/// Recording: the X8's red.
pub const RECORDING: Rgb565 = rgb(0xDF252D);
pub const METER_GREEN: Rgb565 = rgb(0x2BAF42);
pub const METER_YELLOW: Rgb565 = rgb(0xE5C100);
pub const METER_RED: Rgb565 = rgb(0xDF252D);
/// Title bar. Each stock app has its own title colour; this one is not used
/// by any of them.
pub const TITLE: Rgb565 = rgb(0x3B2E7E);
/// Ticks under the waterfall for each kind of detection that can be marked,
/// and the swatch beside each kind's switch on the settings screen. Bat calls
/// keep the yellow they always had; the others differ from it and from each
/// other in hue, and stand out from the waterfall's black, purple and orange.
pub const CALL_TICK: Rgb565 = rgb(0xF7D13D);
pub const TONE_TICK: Rgb565 = rgb(0x4DD9E8);
pub const AUDIBLE_TICK: Rgb565 = rgb(0xFF7AC8);
pub const BROADBAND_TICK: Rgb565 = rgb(0xE0E0E0);

/// Tick colour for a kind of detection.
pub fn tick_colour(kind: bat_dsp::classify::CallKind) -> Rgb565 {
    use bat_dsp::classify::CallKind;
    match kind {
        CallKind::Bat | CallKind::Single => CALL_TICK,
        CallKind::Tone => TONE_TICK,
        CallKind::Audible => AUDIBLE_TICK,
        CallKind::Broadband | CallKind::TooLow => BROADBAND_TICK,
    }
}

/// Resistor colour code by digit, as Oversample colours its frequency marks
/// (each 10 kHz line takes the colour of its tens digit: 40 kHz is yellow).
pub const RESISTOR: [Rgb565; 10] = [
    rgb(0x282828),
    rgb(0x8B4513),
    rgb(0xFF0000),
    rgb(0xFFA500),
    rgb(0xFFFF00),
    rgb(0x008000),
    rgb(0x0000FF),
    rgb(0x9400D3),
    rgb(0x808080),
    rgb(0xFFFFFF),
];

/// Inferno, as RGB565 raw values, indexed by level 0 (quiet) to 255 (loud).
pub static INFERNO_565: [u16; 256] = inferno_565();

/// The waterfall's colours: Inferno, except that the last value shows a display
/// column that was not analysed. It has the colour of `PANEL`, a dark grey that
/// is not in the colour map, so it reads as no data and not as a loud sound.
pub static WATERFALL_565: [u16; 256] = {
    let mut out = inferno_565();
    out[crate::WF_NO_DATA as usize] = (0x35 >> 3 << 11) | (0x35 >> 2 << 5) | (0x35 >> 3);
    out
};

const fn inferno_565() -> [u16; 256] {
    let mut out = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let [r, g, b] = INFERNO[i];
        out[i] = ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
        i += 1;
    }
    out
}

const INFERNO: [[u8; 3]; 256] = [
    [0, 0, 3],
    [0, 0, 4],
    [0, 0, 6],
    [1, 0, 7],
    [1, 1, 9],
    [1, 1, 11],
    [2, 1, 14],
    [2, 2, 16],
    [3, 2, 18],
    [4, 3, 20],
    [4, 3, 22],
    [5, 4, 24],
    [6, 4, 27],
    [7, 5, 29],
    [8, 6, 31],
    [9, 6, 33],
    [10, 7, 35],
    [11, 7, 38],
    [13, 8, 40],
    [14, 8, 42],
    [15, 9, 45],
    [16, 9, 47],
    [18, 10, 50],
    [19, 10, 52],
    [20, 11, 54],
    [22, 11, 57],
    [23, 11, 59],
    [25, 11, 62],
    [26, 11, 64],
    [28, 12, 67],
    [29, 12, 69],
    [31, 12, 71],
    [32, 12, 74],
    [34, 11, 76],
    [36, 11, 78],
    [38, 11, 80],
    [39, 11, 82],
    [41, 11, 84],
    [43, 10, 86],
    [45, 10, 88],
    [46, 10, 90],
    [48, 10, 92],
    [50, 9, 93],
    [52, 9, 95],
    [53, 9, 96],
    [55, 9, 97],
    [57, 9, 98],
    [59, 9, 100],
    [60, 9, 101],
    [62, 9, 102],
    [64, 9, 102],
    [65, 9, 103],
    [67, 10, 104],
    [69, 10, 105],
    [70, 10, 105],
    [72, 11, 106],
    [74, 11, 106],
    [75, 12, 107],
    [77, 12, 107],
    [79, 13, 108],
    [80, 13, 108],
    [82, 14, 108],
    [83, 14, 109],
    [85, 15, 109],
    [87, 15, 109],
    [88, 16, 109],
    [90, 17, 109],
    [91, 17, 110],
    [93, 18, 110],
    [95, 18, 110],
    [96, 19, 110],
    [98, 20, 110],
    [99, 20, 110],
    [101, 21, 110],
    [102, 21, 110],
    [104, 22, 110],
    [106, 23, 110],
    [107, 23, 110],
    [109, 24, 110],
    [110, 24, 110],
    [112, 25, 110],
    [114, 25, 109],
    [115, 26, 109],
    [117, 27, 109],
    [118, 27, 109],
    [120, 28, 109],
    [122, 28, 109],
    [123, 29, 108],
    [125, 29, 108],
    [126, 30, 108],
    [128, 31, 107],
    [129, 31, 107],
    [131, 32, 107],
    [133, 32, 106],
    [134, 33, 106],
    [136, 33, 106],
    [137, 34, 105],
    [139, 34, 105],
    [141, 35, 105],
    [142, 36, 104],
    [144, 36, 104],
    [145, 37, 103],
    [147, 37, 103],
    [149, 38, 102],
    [150, 38, 102],
    [152, 39, 101],
    [153, 40, 100],
    [155, 40, 100],
    [156, 41, 99],
    [158, 41, 99],
    [160, 42, 98],
    [161, 43, 97],
    [163, 43, 97],
    [164, 44, 96],
    [166, 44, 95],
    [167, 45, 95],
    [169, 46, 94],
    [171, 46, 93],
    [172, 47, 92],
    [174, 48, 91],
    [175, 49, 91],
    [177, 49, 90],
    [178, 50, 89],
    [180, 51, 88],
    [181, 51, 87],
    [183, 52, 86],
    [184, 53, 86],
    [186, 54, 85],
    [187, 55, 84],
    [189, 55, 83],
    [190, 56, 82],
    [191, 57, 81],
    [193, 58, 80],
    [194, 59, 79],
    [196, 60, 78],
    [197, 61, 77],
    [199, 62, 76],
    [200, 62, 75],
    [201, 63, 74],
    [203, 64, 73],
    [204, 65, 72],
    [205, 66, 71],
    [207, 68, 70],
    [208, 69, 68],
    [209, 70, 67],
    [210, 71, 66],
    [212, 72, 65],
    [213, 73, 64],
    [214, 74, 63],
    [215, 75, 62],
    [217, 77, 61],
    [218, 78, 59],
    [219, 79, 58],
    [220, 80, 57],
    [221, 82, 56],
    [222, 83, 55],
    [223, 84, 54],
    [224, 86, 52],
    [226, 87, 51],
    [227, 88, 50],
    [228, 90, 49],
    [229, 91, 48],
    [230, 92, 46],
    [230, 94, 45],
    [231, 95, 44],
    [232, 97, 43],
    [233, 98, 42],
    [234, 100, 40],
    [235, 101, 39],
    [236, 103, 38],
    [237, 104, 37],
    [237, 106, 35],
    [238, 108, 34],
    [239, 109, 33],
    [240, 111, 31],
    [240, 112, 30],
    [241, 114, 29],
    [242, 116, 28],
    [242, 117, 26],
    [243, 119, 25],
    [243, 121, 24],
    [244, 122, 22],
    [245, 124, 21],
    [245, 126, 20],
    [246, 128, 18],
    [246, 129, 17],
    [247, 131, 16],
    [247, 133, 14],
    [248, 135, 13],
    [248, 136, 12],
    [248, 138, 11],
    [249, 140, 9],
    [249, 142, 8],
    [249, 144, 8],
    [250, 145, 7],
    [250, 147, 6],
    [250, 149, 6],
    [250, 151, 6],
    [251, 153, 6],
    [251, 155, 6],
    [251, 157, 6],
    [251, 158, 7],
    [251, 160, 7],
    [251, 162, 8],
    [251, 164, 10],
    [251, 166, 11],
    [251, 168, 13],
    [251, 170, 14],
    [251, 172, 16],
    [251, 174, 18],
    [251, 176, 20],
    [251, 177, 22],
    [251, 179, 24],
    [251, 181, 26],
    [251, 183, 28],
    [251, 185, 30],
    [250, 187, 33],
    [250, 189, 35],
    [250, 191, 37],
    [250, 193, 40],
    [249, 195, 42],
    [249, 197, 44],
    [249, 199, 47],
    [248, 201, 49],
    [248, 203, 52],
    [248, 205, 55],
    [247, 207, 58],
    [247, 209, 60],
    [246, 211, 63],
    [246, 213, 66],
    [245, 215, 69],
    [245, 217, 72],
    [244, 219, 75],
    [244, 220, 79],
    [243, 222, 82],
    [243, 224, 86],
    [243, 226, 89],
    [242, 228, 93],
    [242, 230, 96],
    [241, 232, 100],
    [241, 233, 104],
    [241, 235, 108],
    [241, 237, 112],
    [241, 238, 116],
    [241, 240, 121],
    [241, 242, 125],
    [242, 243, 129],
    [242, 244, 133],
    [243, 246, 137],
    [244, 247, 141],
    [245, 248, 145],
    [246, 250, 149],
    [247, 251, 153],
    [249, 252, 157],
    [250, 253, 160],
    [252, 254, 164],
];
