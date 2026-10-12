//! Conversions between zatoshi and stablecoin base units at a USD/ZEC price, with the reference's exact rounding.
//!
//! The price is a JavaScript number (`f64`) in the reference, and so it is here: it is first turned into an integer
//! "micro-rate" `floor(usdPerZec × 1_000_000)` (the same IEEE multiplication JavaScript does), and everything after
//! that is exact integer arithmetic (512-bit, so nothing overflows). [`zat_to_usd_units`] rounds down (the payer of
//! ZEC never gets more than the price); [`usd_units_to_zat`] rounds up (a payment recipient always gets the full
//! amount).
//!
//! Differences from the reference, for absurd inputs only: prices of `2^128 / 10^6` USD/ZEC or more, more than 77
//! decimals, and results above `u128::MAX` are rejected with `invalid_request` instead of producing a bigint.

use k256::elliptic_curve::bigint::{CheckedMul, Encoding, NonZero, U512};

use super::error::{ParalockError, ParalockErrorCode};
use crate::jsnum;

const ZAT_PER_ZEC: u128 = 100_000_000;
const MICRO: u128 = 1_000_000;
const MAX_DECIMALS: u32 = 77;

fn micro_rate(usd_per_zec: f64) -> Result<u128, ParalockError> {
    if !usd_per_zec.is_finite() || usd_per_zec <= 0.0 {
        return Err(ParalockError::invalid_request(format!("invalid price {}", jsnum::format(usd_per_zec))));
    }
    let scaled = (usd_per_zec * 1_000_000.0).floor();
    // 2^128 as f64; `as u128` below is exact for integral values under it.
    if scaled >= 340_282_366_920_938_463_463_374_607_431_768_211_456.0 {
        return Err(ParalockError::invalid_request(format!("price {} is too large", jsnum::format(usd_per_zec))));
    }
    let rate = scaled as u128;
    if rate == 0 {
        return Err(ParalockError::invalid_request(format!("price {} is too small", jsnum::format(usd_per_zec))));
    }
    Ok(rate)
}

fn big(n: u128) -> U512 {
    U512::from_u128(n)
}

fn pow10(decimals: u32) -> Result<U512, ParalockError> {
    if decimals > MAX_DECIMALS {
        return Err(ParalockError::invalid_request(format!("{decimals} decimals is too many")));
    }
    let mut out = big(1);
    for _ in 0..decimals {
        out = mul(&out, &big(10))?;
    }
    Ok(out)
}

fn overflow() -> ParalockError {
    ParalockError::invalid_request("amount out of range")
}

fn mul(a: &U512, b: &U512) -> Result<U512, ParalockError> {
    Option::<U512>::from(a.checked_mul(b)).ok_or_else(overflow)
}

fn div(a: &U512, b: &U512) -> Result<U512, ParalockError> {
    let divisor = Option::<NonZero<U512>>::from(NonZero::new(*b)).ok_or_else(overflow)?;
    Ok(a.div_rem(&divisor).0)
}

fn to_u128(n: &U512) -> Result<u128, ParalockError> {
    let bytes = n.to_be_bytes();
    let (high, low) = bytes.split_at(48);
    if high.iter().any(|&b| b != 0) {
        return Err(overflow());
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(low);
    Ok(u128::from_be_bytes(out))
}

/// Converts zatoshi to stablecoin base units at `usd_per_zec`, rounding down (`zatToUsdUnits`):
/// `zat × microRate × 10^decimals / (10^8 × 10^6)`.
pub fn zat_to_usd_units(zat: u64, usd_per_zec: f64, decimals: u32) -> Result<u128, ParalockError> {
    let rate = micro_rate(usd_per_zec)?;
    let numerator = mul(&mul(&big(u128::from(zat)), &big(rate))?, &pow10(decimals)?)?;
    to_u128(&div(&numerator, &big(ZAT_PER_ZEC * MICRO))?)
}

/// ZEC (zatoshi) needed to deliver `usd_units` at `usd_per_zec`, rounding up so the recipient always gets the full
/// amount (`usdUnitsToZat`): `ceil(usdUnits × 10^8 × 10^6 / (microRate × 10^decimals))`.
pub fn usd_units_to_zat(usd_units: u128, usd_per_zec: f64, decimals: u32) -> Result<u128, ParalockError> {
    let rate = micro_rate(usd_per_zec)?;
    let numerator = mul(&big(usd_units), &big(ZAT_PER_ZEC * MICRO))?;
    let denominator = mul(&big(rate), &pow10(decimals)?)?;
    let rounded_up = numerator.wrapping_add(&denominator).wrapping_sub(&big(1));
    to_u128(&div(&rounded_up, &denominator)?)
}

/// The rate a maker quotes at: `price × (1 − spreadBps / 10 000)`, in `f64` exactly as the reference computes it.
pub fn maker_rate(price: f64, spread_bps: f64) -> f64 {
    price * (1.0 - spread_bps / 10_000.0)
}

/// Stablecoin units a maker quotes for `zat` (exact-in): `zat − zcash_fee_zat` (the maker's own claim fee) converted
/// at `rate`, rounded down. Fails with `limit_exceeded` ("amount too small") when that is 0.
pub fn quote_exact_in(zat: u64, rate: f64, zcash_fee_zat: u64, decimals: u32) -> Result<u128, ParalockError> {
    let usd = zat_to_usd_units(zat.saturating_sub(zcash_fee_zat), rate, decimals)?;
    if usd == 0 {
        return Err(ParalockError::new(ParalockErrorCode::LimitExceeded, "amount too small"));
    }
    Ok(usd)
}

/// Zatoshi a maker quotes to deliver exactly `usd_units` (exact-out): rounded up, plus `zcash_fee_zat`.
pub fn quote_exact_out(usd_units: u128, rate: f64, zcash_fee_zat: u64, decimals: u32) -> Result<u64, ParalockError> {
    let zat = usd_units_to_zat(usd_units, rate, decimals)?;
    u64::try_from(zat).ok().and_then(|z| z.checked_add(zcash_fee_zat)).ok_or_else(overflow)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    #[test]
    fn rounds_like_the_reference() {
        // 1 ZEC at $40 = 40 USDC (6 decimals).
        assert_eq!(zat_to_usd_units(100_000_000, 40.0, 6).unwrap(), 40_000_000);
        assert_eq!(usd_units_to_zat(40_000_000, 40.0, 6).unwrap(), 100_000_000);
        // Rounding: 1 zat at $40 is 0.4 micro-dollars → 0; 1 micro-dollar at $40 needs 2.5 zat → 3.
        assert_eq!(zat_to_usd_units(1, 40.0, 6).unwrap(), 0);
        assert_eq!(usd_units_to_zat(1, 40.0, 6).unwrap(), 3);
        // The micro-rate floors: $39.9999999 is 39_999_999 micro-dollars.
        assert_eq!(zat_to_usd_units(100_000_000, 39.999_999_9, 6).unwrap(), 39_999_999);
        // The IEEE product, not the decimal value: 1.005 × 10^6 = 1004999.9999999999 in binary floating point
        // (JavaScript gives the same), so the micro-rate is 1_004_999.
        assert_eq!(zat_to_usd_units(100_000_000, 1.005, 6).unwrap(), 1_004_999);
        assert_eq!(zat_to_usd_units(100_000_000, 0.29, 6).unwrap(), 290_000);
        // 18 decimals.
        assert_eq!(zat_to_usd_units(100_000_000, 40.0, 18).unwrap(), 40_000_000_000_000_000_000);
        // Largest quoted amounts don't overflow.
        assert!(usd_units_to_zat(10u128.pow(30), 0.000_001, 6).is_ok());
    }

    #[test]
    fn rejects_invalid_prices() {
        assert_eq!(zat_to_usd_units(1, 0.0, 6).unwrap_err().message, "invalid price 0");
        assert_eq!(zat_to_usd_units(1, f64::NAN, 6).unwrap_err().message, "invalid price NaN");
        assert_eq!(zat_to_usd_units(1, -2.5, 6).unwrap_err().message, "invalid price -2.5");
        assert_eq!(zat_to_usd_units(1, f64::INFINITY, 6).unwrap_err().message, "invalid price Infinity");
        assert_eq!(zat_to_usd_units(1, 1e-7, 6).unwrap_err().message, "price 1e-7 is too small");
        assert!(zat_to_usd_units(1, 1e40, 6).is_err());
    }
}
