use crate::replication::Replicable;
use std::{
    error::Error,
    io::{Read, Write},
};

impl Replicable for bool {
    fn collect_changes(&self, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let byte = if *self { 1u8 } else { 0u8 };
        buffer.write_all(&[byte])?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut dyn Read) -> Result<(), Box<dyn Error>> {
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
                fn collect_changes(&self, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
                    buffer.write_all(&self.to_le_bytes())?;
                    Ok(())
                }

                fn apply_changes(&mut self, buffer: &mut dyn Read) -> Result<(), Box<dyn Error>> {
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
    fn collect_changes(&self, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let code = *self as u32;
        buffer.write_all(&code.to_le_bytes())?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut dyn Read) -> Result<(), Box<dyn Error>> {
        let mut buf = [0u8; std::mem::size_of::<u32>()];
        buffer.read_exact(&mut buf)?;
        let code = u32::from_le_bytes(buf);
        *self = std::char::from_u32(code).ok_or("Invalid char code")?;
        Ok(())
    }
}

impl Replicable for String {
    fn collect_changes(&self, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let bytes = self.as_bytes();
        let len = bytes.len() as u64;
        buffer.write_all(&len.to_le_bytes())?;
        buffer.write_all(bytes)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut dyn Read) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut bytes = vec![0u8; len];
        buffer.read_exact(&mut bytes)?;
        *self = String::from_utf8(bytes).map_err(|_| "Invalid UTF-8 string")?;
        Ok(())
    }
}
