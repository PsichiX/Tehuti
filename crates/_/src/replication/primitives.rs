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

macro_rules! impl_replicable_for_pod {
    ($($t:ty),* $(,)?) => {
        $(
            impl Replicable for $t {
                fn collect_changes(&self, buffer: &mut $crate::replication::BufferWrite) -> Result<(), Box<dyn Error>> {
                    buffer.write_all(&self.to_le_bytes())?;
                    Ok(())
                }

                fn apply_changes(&mut self, buffer: &mut $crate::replication::BufferRead) -> Result<(), Box<dyn Error>> {
                    let mut buf = [0u8; std::mem::size_of::<$t>()];
                    buffer.read_exact(&mut buf)?;
                    *self = <$t>::from_le_bytes(buf);
                    Ok(())
                }
            }
        )*
    };
}

impl_replicable_for_pod!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

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

macro_rules! impl_float {
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
impl_float!(RepF32 => f32, RepF64 => f64);

macro_rules! impl_var_int {
    ($( $wrap:ident => $type:ident => $mode:ident ),+) => {
        $(
            #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

            impl $crate::replication::Replicable for $wrap {
                fn collect_changes(
                    &self,
                    buffer: &mut $crate::replication::BufferWrite,
                ) -> Result<(), Box<dyn Error>> {
                    $crate::third_party::leb128::write::$mode(buffer, self.0 as _)?;
                    Ok(())
                }

                fn apply_changes(
                    &mut self,
                    buffer: &mut $crate::replication::BufferRead,
                ) -> Result<(), Box<dyn Error>> {
                    self.0 = $crate::third_party::leb128::read::$mode(buffer)? as _;
                    Ok(())
                }
            }
        )+
    };
}
impl_var_int!(
    RepI16 => i16 => signed,
    RepI32 => i32 => signed,
    RepI64 => i64 => signed,
    RepIsize => isize => signed,
    RepU16 => u16 => unsigned,
    RepU32 => u32 => unsigned,
    RepU64 => u64 => unsigned,
    RepUsize => usize => unsigned
);

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
