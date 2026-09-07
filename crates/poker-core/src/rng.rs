//! Deterministic RNG owned by this crate.
//!
//! Match reproducibility is a core promise: the same seed must deal the same
//! cards *forever*, across library versions and platforms. External RNG
//! crates do not guarantee cross-version stream stability, so we implement a
//! small, well-known generator ourselves: xoshiro256** seeded via splitmix64
//! (both public-domain algorithms by Blackman & Vigna).
//!
//! This generator is for dealing cards, not cryptography. Bots that want
//! randomness bring their own RNG.

/// splitmix64 step; used to expand seeds into generator state.
#[inline]
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// xoshiro256** — fast, tiny, statistically excellent for simulation use.
#[derive(Clone, Debug)]
pub struct Rng64 {
    s: [u64; 4],
}

impl Rng64 {
    /// Seed from a `(seed, stream)` pair. Distinct streams (e.g. one per
    /// hand number) yield independent sequences, so any hand of a match can
    /// be reproduced without replaying prior hands.
    pub fn from_seed_stream(seed: u64, stream: u64) -> Rng64 {
        let mut stream_state = stream ^ 0xD1B5_4A32_D192_ED03;
        let mut key = seed ^ splitmix64(&mut stream_state);
        let mut state = [
            splitmix64(&mut key),
            splitmix64(&mut key),
            splitmix64(&mut key),
            splitmix64(&mut key),
        ];
        // xoshiro must not be seeded with all zeros; splitmix output makes
        // that practically impossible, but be explicit.
        if state == [0; 4] {
            state = [0x9E37_79B9_7F4A_7C15; 4];
        }
        Rng64 { s: state }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform value in `0..n` via rejection sampling (no modulo bias).
    /// Panics if `n == 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "Rng64::below(0)");
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }

    /// In-place Fisher–Yates shuffle.
    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        for i in (1..slice.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            slice.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_are_deterministic_and_distinct() {
        let mut a = Rng64::from_seed_stream(1, 1);
        let mut b = Rng64::from_seed_stream(1, 1);
        let mut c = Rng64::from_seed_stream(1, 2);
        let sa: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let sb: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        let sc: Vec<u64> = (0..8).map(|_| c.next_u64()).collect();
        assert_eq!(sa, sb);
        assert_ne!(sa, sc);
    }

    /// Frozen stream snapshot: if this test fails, seed-reproducibility has
    /// been broken for every previously recorded match. Never "fix" the
    /// expected constants; fix the regression.
    #[test]
    fn stream_snapshot_is_frozen() {
        let mut rng = Rng64::from_seed_stream(0, 0);
        let got: Vec<u64> = (0..4).map(|_| rng.next_u64()).collect();
        assert_eq!(got, SNAPSHOT_SEED0_STREAM0, "xoshiro stream changed");
    }

    const SNAPSHOT_SEED0_STREAM0: [u64; 4] = [
        15_399_411_259_700_385_692,
        13_448_803_775_643_146_997,
        8_865_739_097_192_399_623,
        10_849_236_641_218_379_930,
    ];

    #[test]
    fn first_output_varies_with_stream() {
        const DECK_SIZE: u64 = 52;
        const STREAMS: std::ops::Range<u64> = 0..8;

        for seed in [0u64, 1, 2, 12345] {
            let first_card_per_stream: Vec<u64> = STREAMS
                .map(|stream| Rng64::from_seed_stream(seed, stream).below(DECK_SIZE))
                .collect();

            let all_same = first_card_per_stream
                .iter()
                .all(|&card| card == first_card_per_stream[0]);

            assert!(
                !all_same,
                "seed {seed}: every stream deals card {} first",
                first_card_per_stream[0]
            );
        }
    }

    #[test]
    fn below_is_in_range() {
        let mut rng = Rng64::from_seed_stream(5, 5);
        for n in [1u64, 2, 3, 13, 52, 1000] {
            for _ in 0..200 {
                assert!(rng.below(n) < n);
            }
        }
    }
}
