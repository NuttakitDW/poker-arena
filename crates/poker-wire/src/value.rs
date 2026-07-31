//! Hand strength as it appears on the wire.
//!
//! [`HandValue`] is a thin, totally ordered `u32`: greater is better for the
//! pot side it was computed for. The *encodings* — and every evaluator that
//! produces them — live in `poker_core::eval`; this module only owns the
//! type that showdown events carry, so a bot can read `hi`/`lo` off the
//! stream without linking the engine.

/// Category of a *high* poker hand (also the class penalty scale for A-5).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum HandClass {
    HighCard = 0,
    OnePair,
    TwoPair,
    Trips,
    Straight,
    Flush,
    FullHouse,
    Quads,
    StraightFlush,
}

/// Totally ordered hand strength; greater wins the pot side it was computed
/// for. Only comparable against values from the same `EvalKind`.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct HandValue(pub u32);

impl HandValue {
    /// The [`HandClass`] of a value produced by `EvalKind::High`.
    /// Meaningless for values from other evaluators.
    pub fn high_class(self) -> HandClass {
        const CLASSES: [HandClass; 9] = [
            HandClass::HighCard,
            HandClass::OnePair,
            HandClass::TwoPair,
            HandClass::Trips,
            HandClass::Straight,
            HandClass::Flush,
            HandClass::FullHouse,
            HandClass::Quads,
            HandClass::StraightFlush,
        ];
        CLASSES[((self.0 >> 20) & 0xF) as usize]
    }
}
