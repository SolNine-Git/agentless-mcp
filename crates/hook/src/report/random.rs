//! The Mersenne Twister generator seeded from a text, and the draw of a sample without
//! replacement, so a seed draws the same rows as in logs that earlier builds sampled.

use std::collections::HashSet;

use sha2::{Digest, Sha512};

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;
const INIT_SEED: u32 = 19_650_218;
// The size of a small set less the size of an empty list, in the rule that picks the draw.
const SMALL_SET: usize = 21;
const SMALL_SAMPLE: usize = 5;

pub(crate) struct Twister {
    state: [u32; N],
    index: usize,
}

impl Twister {
    // The generator that a text seed gives: its UTF-8 bytes and their SHA-512 digest, read as one
    // big-endian integer and cut into 32-bit key words from the least significant end.
    pub(crate) fn from_text(seed: &str) -> Self {
        let mut bytes = seed.as_bytes().to_vec();
        bytes.extend_from_slice(&Sha512::digest(seed.as_bytes()));
        let start = bytes
            .iter()
            .position(|&byte| byte != 0)
            .unwrap_or(bytes.len());
        let significant = &bytes[start..];
        let mut key: Vec<u32> = significant
            .rchunks(4)
            .map(|chunk| {
                chunk
                    .iter()
                    .fold(0, |word, &byte| (word << 8) | u32::from(byte))
            })
            .collect();
        if key.is_empty() {
            key.push(0);
        }
        let mut twister = Self::with_seed(INIT_SEED);
        twister.mix(&key);
        twister
    }

    fn with_seed(seed: u32) -> Self {
        let mut state = [0; N];
        state[0] = seed;
        for index in 1..N {
            let previous = state[index - 1];
            let step = u32::try_from(index).unwrap_or(u32::MAX);
            state[index] = 1_812_433_253_u32
                .wrapping_mul(previous ^ (previous >> 30))
                .wrapping_add(step);
        }
        Self { state, index: N }
    }

    fn mix(&mut self, key: &[u32]) {
        let mt = &mut self.state;
        let (mut i, mut j) = (1, 0);
        for _ in 0..N.max(key.len()) {
            let previous = mt[i - 1] ^ (mt[i - 1] >> 30);
            let offset = u32::try_from(j).unwrap_or(u32::MAX);
            mt[i] = (mt[i] ^ previous.wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(offset);
            (i, j) = (i + 1, j + 1);
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
            if j >= key.len() {
                j = 0;
            }
        }
        for _ in 1..N {
            let previous = mt[i - 1] ^ (mt[i - 1] >> 30);
            let offset = u32::try_from(i).unwrap_or(u32::MAX);
            mt[i] = (mt[i] ^ previous.wrapping_mul(1_566_083_941)).wrapping_sub(offset);
            i += 1;
            if i >= N {
                mt[0] = mt[N - 1];
                i = 1;
            }
        }
        mt[0] = UPPER_MASK;
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            self.twist();
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    fn twist(&mut self) {
        let mt = &mut self.state;
        for kk in 0..N {
            let y = (mt[kk] & UPPER_MASK) | (mt[(kk + 1) % N] & LOWER_MASK);
            let odd = if y & 1 == 1 { MATRIX_A } else { 0 };
            mt[kk] = mt[(kk + M) % N] ^ (y >> 1) ^ odd;
        }
        self.index = 0;
    }

    // `k` random bits, `k` at most 64: 32-bit words from the least significant, the last one cut.
    pub(crate) fn bits(&mut self, k: u32) -> u64 {
        let mut value = 0;
        let mut left = k;
        let mut shift = 0;
        while left > 0 {
            let word = self.next_u32();
            let taken = left.min(32);
            value |= u64::from(word >> (32 - taken)) << shift;
            shift += 32;
            left -= taken;
        }
        value
    }

    // A value in 0..n, for n above 0: draws of n's bit length until one is below n.
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        let k = u64::BITS - n.leading_zeros();
        loop {
            let drawn = self.bits(k);
            if drawn < n {
                return drawn;
            }
        }
    }

    // The positions of `k` items drawn without replacement from `n`, in draw order. An n-length
    // pool is used when it is smaller than a k-length set, else a set of the drawn positions.
    pub(crate) fn sample(&mut self, n: usize, k: usize) -> Vec<usize> {
        if n <= set_size(k) {
            self.pool_sample(n, k)
        } else {
            self.set_sample(n, k)
        }
    }

    fn pool_sample(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut pool: Vec<usize> = (0..n).collect();
        let mut drawn = Vec::with_capacity(k);
        for i in 0..k {
            let j = self.below_usize(n - i);
            drawn.push(pool[j]);
            pool[j] = pool[n - i - 1];
        }
        drawn
    }

    fn set_sample(&mut self, n: usize, k: usize) -> Vec<usize> {
        let mut selected = HashSet::with_capacity(k);
        let mut drawn = Vec::with_capacity(k);
        for _ in 0..k {
            let mut j = self.below_usize(n);
            while selected.contains(&j) {
                j = self.below_usize(n);
            }
            selected.insert(j);
            drawn.push(j);
        }
        drawn
    }

    fn below_usize(&mut self, n: usize) -> usize {
        let bound = u64::try_from(n).unwrap_or(u64::MAX);
        usize::try_from(self.below(bound)).unwrap_or(usize::MAX)
    }
}

// The population size up to which the pool draw is used: a small set, plus the table size of a
// k-length set, 4 to the power ceil(log4(3k)), when k is above 5.
pub(crate) fn set_size(k: usize) -> usize {
    if k <= SMALL_SAMPLE {
        return SMALL_SET;
    }
    let count = u32::try_from(k).map_or(f64::MAX, f64::from);
    let ratio = (count * 3.0).ln() / 4.0_f64.ln();
    SMALL_SET.saturating_add(4_usize.saturating_pow(ceiling(ratio)))
}

// The least whole number at or above `ratio`.
fn ceiling(ratio: f64) -> u32 {
    let mut whole = 0;
    while f64::from(whole) < ratio {
        whole += 1;
    }
    whole
}
