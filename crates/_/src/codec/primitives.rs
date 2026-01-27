use crate::codec::Codec;
use std::{
    error::Error,
    io::{Read, Write},
};

impl Codec for bool {
    type Value = bool;

    fn encode(message: &Self::Value, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let byte = if *message { 1u8 } else { 0u8 };
        buffer.write_all(&[byte])?;
        Ok(())
    }

    fn decode(buffer: &mut dyn Read) -> Result<Self::Value, Box<dyn Error>> {
        let mut byte_buf = [0u8; 1];
        buffer.read_exact(&mut byte_buf)?;
        match byte_buf[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("Invalid byte for bool".into()),
        }
    }
}

macro_rules! impl_codec_for_pod {
    ($($t:ty),* $(,)?) => {
        $(
            impl Codec for $t {
                type Value = $t;

                fn encode(message: &Self::Value, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
                    buffer.write_all(&message.to_le_bytes())?;
                    Ok(())
                }

                fn decode(buffer: &mut dyn Read) -> Result<Self::Value, Box<dyn Error>> {
                    let mut buf = [0u8; std::mem::size_of::<$t>()];
                    buffer.read_exact(&mut buf)?;
                    Ok(<$t>::from_le_bytes(buf))
                }
            }
        )*
    };
}

impl_codec_for_pod!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64
);

impl Codec for char {
    type Value = char;

    fn encode(message: &Self::Value, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let code = *message as u32;
        buffer.write_all(&code.to_le_bytes())?;
        Ok(())
    }

    fn decode(buffer: &mut dyn Read) -> Result<Self::Value, Box<dyn Error>> {
        let mut buf = [0u8; std::mem::size_of::<u32>()];
        buffer.read_exact(&mut buf)?;
        let code = u32::from_le_bytes(buf);
        match std::char::from_u32(code) {
            Some(c) => Ok(c),
            None => Err("Invalid char encoding".into()),
        }
    }
}

impl Codec for String {
    type Value = String;

    fn encode(message: &Self::Value, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let bytes = message.as_bytes();
        buffer.write_all(&(bytes.len() as u64).to_le_bytes())?;
        buffer.write_all(bytes)?;
        Ok(())
    }

    fn decode(buffer: &mut dyn Read) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut data = vec![0u8; len];
        buffer.read_exact(&mut data)?;
        Ok(String::from_utf8(data)?)
    }
}
