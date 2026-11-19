//! Rabin-Karp style rolling hashes over normalized token streams.

use crate::normalize::NormalizedToken;

const BASE: u64 = 1_000_003;
const MOD: u64 = 0xFFFF_FFFF_FFFF_FFC5; // large 64-bit prime-ish modulus

#[derive(Clone, Copy, Debug)]
pub struct WindowHit {
    pub file_idx: usize,
    pub start: usize,
    pub hash: u64,
}

/// Compute rolling hashes for every window of `window` tokens.
pub fn window_hashes(tokens: &[NormalizedToken], window: usize) -> Vec<(usize, u64)> {
    if tokens.len() < window || window == 0 {
        return Vec::new();
    }

    let mut pow = 1u64;
    for _ in 1..window {
        pow = mul_mod(pow, BASE);
    }

    let mut hash = 0u64;
    for token in tokens.iter().take(window) {
        hash = add_mod(mul_mod(hash, BASE), u64::from(token.symbol));
    }

    let mut out = Vec::with_capacity(tokens.len() - window + 1);
    out.push((0, hash));

    for i in window..tokens.len() {
        let leave = u64::from(tokens[i - window].symbol);
        let enter = u64::from(tokens[i].symbol);
        // hash = (hash - leave * BASE^(W-1)) * BASE + enter
        let leave_term = mul_mod(leave, pow);
        hash = sub_mod(hash, leave_term);
        hash = add_mod(mul_mod(hash, BASE), enter);
        out.push((i - window + 1, hash));
    }

    out
}

/// Fingerprint a token slice (used for clone group identity).
pub fn fingerprint_tokens(tokens: &[NormalizedToken]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64; // FNV-1a 64 offset
    for t in tokens {
        hash ^= u64::from(t.symbol);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[inline]
fn mul_mod(a: u64, b: u64) -> u64 {
    ((u128::from(a) * u128::from(b)) % u128::from(MOD)) as u64
}

#[inline]
fn add_mod(a: u64, b: u64) -> u64 {
    let sum = u128::from(a) + u128::from(b);
    (sum % u128::from(MOD)) as u64
}

#[inline]
fn sub_mod(a: u64, b: u64) -> u64 {
    if a >= b {
        a - b
    } else {
        MOD - (b - a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(sym: u32) -> NormalizedToken {
        NormalizedToken {
            symbol: sym,
            start_line: 1,
            end_line: 1,
        }
    }

    #[test]
    fn rolling_matches_naive() {
        let tokens: Vec<_> = (0..20u32).map(tok).collect();
        let window = 5;
        let rolling = window_hashes(&tokens, window);

        for (start, rh) in rolling {
            let mut naive = 0u64;
            for t in &tokens[start..start + window] {
                naive = add_mod(mul_mod(naive, BASE), u64::from(t.symbol));
            }
            assert_eq!(rh, naive, "mismatch at start={start}");
        }
    }
}
