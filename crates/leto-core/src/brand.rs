//! Leto's name and colours, in one place for every app.
//!
//! The palette is called "Braise". The logo shows a crescent moon holding a
//! sun: Leto with her children Artemis and Apollo.

/// Product name.
pub const NAME: &str = "Leto";

/// One-line description shown under the name.
pub const TAGLINE: &str = "Remote desktop and game streaming, unified.";

/// Project website.
pub const HOMEPAGE: &str = "https://leto.re";

/// An opaque sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// Builds a colour from its red, green and blue components.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// The colour as a `#RRGGBB` string.
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

/// Warm black: app background, and ink on light surfaces.
pub const GROUND: Rgb = Rgb::new(0x15, 0x13, 0x12);

/// Slightly raised surface on top of [`GROUND`], for cards and panels.
pub const SURFACE: Rgb = Rgb::new(0x21, 0x1E, 0x1C);

/// Orange sun: the accent colour.
pub const SUN: Rgb = Rgb::new(0xFF, 0x6B, 0x35);

/// Ivory moon: text and icons on dark surfaces, and the light background.
pub const MOON: Rgb = Rgb::new(0xF2, 0xED, 0xE4);

/// Muted ivory for secondary text on [`GROUND`].
pub const MOON_DIM: Rgb = Rgb::new(0xA8, 0xA2, 0x98);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_matches_the_brand_sheet() {
        assert_eq!(GROUND.hex(), "#151312");
        assert_eq!(SUN.hex(), "#FF6B35");
        assert_eq!(MOON.hex(), "#F2EDE4");
    }
}
