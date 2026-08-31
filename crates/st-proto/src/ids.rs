//! Identifier newtypes shared by both planes.
//!
//! Every id is a plain integer on the wire (`#[serde(transparent)]`), so the
//! JSON encoding is a bare number — matching the TypeScript
//! `type SessionId = number` aliases in `docs/plan/02-protocol.md` §3.2 — and
//! the postcard encoding is a single varint.
//!
//! Widths follow `docs/plan/02-protocol.md`:
//!
//! | Type | Width | Meaning |
//! |---|---|---|
//! | [`SessionId`], [`TabId`], [`SurfaceId`] | `u32` | server-allocated, never reused during a server lifetime (§1 conventions) |
//! | [`Seq`] | `u64` | per-Surface monotonic state counter, starts at 1 (§6) |
//! | [`AbsLine`] | `u64` | absolute line id, assigned once per line, never renumbered (§8) |
//! | [`StyleIdx`] | `u16` | index into a Surface's style table, 0 = default (§5.3) |

use std::fmt;

macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident, $repr:ty) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name(pub $repr);

        impl $name {
            #[doc = concat!("The zero value of [`", stringify!($name), "`].")]
            pub const ZERO: Self = Self(0);

            #[doc = concat!("Wraps a raw `", stringify!($repr), "` as a [`", stringify!($name), "`].")]
            #[inline]
            #[must_use]
            pub const fn new(raw: $repr) -> Self {
                Self(raw)
            }

            #[doc = concat!("Returns the raw `", stringify!($repr), "` value.")]
            #[inline]
            #[must_use]
            pub const fn get(self) -> $repr {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<$repr> for $name {
            #[inline]
            fn from(raw: $repr) -> Self {
                Self(raw)
            }
        }

        impl From<$name> for $repr {
            #[inline]
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

id_newtype! {
    /// Identifies a Session (a named group of Tabs) within the Workspace.
    SessionId, u32
}

id_newtype! {
    /// Identifies a Tab within the Workspace. A Tab holds exactly one Surface in v1.
    TabId, u32
}

id_newtype! {
    /// Identifies a Surface (one PTY + one authoritative terminal state machine).
    SurfaceId, u32
}

id_newtype! {
    /// Per-Surface state sequence number.
    ///
    /// Starts at 1 (the Surface's creation state) and increments by exactly one
    /// for every authoritative state change (`Snapshot`, `Delta`, `SurfaceExited`).
    /// `0` means "nothing known" in [`crate::data::Attach::known_seq`].
    Seq, u64
}

id_newtype! {
    /// Absolute line id: assigned once when a line is created and never renumbered.
    ///
    /// Ids increase downward. History reflow is disabled in v1 (grilling Q40)
    /// precisely so this invariant holds.
    AbsLine, u64
}

id_newtype! {
    /// Index into a Surface's style table. Index `0` is always the default style.
    StyleIdx, u16
}

impl Seq {
    /// The first sequence number a Surface ever emits.
    pub const FIRST: Self = Self(1);

    /// Returns the next sequence number, saturating at `u64::MAX`.
    #[inline]
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl AbsLine {
    /// Returns this line id advanced by `n` lines, saturating at `u64::MAX`.
    #[inline]
    #[must_use]
    pub const fn saturating_add(self, n: u64) -> Self {
        Self(self.0.saturating_add(n))
    }

    /// Returns `self - other` as a line count, or `None` when `self < other`.
    #[inline]
    #[must_use]
    pub const fn checked_sub(self, other: Self) -> Option<u64> {
        self.0.checked_sub(other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_transparent_in_json() {
        assert_eq!(serde_json::to_string(&SurfaceId(9)).unwrap(), "9");
        assert_eq!(
            serde_json::from_str::<SurfaceId>("9").unwrap(),
            SurfaceId(9)
        );
    }

    #[test]
    fn ids_are_a_bare_varint_in_postcard() {
        assert_eq!(postcard::to_stdvec(&StyleIdx(1)).unwrap(), vec![1]);
        assert_eq!(postcard::to_stdvec(&Seq(300)).unwrap(), vec![0xAC, 0x02]);
    }

    #[test]
    fn seq_and_absline_arithmetic() {
        assert_eq!(Seq::FIRST.next(), Seq(2));
        assert_eq!(Seq(u64::MAX).next(), Seq(u64::MAX));
        assert_eq!(AbsLine(10).saturating_add(5), AbsLine(15));
        assert_eq!(AbsLine(10).checked_sub(AbsLine(4)), Some(6));
        assert_eq!(AbsLine(4).checked_sub(AbsLine(10)), None);
    }

    #[test]
    fn ids_round_trip_through_raw_values() {
        let id = TabId::new(12);
        assert_eq!(u32::from(id), 12);
        assert_eq!(TabId::from(12u32), id);
        assert_eq!(id.to_string(), "12");
        assert_eq!(SessionId::ZERO.get(), 0);
    }
}
