use crate::{
    buffer::Buffer,
    codec::Codec,
    replication::{CodecReplicated, HashRep, ManRep, MutRep},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    error::Error,
    io::{Read, Write},
    marker::PhantomData,
};

pub type PostcardReplicated<P, T> = CodecReplicated<P, T, PostcardCodec<T>>;
pub type FullPostcardReplicated<T> = PostcardReplicated<(), T>;
pub type HashPostcardReplicated<T> = PostcardReplicated<HashRep, T>;
pub type MutPostcardReplicated<T> = PostcardReplicated<MutRep, T>;
pub type ManPostcardReplicated<T> = PostcardReplicated<ManRep, T>;

pub struct PostcardCodec<T>(PhantomData<T>);

impl<T: Serialize + DeserializeOwned> Codec for PostcardCodec<T> {
    type Value = T;

    fn encode(message: &T, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let data = postcard::to_allocvec(message)?;
        buffer.write_all(&(data.len() as u64).to_le_bytes())?;
        buffer.write_all(&data)?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<T, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut data = vec![0u8; len];
        buffer.read_exact(&mut data)?;
        Ok(postcard::from_bytes::<T>(&data)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::io::Cursor;

    #[test]
    fn test_postcard_codec() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct TestMessage {
            id: u32,
        }

        let message = TestMessage { id: 42 };
        let mut buffer = Cursor::new(Vec::new());
        PostcardCodec::<TestMessage>::encode(&message, &mut buffer).unwrap();

        let mut buffer = Cursor::new(buffer.into_inner());
        let decoded_message = PostcardCodec::<TestMessage>::decode(&mut buffer).unwrap();

        assert_eq!(message, decoded_message);
    }
}
