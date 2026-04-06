use crate::{buffer::Buffer, codec::Codec, replication::Replicable};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    io::{Read, Write},
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Rem, RemAssign, Sub, SubAssign},
    str::FromStr,
};

#[derive(Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fixed<const N: u8>(Decimal);

impl<const N: u8> Fixed<N> {
    pub const SCALE: u8 = N;

    pub fn new(mut value: Decimal) -> Self {
        value.rescale(N.into());
        Self(value)
    }

    pub fn decimal(&self) -> Decimal {
        self.0
    }

    pub fn from_i128(value: i128) -> Self {
        Self::new(Decimal::from(value))
    }

    pub fn from_i64(value: i64) -> Self {
        Self::new(Decimal::from(value))
    }

    pub fn from_i32(value: i32) -> Self {
        Self::new(Decimal::from(value))
    }

    pub fn from_i16(value: i16) -> Self {
        Self::new(Decimal::from(value))
    }

    pub fn from_i8(value: i8) -> Self {
        Self::new(Decimal::from(value))
    }

    pub fn from_isize(value: isize) -> Self {
        Self::new(Decimal::from(value))
    }

    pub fn from_f64(value: f64) -> Self {
        Self::new(Decimal::from_f64_retain(value).unwrap())
    }

    pub fn from_f32(value: f32) -> Self {
        Self::new(Decimal::from_f32_retain(value).unwrap())
    }

    pub fn into_i128(self) -> i128 {
        i128::try_from(self.decimal()).unwrap()
    }

    pub fn into_i64(self) -> i64 {
        i64::try_from(self.decimal()).unwrap()
    }

    pub fn into_i32(self) -> i32 {
        i32::try_from(self.decimal()).unwrap()
    }

    pub fn into_i16(self) -> i16 {
        i16::try_from(self.decimal()).unwrap()
    }

    pub fn into_i8(self) -> i8 {
        i8::try_from(self.decimal()).unwrap()
    }

    pub fn into_f64(self) -> f64 {
        f64::try_from(self.decimal()).unwrap()
    }

    pub fn into_f32(self) -> f32 {
        f32::try_from(self.decimal()).unwrap()
    }
}

impl<const N: u8> Neg for Fixed<N> {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(-self.0)
    }
}

impl<const N: u8> Add for Fixed<N> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.0 + rhs.0)
    }
}

impl<const N: u8> Sub for Fixed<N> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.0 - rhs.0)
    }
}

impl<const N: u8> Mul for Fixed<N> {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self::new(self.0 * rhs.0)
    }
}

impl<const N: u8> Div for Fixed<N> {
    type Output = Self;

    fn div(self, rhs: Self) -> Self::Output {
        Self::new(self.0 / rhs.0)
    }
}

impl<const N: u8> Rem for Fixed<N> {
    type Output = Self;

    fn rem(self, rhs: Self) -> Self::Output {
        Self::new(self.0 % rhs.0)
    }
}

impl<const N: u8> AddAssign for Fixed<N> {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
        self.0.rescale(N.into());
    }
}

impl<const N: u8> SubAssign for Fixed<N> {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
        self.0.rescale(N.into());
    }
}

impl<const N: u8> MulAssign for Fixed<N> {
    fn mul_assign(&mut self, rhs: Self) {
        self.0 *= rhs.0;
        self.0.rescale(N.into());
    }
}

impl<const N: u8> DivAssign for Fixed<N> {
    fn div_assign(&mut self, rhs: Self) {
        self.0 /= rhs.0;
        self.0.rescale(N.into());
    }
}

impl<const N: u8> RemAssign for Fixed<N> {
    fn rem_assign(&mut self, rhs: Self) {
        self.0 %= rhs.0;
        self.0.rescale(N.into());
    }
}

impl<const N: u8> FromStr for Fixed<N> {
    type Err = Box<dyn Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(Decimal::from_str(s)?))
    }
}

impl<const N: u8> std::fmt::Display for Fixed<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<const N: u8> std::fmt::Debug for Fixed<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl<const N: u8> Replicable for Fixed<N> {
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&self.0.serialize())?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut data = [0u8; 16];
        buffer.read_exact(&mut data)?;
        *self = Self::new(Decimal::deserialize(data));
        Ok(())
    }
}

impl<const N: u8> Codec for Fixed<N> {
    type Value = Self;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&message.0.serialize())?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut data = [0u8; 16];
        buffer.read_exact(&mut data)?;
        Ok(Self::new(Decimal::deserialize(data)))
    }
}
