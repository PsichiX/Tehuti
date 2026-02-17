use crate::{buffer::Buffer, codec::Codec, replication::Replicable};
use std::{error::Error, marker::PhantomData};

pub struct RepCodec<T>(PhantomData<fn() -> T>);

impl<T> Codec for RepCodec<T>
where
    T: Replicable + Default,
{
    type Value = T;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        message.collect_changes(buffer)?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut message = T::default();
        message.apply_changes(buffer)?;
        Ok(message)
    }
}
