use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Rem, RemAssign, Sub, SubAssign},
    str::FromStr,
};

use crate::{
    buffer::Buffer,
    codec::{Codec, replicable::RepCodec},
    replication::Replicable,
};

/// Macro to create a Fixed-point number.
///
/// # Example
/// ```ignore
/// let a = fixed!(2: 1,50);   // Creates Fixed::<2> with value 1.50
/// let b = fixed!(2: 2,25);   // Creates Fixed::<2> with value 2.25
/// let c = fixed!(1: 3,5);    // Creates Fixed::<1> with value 3.5
/// ```
#[macro_export]
macro_rules! fixed {
    ($n:literal : $integer:literal , $fractional:literal) => {{ Fixed::<{ $n as u8 }>::new($integer as i128, $fractional as i128) }};
}

#[derive(Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Fixed<const N: u8>(i128);

impl<const N: u8> Fixed<N> {
    pub const DIGITS: u8 = N;
    pub const SCALE: i128 = Self::calculate_scale(N);
    pub const MAX: Self = Self(i128::MAX);
    pub const MIN: Self = Self(i128::MIN);
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(Self::SCALE);
    pub const NEG_ONE: Self = Self(-Self::SCALE);
    pub const EPSILON: Self = Self(1);

    const fn calculate_scale(n: u8) -> i128 {
        let mut i = 0;
        let mut v = 1i128;
        while i < n {
            v *= 10;
            i += 1;
        }
        v
    }

    pub const fn new(integer: i128, mut fractional: i128) -> Self {
        if fractional < 0 {
            panic!("fractional part cannot be negative");
        }
        if fractional >= Self::SCALE {
            fractional = Self::SCALE - 1;
        }
        Self(integer * Self::SCALE + fractional)
    }

    pub const fn from_inner(inner: i128) -> Self {
        Self(inner)
    }

    pub const fn into_inner(self) -> i128 {
        self.0
    }

    pub const fn inner(&self) -> &i128 {
        &self.0
    }

    pub const fn inner_mut(&mut self) -> &mut i128 {
        &mut self.0
    }

    pub const fn from_i8(value: i8) -> Self {
        Self(
            (value as i128)
                .checked_mul(Self::SCALE)
                .expect("from_i8 overflow"),
        )
    }

    pub const fn into_i8(self) -> i8 {
        self.0.checked_div(Self::SCALE).expect("into_i8 overflow") as i8
    }

    pub const fn from_i16(value: i16) -> Self {
        Self(
            (value as i128)
                .checked_mul(Self::SCALE)
                .expect("from_i16 overflow"),
        )
    }

    pub const fn into_i16(self) -> i16 {
        self.0.checked_div(Self::SCALE).expect("into_i16 overflow") as i16
    }

    pub const fn from_i32(value: i32) -> Self {
        Self(
            (value as i128)
                .checked_mul(Self::SCALE)
                .expect("from_i32 overflow"),
        )
    }

    pub const fn into_i32(self) -> i32 {
        self.0.checked_div(Self::SCALE).expect("into_i32 overflow") as i32
    }

    pub const fn from_i64(value: i64) -> Self {
        Self(
            (value as i128)
                .checked_mul(Self::SCALE)
                .expect("from_i64 overflow"),
        )
    }

    pub const fn into_i64(self) -> i64 {
        self.0.checked_div(Self::SCALE).expect("into_i64 overflow") as i64
    }

    pub const fn from_i128(value: i128) -> Self {
        Self(value.checked_mul(Self::SCALE).expect("from_i128 overflow"))
    }

    pub const fn into_i128(self) -> i128 {
        self.0.checked_div(Self::SCALE).expect("into_i128 overflow")
    }

    pub const fn from_f32(value: f32) -> Self {
        Self((value * Self::SCALE as f32).round() as i128)
    }

    pub const fn into_f32(self) -> f32 {
        self.0 as f32 / Self::SCALE as f32
    }

    pub const fn from_f64(value: f64) -> Self {
        Self((value * Self::SCALE as f64).round() as i128)
    }

    pub const fn into_f64(self) -> f64 {
        self.0 as f64 / Self::SCALE as f64
    }

    pub const fn is_zero(&self) -> bool {
        self.0 == 0
    }

    pub const fn is_positive(&self) -> bool {
        self.0 > 0
    }

    pub const fn is_negative(&self) -> bool {
        self.0 < 0
    }

    pub const fn is_integer(&self) -> bool {
        self.0 % Self::SCALE == 0
    }

    pub const fn is_fractional(&self) -> bool {
        self.0 % Self::SCALE != 0
    }

    pub const fn integer_part(&self) -> i128 {
        self.0 / Self::SCALE
    }

    pub const fn fractional_part(&self) -> i128 {
        self.0 % Self::SCALE
    }

    pub const fn abs(self) -> Self {
        Self(self.0.abs())
    }

    pub const fn signum(self) -> Self {
        Self(self.0.signum() * Self::SCALE)
    }

    pub fn min(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }

    pub fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }

    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self(self.0.clamp(min.0, max.0))
    }

    pub const fn round(self) -> Self {
        let fractional = self.fractional_part();
        if fractional.abs() >= Self::SCALE / 2 {
            if self.0 >= 0 {
                Self(self.0 + (Self::SCALE - fractional))
            } else {
                Self(self.0 - (Self::SCALE + fractional))
            }
        } else {
            Self(self.0 - fractional)
        }
    }

    pub const fn floor(self) -> Self {
        let fractional = self.fractional_part();
        if self.0 >= 0 || fractional == 0 {
            Self(self.0 - fractional)
        } else {
            Self(self.0 - (Self::SCALE + fractional))
        }
    }

    pub const fn ceil(self) -> Self {
        let fractional = self.fractional_part();
        if self.0 <= 0 || fractional == 0 {
            Self(self.0 - fractional)
        } else {
            Self(self.0 + (Self::SCALE - fractional))
        }
    }

    pub const fn fract(self) -> Self {
        Self(self.0 % Self::SCALE)
    }

    pub const fn trunc(self) -> Self {
        Self(self.0 - self.fractional_part())
    }
}

impl<const N: u8> FromStr for Fixed<N> {
    type Err = Box<dyn Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('.');
        let integer_part = match parts.next() {
            Some(part) => part.parse::<i128>()?,
            None => 0,
        };
        let fractional_part = match parts.next() {
            Some(part) => {
                let mut frac = part.parse::<i128>()?;
                let shift = N.saturating_sub(part.len() as u8);
                for _ in 0..shift {
                    frac *= 10;
                }
                frac
            }
            None => 0,
        };
        Ok(Self::new(integer_part, fractional_part))
    }
}

impl<const N: u8> std::fmt::Display for Fixed<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.abs();
        let integer_part = abs / Self::SCALE;
        let fractional_part = abs % Self::SCALE;
        if N == 0 {
            write!(f, "{}{}", sign, integer_part)
        } else {
            write!(
                f,
                "{}{}.{:0width$}",
                sign,
                integer_part,
                fractional_part,
                width = N as usize
            )
        }
    }
}

impl<const N: u8> std::fmt::Debug for Fixed<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl<const N: u8> Replicable for Fixed<N> {
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.to_le_bytes().collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl<const N: u8> Codec for Fixed<N> {
    type Value = Self;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        RepCodec::<i128>::encode(&message.0, buffer)
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        Ok(Self(RepCodec::<i128>::decode(buffer)?))
    }
}

impl<const N: u8> Neg for Fixed<N> {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl<const N: u8> Add for Fixed<N> {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self(self.0 + other.0)
    }
}

impl<const N: u8> AddAssign for Fixed<N> {
    fn add_assign(&mut self, other: Self) {
        self.0 += other.0;
    }
}

impl<const N: u8> Sub for Fixed<N> {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self(self.0 - other.0)
    }
}

impl<const N: u8> SubAssign for Fixed<N> {
    fn sub_assign(&mut self, other: Self) {
        self.0 -= other.0;
    }
}

impl<const N: u8> Mul for Fixed<N> {
    type Output = Self;

    fn mul(self, other: Self) -> Self::Output {
        #[allow(clippy::suspicious_arithmetic_impl)]
        Self(self.0.checked_mul(other.0).expect("mul overflow") / Self::SCALE)
    }
}

impl<const N: u8> MulAssign for Fixed<N> {
    fn mul_assign(&mut self, other: Self) {
        *self = *self * other;
    }
}

impl<const N: u8> Div for Fixed<N> {
    type Output = Self;

    fn div(self, other: Self) -> Self::Output {
        assert!(other.0 != 0, "div by zero");
        Self(self.0.checked_mul(Self::SCALE).expect("div overflow") / other.0)
    }
}

impl<const N: u8> DivAssign for Fixed<N> {
    fn div_assign(&mut self, other: Self) {
        *self = *self / other;
    }
}

impl<const N: u8> Rem for Fixed<N> {
    type Output = Self;

    fn rem(self, other: Self) -> Self::Output {
        assert!(other.0 != 0, "div by zero");
        Self(self.0 % other.0)
    }
}

impl<const N: u8> RemAssign for Fixed<N> {
    fn rem_assign(&mut self, other: Self) {
        *self = *self % other;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_basics() {
        let a = Fixed::<4>::from_f64(1.5);
        assert_eq!(a.into_inner(), 15000);
        assert_eq!(a.to_string(), "1.5000");

        let b = Fixed::<4>::from_f64(2.25);
        assert_eq!(b.into_inner(), 22500);
        assert_eq!(b.to_string(), "2.2500");

        let c = a + b;
        assert_eq!(c.into_inner(), 37500);
        assert_eq!(c.to_string(), "3.7500");

        let d = c - a;
        assert_eq!(d.into_inner(), 22500);
        assert_eq!(d.to_string(), "2.2500");

        let e = a * b;
        assert_eq!(e.into_inner(), 33750);
        assert_eq!(e.to_string(), "3.3750");

        let f = e / a;
        assert_eq!(f.into_inner(), 22500);
        assert_eq!(f.to_string(), "2.2500");

        let v = Fixed::<4>::new(3, 1415);
        assert_eq!(v.into_inner(), 31415);
        assert_eq!(v.to_string(), "3.1415");

        let v = "4".parse::<Fixed<4>>().unwrap();
        assert_eq!(v.into_inner(), 40000);
        assert_eq!(v.to_string(), "4.0000");

        let v = "4.1000".parse::<Fixed<4>>().unwrap();
        assert_eq!(v.into_inner(), 41000);
        assert_eq!(v.to_string(), "4.1000");
    }

    #[test]
    fn test_fixed_ops() {
        let a = Fixed::<2>::from_f64(1.5);
        let b = Fixed::<2>::from_f64(2.25);

        assert_eq!(a.into_f64(), 1.5);
        assert_eq!(b.into_f64(), 2.25);
        assert_eq!((-a).to_string(), "-1.50");
        assert_eq!((a + b).to_string(), "3.75");
        assert_eq!((a - b).to_string(), "-0.75");
        assert_eq!((a * b).to_string(), "3.37");
        assert_eq!((b / a).to_string(), "1.50");
        assert_eq!((b % a).to_string(), "0.75");
    }

    #[test]
    fn test_fixed_functions() {
        let a = Fixed::<2>::from_f64(1.5);
        let b = Fixed::<2>::from_f64(2.25);

        assert_eq!(a.abs().to_string(), "1.50");
        assert_eq!((-a).abs().to_string(), "1.50");
        assert_eq!(a.signum().to_string(), "1.00");
        assert_eq!((-a).signum().to_string(), "-1.00");
        assert_eq!(a.min(b).to_string(), "1.50");
        assert_eq!(a.max(b).to_string(), "2.25");
        assert_eq!(
            a.clamp(Fixed::<2>::from_f64(1.0), Fixed::<2>::from_f64(2.0))
                .to_string(),
            "1.50"
        );
        assert_eq!(
            b.clamp(Fixed::<2>::from_f64(1.0), Fixed::<2>::from_f64(2.0))
                .to_string(),
            "2.00"
        );
        assert_eq!(a.round().to_string(), "2.00");
        assert_eq!(b.round().to_string(), "2.00");
        assert_eq!(a.floor().to_string(), "1.00");
        assert_eq!(b.floor().to_string(), "2.00");
        assert_eq!(a.ceil().to_string(), "2.00");
        assert_eq!(b.ceil().to_string(), "3.00");
    }

    #[test]
    fn test_fixed_macro() {
        let a = fixed!(2: 1, 50);
        assert_eq!(a.into_inner(), 150);
        assert_eq!(a.to_string(), "1.50");

        let b = fixed!(2: 2, 25);
        assert_eq!(b.into_inner(), 225);
        assert_eq!(b.to_string(), "2.25");

        let c = fixed!(1: 3, 5);
        assert_eq!(c.into_inner(), 35);
        assert_eq!(c.to_string(), "3.5");

        let c = fixed!(3: 3, 5);
        assert_eq!(c.into_inner(), 3005);
        assert_eq!(c.to_string(), "3.005");
    }
}
