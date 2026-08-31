//! Numeric serde helpers.
//!
//! TOML has one number type per kind, and `serde` is strict about which one it
//! sees. Two consequences we smooth over here:
//!
//! * `size = 13` (an integer) must be accepted for a float field — writing
//!   `13.0` is not something a user should have to remember.
//! * an `f32` widened to the `f64` TOML uses prints as `1.2000000476837158`.
//!   Serialising the *shortest decimal that round-trips as `f32`* keeps
//!   `docs/config-example.toml` readable.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserializer, Serializer};

/// `#[serde(with = "crate::num::f32_toml")]`.
pub(crate) mod f32_toml {
    use super::*;

    pub(crate) fn serialize<S: Serializer>(value: &f32, s: S) -> Result<S::Ok, S::Error> {
        let shortest = format!("{value}").parse::<f64>().unwrap_or(*value as f64);
        s.serialize_f64(shortest)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
        d.deserialize_any(NumberVisitor).map(|v| v as f32)
    }
}

/// `#[serde(with = "crate::num::f64_toml")]`.
pub(crate) mod f64_toml {
    use super::*;

    pub(crate) fn serialize<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(*value)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        d.deserialize_any(NumberVisitor)
    }
}

/// Accepts any TOML number and yields it as `f64`.
struct NumberVisitor;

impl Visitor<'_> for NumberVisitor {
    type Value = f64;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a number")
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<f64, E> {
        Ok(v)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<f64, E> {
        Ok(v as f64)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<f64, E> {
        Ok(v as f64)
    }
}
