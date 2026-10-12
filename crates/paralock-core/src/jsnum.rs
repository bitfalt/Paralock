//! JavaScript number formatting (`Number.prototype.toString` / `JSON.stringify`) and the serde glue that writes it,
//! so floats in JSON (only a quote's `rate`) are byte-identical to what the TypeScript reference emits.

use serde::Serializer;
use serde_json::value::RawValue;

/// ECMAScript `Number::toString(x)` (radix 10) for an `f64`. Uses the shortest round-trip digits (ryu, like V8) and
/// the spec's choice between fixed and exponent notation: `40`, `39.8`, `1e+21`, `1e-7`, `0.000001`.
pub(crate) fn format(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if x == 0.0 {
        return "0".to_owned(); // also -0
    }
    let mut buf = ryu::Buffer::new();
    let shortest = buf.format_finite(x.abs());
    let (mantissa, exp) = match shortest.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (shortest, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all: String = int_part.chars().chain(frac_part.chars()).collect();
    // value = 0.<all> × 10^(len(int_part) + exp)
    let leading = all.len() - all.trim_start_matches('0').len();
    let digits = all.trim_start_matches('0').trim_end_matches('0');
    let n = int_part.len() as i32 + exp - leading as i32;
    let k = digits.len() as i32;

    let mut out = String::with_capacity(32);
    if x < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(digits);
        out.extend(core::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        let (a, b) = digits.split_at(n as usize);
        out.push_str(a);
        out.push('.');
        out.push_str(b);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(core::iter::repeat_n('0', (-n) as usize));
        out.push_str(digits);
    } else {
        let e = n - 1;
        let (first, rest) = digits.split_at(1);
        out.push_str(first);
        if !rest.is_empty() {
            out.push('.');
            out.push_str(rest);
        }
        out.push('e');
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&e.unsigned_abs().to_string());
    }
    out
}

/// Serializes an `f64` as a raw JSON number formatted like JavaScript. Non-finite values have no JSON form and fail.
pub(crate) fn serialize<S: Serializer>(x: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if !x.is_finite() {
        return Err(serde::ser::Error::custom("non-finite number"));
    }
    let raw = RawValue::from_string(format(*x)).map_err(serde::ser::Error::custom)?;
    serde::Serialize::serialize(&raw, serializer)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::format;

    #[test]
    fn formats_like_javascript() {
        let cases: &[(f64, &str)] = &[
            (40.0, "40"),
            (39.8, "39.8"),
            (-1.5, "-1.5"),
            (0.1, "0.1"),
            (0.000001, "0.000001"),
            (0.0000001, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e21, "1e+21"),
            (1.2345e22, "1.2345e+22"),
            (1e16, "10000000000000000"),
            (9007199254740993.0, "9007199254740992"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (39.800000000000004, "39.800000000000004"),
            (0.1 + 0.2, "0.30000000000000004"),
            (100.0, "100"),
            (-0.0, "0"),
            (12.5e-6, "0.0000125"),
        ];
        for (x, expected) in cases {
            assert_eq!(format(*x), *expected, "{x:e}");
        }
    }
}
