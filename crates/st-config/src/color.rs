//! The colour type used by the `[theme]` section of `config.toml`.
//!
//! Colours are written as hex strings (`"#1e1e1e"`, or the short form `"#1e1"`)
//! and are always serialised back in the long `#rrggbb` form so that a
//! round-trip through [`crate::Config::to_toml_string`] is stable.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An opaque 24-bit colour.
///
/// In TOML this is a string: `"#rrggbb"` (or `"#rgb"`, which expands each digit).
/// A leading `#` is optional on input but always present on output.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Rgb {
    /// Red channel, 0-255.
    pub r: u8,
    /// Green channel, 0-255.
    pub g: u8,
    /// Blue channel, 0-255.
    pub b: u8,
}

impl Rgb {
    /// Builds a colour from its three channels.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Builds a colour from a packed `0xrrggbb` integer. The top byte is ignored.
    pub const fn from_u32(v: u32) -> Self {
        Self::new((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Packs the colour into `0x00rrggbb`.
    pub const fn to_u32(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }

    /// Renders the colour as a lowercase `#rrggbb` string.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl fmt::Debug for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rgb({self})")
    }
}

/// The error returned when a string is not a valid `#rrggbb` colour.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid colour {input:?}: expected a hex string such as \"#1e1e1e\" or \"#eee\" \
     (3 or 6 hex digits, optional leading `#`)"
)]
pub struct ParseColorError {
    /// The string that could not be parsed.
    pub input: String,
}

impl FromStr for Rgb {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseColorError {
            input: s.to_owned(),
        };
        let digits = s.strip_prefix('#').unwrap_or(s);
        if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(err());
        }
        let nibble = |i: usize| -> u8 { u8::from_str_radix(&digits[i..i + 1], 16).unwrap_or(0) };
        let byte = |i: usize| -> u8 { u8::from_str_radix(&digits[i..i + 2], 16).unwrap_or(0) };
        match digits.len() {
            3 => Ok(Self::new(nibble(0) * 17, nibble(1) * 17, nibble(2) * 17)),
            6 => Ok(Self::new(byte(0), byte(2), byte(4))),
            _ => Err(err()),
        }
    }
}

impl Serialize for Rgb {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HexVisitor;

        impl Visitor<'_> for HexVisitor {
            type Value = Rgb;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a hex colour string such as \"#1e1e1e\"")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Rgb, E> {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(HexVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_long_and_short_forms() {
        assert_eq!(
            "#1e1e1e".parse::<Rgb>().unwrap(),
            Rgb::new(0x1e, 0x1e, 0x1e)
        );
        assert_eq!("1e1e1e".parse::<Rgb>().unwrap(), Rgb::new(0x1e, 0x1e, 0x1e));
        assert_eq!("#EEE".parse::<Rgb>().unwrap(), Rgb::new(0xee, 0xee, 0xee));
        assert_eq!("#0f8".parse::<Rgb>().unwrap(), Rgb::new(0x00, 0xff, 0x88));
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "",
            "#",
            "#12",
            "#12345",
            "#1234567",
            "rebeccapurple",
            "#12345g",
        ] {
            assert!(bad.parse::<Rgb>().is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn round_trips_through_hex() {
        let c = Rgb::new(0x26, 0x4f, 0x78);
        assert_eq!(c.to_hex(), "#264f78");
        assert_eq!(c.to_hex().parse::<Rgb>().unwrap(), c);
        assert_eq!(Rgb::from_u32(0x264f78), c);
        assert_eq!(c.to_u32(), 0x264f78);
    }
}
