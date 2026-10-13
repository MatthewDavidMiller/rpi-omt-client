//! Unicode Normalization Form C, for the one question the validators ask:
//! is this string already in NFC?
//!
//! The algorithm is the one in UAX #15: full canonical decomposition, the
//! canonical ordering of combining marks, then canonical composition. Hangul
//! syllables are decomposed and composed arithmetically rather than by
//! table. The tables are Unicode 16.0.0 (tools/gen/gen_nfc_tables.py).

use crate::nfc_tables::{CCC, COMP, DECOMP};

const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

pub fn combining_class(cp: u32) -> u8 {
    CCC.binary_search_by_key(&cp, |&(c, _)| c).map_or(0, |i| CCC[i].1)
}

fn compose_pair(a: u32, b: u32) -> Option<u32> {
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        return Some(S_BASE + ((a - L_BASE) * V_COUNT + (b - V_BASE)) * T_COUNT);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE).is_multiple_of(T_COUNT)
        && b > T_BASE
        && b < T_BASE + T_COUNT
    {
        return Some(a + (b - T_BASE));
    }
    COMP.binary_search_by_key(&(a, b), |&(x, y, _)| (x, y)).ok().map(|i| COMP[i].2)
}

/// Canonical mappings nest at most a few levels; the depth bound is generous.
fn decompose(out: &mut Vec<u32>, cp: u32, depth: u32, limit: usize) -> bool {
    if depth > 8 {
        return false;
    }
    let mut push = |c: u32| {
        if out.len() >= limit {
            return false;
        }
        out.push(c);
        true
    };
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let index = cp - S_BASE;
        return push(L_BASE + index / N_COUNT)
            && push(V_BASE + (index % N_COUNT) / T_COUNT)
            && (index.is_multiple_of(T_COUNT) || push(T_BASE + index % T_COUNT));
    }
    match DECOMP.binary_search_by_key(&cp, |&(c, _, _)| c) {
        Err(_) => push(cp),
        Ok(i) => {
            let (_, first, second) = DECOMP[i];
            decompose(out, first, depth + 1, limit)
                && (second == 0 || decompose(out, second, depth + 1, limit))
        }
    }
}

fn to_nfc(s: &str) -> Option<Vec<u32>> {
    let limit = s.len().checked_mul(4)?.checked_add(16)?;
    let mut v = Vec::new();
    for c in s.chars() {
        if !decompose(&mut v, u32::from(c), 0, limit) {
            return None;
        }
    }
    // Canonical ordering: a stable insertion sort of each run of
    // non-starters by combining class.
    for k in 1..v.len() {
        let c = combining_class(v[k]);
        if c == 0 {
            continue;
        }
        let cp = v[k];
        let mut j = k;
        while j > 0 {
            let prev = combining_class(v[j - 1]);
            if prev == 0 || prev <= c {
                break;
            }
            v[j] = v[j - 1];
            j -= 1;
        }
        v[j] = cp;
    }
    // Canonical composition.
    let mut starter: Option<usize> = None;
    let mut out = 0;
    // The class of the most recent character kept after the starter; a
    // starter directly before is never blocking.
    let mut last_class: Option<u8> = None;
    for k in 0..v.len() {
        let cp = v[k];
        let c = combining_class(cp);
        if let Some(s) = starter {
            let blocked = last_class.is_some_and(|last| last == 0 || last >= c);
            if !blocked && let Some(composite) = compose_pair(v[s], cp) {
                v[s] = composite;
                continue;
            }
        }
        if c == 0 {
            starter = Some(out);
            last_class = None;
        } else {
            last_class = Some(c);
        }
        v[out] = cp;
        out += 1;
    }
    v.truncate(out);
    Some(v)
}

/// `s` normalized to NFC, or `None` when the work would pass its bound.
pub fn normalize(s: &str) -> Option<String> {
    to_nfc(s)?.into_iter().map(char::from_u32).collect()
}

/// True when `s` is unchanged by NFC. Inputs longer than `limit` bytes are
/// refused (false), keeping the work bounded.
pub fn is_normalized(s: &str, limit: usize) -> bool {
    if s.len() > limit {
        return false;
    }
    to_nfc(s).is_some_and(|v| s.chars().map(u32::from).eq(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(hex: &str) -> Vec<u8> {
        (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn matches_the_python_oracle() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors/nfc/vectors.txt");
        let text = std::fs::read_to_string(root).unwrap();
        let mut count = 0;
        for line in text.lines() {
            let Some((input, expected)) = line.split_once(' ') else { continue };
            let input = String::from_utf8(unhex(input)).unwrap();
            let expected = String::from_utf8(unhex(expected)).unwrap();
            assert_eq!(normalize(&input).as_deref(), Some(expected.as_str()), "line {}", count + 1);
            assert_eq!(is_normalized(&input, 1024), input == expected, "line {}", count + 1);
            count += 1;
        }
        assert!(count > 5000);
    }

    #[test]
    fn source_name_examples() {
        assert!(is_normalized("Camera \u{1f600}", 64));
        assert!(!is_normalized("Cafe\u{301}", 64));
        assert!(is_normalized("Caf\u{e9}", 64));
        assert!(!is_normalized("abc", 2));
    }
}
