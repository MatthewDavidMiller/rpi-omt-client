//! Text formatting shared by the writers: the two ways this project prints a
//! floating-point number.
//!
//! Rust's `Display` for `f64` is the format the human-facing text (status
//! details, runtime settings) uses. JSON documents use serde_json's format,
//! which ryu lays out differently: always a fraction or an exponent, and
//! scientific notation outside 1e-5..1e16. Both use the shortest digit string
//! that round-trips, which `{:e}` produces.

use std::fmt::Write;

/// Formats a finite double the way serde_json does; non-finite is `null`.
pub fn json_f64(value: f64) -> String {
    let mut out = String::new();
    write_json_f64(&mut out, value);
    out
}

pub fn write_json_f64(out: &mut String, value: f64) {
    if !value.is_finite() {
        out.push_str("null");
        return;
    }
    if value == 0.0 {
        out.push_str(if value.is_sign_negative() { "-0.0" } else { "0.0" });
        return;
    }
    let sci = format!("{value:e}");
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let (negative, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.as_str();
    let nd = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    // Position of the decimal point relative to the first digit.
    let kk = exponent.parse::<i64>().unwrap_or(0).saturating_add(1);
    if negative {
        out.push('-');
    }
    if nd <= kk && kk <= 16 {
        out.push_str(digits);
        for _ in nd..kk {
            out.push('0');
        }
        out.push_str(".0");
    } else if 0 < kk && kk <= 16 {
        let (int, frac) = digits.split_at(usize::try_from(kk).unwrap_or(0));
        out.push_str(int);
        out.push('.');
        out.push_str(frac);
    } else if -5 < kk && kk <= 0 {
        out.push_str("0.");
        for _ in kk..0 {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        let (first, rest) = digits.split_at(1);
        out.push_str(first);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        let _ = write!(out, "e{}", kk.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_f64_matches_serde() {
        let cases = [
            (60.0, "60.0"),
            (59.94, "59.94"),
            (0.5, "0.5"),
            (1e16, "1e16"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (1e-5, "0.00001"),
            (1e-6, "1e-6"),
            (-2.25, "-2.25"),
            (29.970_029_970_029_97, "29.97002997002997"),
            (123_456.789, "123456.789"),
            (1.5e-7, "1.5e-7"),
            (0.0, "0.0"),
            (f64::NAN, "null"),
        ];
        for (v, s) in cases {
            assert_eq!(json_f64(v), s, "{v}");
        }
    }

    #[test]
    fn display_is_rusts() {
        assert_eq!(format!("{}", 0.1_f64), "0.1");
        assert_eq!(format!("{}", 1e16_f64), "10000000000000000");
        assert_eq!(format!("{}", 1e-7_f64), "0.0000001");
    }
}
