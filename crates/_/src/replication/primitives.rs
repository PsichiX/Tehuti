use crate::replication::{BufferRead, BufferWrite, Replicable};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    hash::{Hash, Hasher},
    io::{Read, Write},
    ops::{Deref, DerefMut},
};

impl Replicable for bool {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        let byte = if *self { 1u8 } else { 0u8 };
        buffer.write_all(&[byte])?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        let mut byte = [0u8; 1];
        buffer.read_exact(&mut byte)?;
        *self = byte[0] != 0;
        Ok(())
    }
}

// TODO: use leb128 varints encoding for better compression of small numbers!
// This will remove the need for separate Rep var int wrappers and make it more
// ergonomic to use primitive types directly as replicables.
macro_rules! impl_replicable_for_pod {
    ($($t:ty => $mode:ident),* $(,)?) => {
        $(
            impl Replicable for $t {
                fn collect_changes(
                    &self,
                    buffer: &mut $crate::replication::BufferWrite,
                ) -> Result<(), Box<dyn Error>> {
                    $crate::third_party::leb128::write::$mode(buffer, *self as _)?;
                    Ok(())
                }

                fn apply_changes(
                    &mut self,
                    buffer: &mut $crate::replication::BufferRead,
                ) -> Result<(), Box<dyn Error>> {
                    *self = $crate::third_party::leb128::read::$mode(buffer)? as _;
                    Ok(())
                }
            }
        )*
    };
}

impl_replicable_for_pod!(
    u8 => unsigned,
    u16 => unsigned,
    u32 => unsigned,
    u64 => unsigned,
    u128 => unsigned,
    usize => unsigned,
    i8 => signed,
    i16 => signed,
    i32 => signed,
    i64 => signed,
    i128 => signed,
    isize => signed,
);

impl Replicable for f32 {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        self.to_bits().collect_changes(buffer)
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        let mut bits = 0u32;
        bits.apply_changes(buffer)?;
        *self = f32::from_bits(bits);
        Ok(())
    }
}

impl Replicable for f64 {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        self.to_bits().collect_changes(buffer)
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        let mut bits = 0u64;
        bits.apply_changes(buffer)?;
        *self = f64::from_bits(bits);
        Ok(())
    }
}

impl Replicable for char {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        let code = *self as u32;
        buffer.write_all(&code.to_le_bytes())?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        let mut buf = [0u8; std::mem::size_of::<u32>()];
        buffer.read_exact(&mut buf)?;
        let code = u32::from_le_bytes(buf);
        *self = std::char::from_u32(code).ok_or("Invalid char code")?;
        Ok(())
    }
}

impl Replicable for String {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        let bytes = self.as_bytes();
        let len = bytes.len() as u64;
        buffer.write_all(&len.to_le_bytes())?;
        buffer.write_all(bytes)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut bytes = vec![0u8; len];
        buffer.read_exact(&mut bytes)?;
        *self = String::from_utf8(bytes).map_err(|_| "Invalid UTF-8 string")?;
        Ok(())
    }
}

macro_rules! impl_rep_float {
    ($( $wrap:ident => $type:ident ),+) => {
        $(
            #[derive(Debug, Default, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
            pub struct $wrap(pub $type);

            impl From<$type> for $wrap {
                fn from(value: $type) -> Self {
                    Self(value)
                }
            }

            impl From<$wrap> for $type {
                fn from(value: $wrap) -> Self {
                    value.0
                }
            }

            impl Deref for $wrap {
                type Target = $type;

                fn deref(&self) -> &Self::Target {
                    &self.0
                }
            }

            impl DerefMut for $wrap {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self.0
                }
            }

            impl Hash for $wrap {
                fn hash<H: Hasher>(&self, state: &mut H) {
                    self.0.to_bits().hash(state);
                }
            }

            impl $crate::replication::Replicable for $wrap {
                fn collect_changes(
                    &self,
                    buffer: &mut $crate::replication::BufferWrite,
                ) -> Result<(), Box<dyn Error>> {
                    self.0.collect_changes(buffer)?;
                    Ok(())
                }

                fn apply_changes(
                    &mut self,
                    buffer: &mut $crate::replication::BufferRead,
                ) -> Result<(), Box<dyn Error>> {
                    self.0.apply_changes(buffer)?;
                    Ok(())
                }
            }
        )+
    };
}
impl_rep_float!(RepF32 => f32, RepF64 => f64);

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct RepChar(pub char);

impl From<char> for RepChar {
    fn from(value: char) -> Self {
        Self(value)
    }
}

impl From<RepChar> for char {
    fn from(value: RepChar) -> Self {
        value.0
    }
}

impl Deref for RepChar {
    type Target = char;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RepChar {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Replicable for RepChar {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        leb128::write::unsigned(buffer, self.0 as u64)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        self.0 =
            std::char::from_u32(leb128::read::unsigned(buffer)? as u32).ok_or("Invalid char")?;
        Ok(())
    }
}
