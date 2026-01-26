use crate::codec::Codec;
use std::{
    error::Error,
    io::{Read, Write},
};

pub struct StringCodec;

impl Codec for StringCodec {
    type Value = String;

    fn encode(message: &Self::Value, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let bytes = message.as_bytes();
        buffer.write_all(&(bytes.len() as u64).to_be_bytes())?;
        buffer.write_all(bytes)?;
        Ok(())
    }

    fn decode(buffer: &mut dyn Read) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_be_bytes(len_buf) as usize;
        let mut data = vec![0u8; len];
        buffer.read_exact(&mut data)?;
        Ok(String::from_utf8(data)?)
    }
}
