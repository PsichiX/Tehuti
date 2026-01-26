use crate::codec::Codec;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    error::Error,
    io::{Read, Write},
    marker::PhantomData,
};

pub struct PostcardCodec<T>(PhantomData<T>);

impl<T: Serialize + DeserializeOwned> Codec for PostcardCodec<T> {
    type Value = T;

    fn encode(message: &T, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        let data = postcard::to_allocvec(message)?;
        buffer.write_all(&(data.len() as u64).to_be_bytes())?;
        buffer.write_all(&data)?;
        Ok(())
    }

    fn decode(buffer: &mut dyn Read) -> Result<T, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_be_bytes(len_buf) as usize;
        let mut data = vec![0u8; len];
        buffer.read_exact(&mut data)?;
        Ok(postcard::from_bytes::<T>(&data)?)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[test]
    fn test_postcard_codec() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct TestMessage {
            id: u32,
        }

        let message = TestMessage { id: 42 };
        let mut buffer = Vec::new();

        PostcardCodec::<TestMessage>::encode(&message, &mut buffer).unwrap();
        let decoded_message = PostcardCodec::<TestMessage>::decode(&mut buffer.as_slice()).unwrap();

        assert_eq!(message, decoded_message);
    }
}
