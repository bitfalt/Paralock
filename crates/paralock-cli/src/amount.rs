//! Exact decimal amounts: ZEC ↔ zatoshi and dollars ↔ token base units, without floating point.

use crate::error::{CliError, CliResult};

/// Zatoshi per ZEC.
pub const ZAT_PER_ZEC: u64 = 100_000_000;

/// Parses a non-negative decimal (`"0.1"`, `"25"`, `"1.000001"`) into base units with `decimals` digits. Refuses
/// signs, exponents, empty parts and more fractional digits than `decimals`.
pub fn parse_decimal(text: &str, decimals: u32, what: &str) -> CliResult<u128> {
    let bad = || {
        CliError::usage(format!(
            "{what} must be a positive decimal number with at most {decimals} decimals, got \"{text}\""
        ))
    };
    let text = text.trim();
    let (whole, frac) = text.split_once('.').unwrap_or((text, ""));
    if (whole.is_empty() && frac.is_empty())
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || frac.len() > decimals as usize
        || (text.contains('.') && frac.is_empty())
    {
        return Err(bad());
    }
    let scale = 10u128.checked_pow(decimals).ok_or_else(bad)?;
    let whole: u128 = if whole.is_empty() { 0 } else { whole.parse().map_err(|_| bad())? };
    let frac_units: u128 = if frac.is_empty() {
        0
    } else {
        let padded = format!("{frac:0<width$}", width = decimals as usize);
        padded.parse().map_err(|_| bad())?
    };
    whole.checked_mul(scale).and_then(|w| w.checked_add(frac_units)).ok_or_else(bad)
}

/// Parses an amount of ZEC into zatoshi (more than zero).
pub fn parse_zec(text: &str) -> CliResult<u64> {
    let zat = parse_decimal(text, 8, "--zec")?;
    let zat = u64::try_from(zat).map_err(|_| CliError::usage("--zec is too large"))?;
    if zat == 0 {
        return Err(CliError::usage("--zec must be more than 0"));
    }
    Ok(zat)
}

/// Parses an amount of dollars into token base units with `decimals` digits (more than zero).
pub fn parse_usd(text: &str, decimals: u32) -> CliResult<u128> {
    let units = parse_decimal(text, decimals, "--usd")?;
    if units == 0 {
        return Err(CliError::usage("--usd must be more than 0"));
    }
    Ok(units)
}

/// Zatoshi as ZEC, without trailing zeros (`10000000` → `"0.1"`).
pub fn zec(zat: u64) -> String {
    let (whole, frac) = (zat / ZAT_PER_ZEC, zat % ZAT_PER_ZEC);
    if frac == 0 { whole.to_string() } else { format!("{whole}.{}", format!("{frac:08}").trim_end_matches('0')) }
}

/// Token base units as dollars with two decimals, rounded down (`4975000` → `"4.97"` for 6 decimals).
pub fn usd(units: u128, decimals: u32) -> String {
    let scale = 10u128.saturating_pow(decimals).max(1);
    let cents = units.saturating_mul(100) / scale;
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// Token base units as an exact decimal string (`4975000` → `"4.975"` for 6 decimals).
pub fn exact(units: u128, decimals: u32) -> String {
    let scale = 10u128.saturating_pow(decimals).max(1);
    let (whole, frac) = (units / scale, units % scale);
    if frac == 0 {
        whole.to_string()
    } else {
        let digits = format!("{frac:0width$}", width = decimals as usize);
        format!("{whole}.{}", digits.trim_end_matches('0'))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn parses_exact_decimals() {
        assert_eq!(parse_zec("0.1").unwrap(), 10_000_000);
        assert_eq!(parse_zec("1").unwrap(), 100_000_000);
        assert_eq!(parse_zec(".5").unwrap(), 50_000_000);
        assert_eq!(parse_zec("0.00000001").unwrap(), 1);
        assert_eq!(parse_usd("25", 6).unwrap(), 25_000_000);
        assert_eq!(parse_usd("0.000001", 6).unwrap(), 1);
        for bad in ["", ".", "1.", "-1", "1e3", "0.000000001", "1,5", " ", "0", "0.0", "abc", "1.2.3", "+1"] {
            assert!(parse_zec(bad).is_err(), "{bad}");
        }
        assert!(parse_usd("1.0000001", 6).is_err());
        assert!(parse_decimal("340282366920938463463374607431768211456", 0, "x").is_err());
    }

    #[test]
    fn formats_amounts() {
        assert_eq!(zec(10_000_000), "0.1");
        assert_eq!(zec(123_456_789), "1.23456789");
        assert_eq!(zec(500_000_000), "5");
        assert_eq!(usd(4_975_000, 6), "4.97");
        assert_eq!(usd(5, 6), "0.00");
        assert_eq!(exact(4_975_000, 6), "4.975");
        assert_eq!(exact(25_000_000, 6), "25");
    }
}
