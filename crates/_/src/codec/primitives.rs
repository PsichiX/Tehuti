use crate::{buffer::Buffer, codec::Codec};
use std::{
    error::Error,
    io::{Read, Write},
};

impl Codec for bool {
    type Value = bool;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let byte = if *message { 1u8 } else { 0u8 };
        buffer.write_all(&[byte])?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut byte_buf = [0u8; 1];
        buffer.read_exact(&mut byte_buf)?;
        Ok(byte_buf[0] != 0)
    }
}

macro_rules! impl_codec_for_pod {
    ($($t:ty => $mode:ident),* $(,)?) => {
        $(
            impl Codec for $t {
                type Value = $t;

                fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
                    buffer.write_all(&message.to_le_bytes())?;
                    leb128::write::$mode(buffer, *message as _)?;
                    Ok(())
                }

                fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
                    Ok(leb128::read::$mode(buffer)? as _)
                }
            }
        )*
    };
}

impl_codec_for_pod!(
    u16 => unsigned,
    u32 => unsigned,
    u64 => unsigned,
    u128 => unsigned,
    usize => unsigned,
    i16 => signed,
    i32 => signed,
    i64 => signed,
    i128 => signed,
    isize => signed,
);

impl Codec for u8 {
    type Value = u8;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&message.to_le_bytes())?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut byte = [0u8; 1];
        buffer.read_exact(&mut byte)?;
        Ok(u8::from_le_bytes(byte))
    }
}

impl Codec for i8 {
    type Value = i8;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&message.to_le_bytes())?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut byte = [0u8; 1];
        buffer.read_exact(&mut byte)?;
        Ok(i8::from_le_bytes(byte))
    }
}

impl Codec for f32 {
    type Value = f32;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&message.to_le_bytes())?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut bytes = [0u8; 4];
        buffer.read_exact(&mut bytes)?;
        Ok(f32::from_le_bytes(bytes))
    }
}

impl Codec for f64 {
    type Value = f64;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&message.to_le_bytes())?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut bytes = [0u8; 8];
        buffer.read_exact(&mut bytes)?;
        Ok(f64::from_le_bytes(bytes))
    }
}

impl Codec for char {
    type Value = char;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let code = *message as u32;
        buffer.write_all(&code.to_le_bytes())?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut buf = [0u8; std::mem::size_of::<u32>()];
        buffer.read_exact(&mut buf)?;
        let code = u32::from_le_bytes(buf);
        Ok(std::char::from_u32(code).ok_or("Invalid char code")?)
    }
}

impl Codec for String {
    type Value = String;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let bytes = message.as_bytes();
        buffer.write_all(&(bytes.len() as u64).to_le_bytes())?;
        buffer.write_all(bytes)?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut data = vec![0u8; len];
        buffer.read_exact(&mut data)?;
        Ok(String::from_utf8(data)?)
    }
}
