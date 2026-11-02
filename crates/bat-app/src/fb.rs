//! An RGB565 framebuffer in a caller's slice, as `embedded-graphics` draws to.
//!
//! On the X8 this would wrap the display controller's framebuffer; in the
//! emulator it wraps a buffer that is copied to a canvas.

use core::convert::Infallible;

use embedded_graphics::pixelcolor::{IntoStorage, Rgb565};
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

pub struct Rgb565Buffer<'a> {
    pixels: &'a mut [u16],
    width: usize,
    height: usize,
}

impl<'a> Rgb565Buffer<'a> {
    /// Panics if `pixels` is shorter than `width * height`.
    pub fn new(pixels: &'a mut [u16], width: usize, height: usize) -> Self {
        assert!(pixels.len() >= width * height, "framebuffer too small");
        Self { pixels, width, height }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// One row of raw pixels, for fast copies.
    pub fn row_mut(&mut self, y: usize) -> &mut [u16] {
        let start = y * self.width;
        &mut self.pixels[start..start + self.width]
    }

    pub fn pixels(&self) -> &[u16] {
        &self.pixels[..self.width * self.height]
    }
}

impl OriginDimensions for Rgb565Buffer<'_> {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

impl DrawTarget for Rgb565Buffer<'_> {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && (p.x as usize) < self.width && (p.y as usize) < self.height {
                self.pixels[p.y as usize * self.width + p.x as usize] = c.into_storage();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        let Some(bottom_right) = area.bottom_right() else { return Ok(()) };
        let raw = color.into_storage();
        let (x0, x1) = (area.top_left.x as usize, bottom_right.x as usize + 1);
        for y in area.top_left.y as usize..=bottom_right.y as usize {
            self.row_mut(y)[x0..x1].fill(raw);
        }
        Ok(())
    }
}
