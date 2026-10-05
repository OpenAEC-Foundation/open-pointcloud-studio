//! A hash of 128 bits that stays the same on every machine, in every run
//! and with every version of Rust, unlike the hasher of the standard
//! library: FNV-1a with the 128-bit parameters. It names what was computed
//! from what, so that a saved project can tell whether a step is out of
//! date, and gives the stable ids of elements and their IFC GUIDs.
//!
//! Values go in with a fixed layout: integers little-endian, floats as
//! their bits, text and byte strings with their length first, so that
//! `("ab", "c")` and `("a", "bc")` hash differently.

const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

/// A running FNV-1a hash of 128 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StableHasher {
    state: u128,
}

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    pub fn new() -> Self {
        Self { state: OFFSET }
    }

    /// Bytes as they are, without their length.
    pub fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        for byte in bytes {
            self.state ^= u128::from(*byte);
            self.state = self.state.wrapping_mul(PRIME);
        }
        self
    }

    /// A byte string, with its length first.
    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.u64(bytes.len() as u64).raw(bytes)
    }

    pub fn str(&mut self, text: &str) -> &mut Self {
        self.bytes(text.as_bytes())
    }

    pub fn u64(&mut self, value: u64) -> &mut Self {
        self.raw(&value.to_le_bytes())
    }

    pub fn i64(&mut self, value: i64) -> &mut Self {
        self.raw(&value.to_le_bytes())
    }

    pub fn u128(&mut self, value: u128) -> &mut Self {
        self.raw(&value.to_le_bytes())
    }

    pub fn bool(&mut self, value: bool) -> &mut Self {
        self.raw(&[u8::from(value)])
    }

    /// A float by its bits, with every zero as positive zero and every NaN
    /// as one NaN, so that equal values hash equally.
    pub fn f64(&mut self, value: f64) -> &mut Self {
        let value = if value == 0.0 {
            0.0
        } else if value.is_nan() {
            f64::NAN
        } else {
            value
        };
        self.u64(value.to_bits())
    }

    pub fn f64s(&mut self, values: &[f64]) -> &mut Self {
        self.u64(values.len() as u64);
        for value in values {
            self.f64(*value);
        }
        self
    }

    /// An optional value: whether there is one, and then the value.
    pub fn option<T>(&mut self, value: Option<T>, add: impl FnOnce(&mut Self, T)) -> &mut Self {
        match value {
            Some(value) => {
                self.bool(true);
                add(self, value);
            }
            None => {
                self.bool(false);
            }
        }
        self
    }

    pub fn finish(&self) -> u128 {
        self.state
    }
}

/// The FNV-1a hash of 128 bits of these byte strings, each with its length.
pub fn stable_hash128(parts: &[&[u8]]) -> u128 {
    let mut hasher = StableHasher::new();
    for part in parts {
        hasher.bytes(part);
    }
    hasher.finish()
}

/// A hash as the 32 hexadecimal digits a file keeps it as.
pub fn hash_hex(hash: u128) -> String {
    format!("{hash:032x}")
}

/// The hash of 32 hexadecimal digits, as `hash_hex` writes it.
pub fn parse_hash_hex(text: &str) -> Option<u128> {
    (text.len() == 32)
        .then(|| u128::from_str_radix(text, 16).ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_fnv_1a_of_128_bits() {
        // The published vectors of FNV-1a with 128 bits.
        assert_eq!(StableHasher::new().finish(), OFFSET);
        assert_eq!(
            StableHasher::new().raw(b"a").finish(),
            0xd228_cb69_6f1a_8caf_7891_2b70_4e4a_8964
        );
        assert_eq!(
            StableHasher::new().raw(b"foobar").finish(),
            0x343e_1662_793c_64bf_6f0d_3597_ba44_6f18
        );
    }

    #[test]
    fn values_have_a_fixed_layout_and_their_lengths() {
        assert_ne!(
            stable_hash128(&[b"ab", b"c"]),
            stable_hash128(&[b"a", b"bc"])
        );
        assert_eq!(
            stable_hash128(&[b"ab", b"c"]),
            stable_hash128(&[b"ab", b"c"])
        );
        let zero = StableHasher::new().f64(0.0).finish();
        assert_eq!(StableHasher::new().f64(-0.0).finish(), zero);
        assert_ne!(StableHasher::new().f64(1e-300).finish(), zero);
        assert_eq!(
            StableHasher::new().f64(f64::NAN).finish(),
            StableHasher::new().f64(-f64::NAN).finish()
        );
        let some = StableHasher::new()
            .option(Some(2.5), |hasher, value| {
                hasher.f64(value);
            })
            .finish();
        let none = StableHasher::new()
            .option(None::<f64>, |hasher, value| {
                hasher.f64(value);
            })
            .finish();
        assert_ne!(some, none);
        // The layout does not change between versions: this digest is fixed.
        let mut hasher = StableHasher::new();
        hasher
            .str("level")
            .u64(3)
            .i64(-2)
            .f64(3.2)
            .bool(true)
            .f64s(&[1.0, 2.0]);
        assert_eq!(
            hash_hex(hasher.finish()),
            hash_hex(FIXED),
            "the layout of the hash changed"
        );
        assert_eq!(parse_hash_hex(&hash_hex(FIXED)), Some(FIXED));
        assert_eq!(parse_hash_hex("12"), None);
        assert_eq!(parse_hash_hex(&"g".repeat(32)), None);
    }

    /// The digest of the values of `values_have_a_fixed_layout_and_their_lengths`.
    const FIXED: u128 = 0xd565_56f1_641f_0074_5bee_b595_5272_70e8;
}
