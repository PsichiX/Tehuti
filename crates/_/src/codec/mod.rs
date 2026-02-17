#[cfg(feature = "postcard")]
pub mod postcard;
pub mod raw_bytes;
pub mod replicable;

use crate::buffer::Buffer;
use std::{error::Error, marker::PhantomData};

pub trait Codec {
    type Value;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>>;
    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>>;
}

impl<T> Codec for PhantomData<T> {
    type Value = PhantomData<T>;

    fn encode(_: &Self::Value, _: &mut Buffer) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn decode(_: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        Ok(PhantomData)
    }
}

impl Codec for () {
    type Value = ();

    fn encode(_: &Self::Value, _: &mut Buffer) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn decode(_: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        Ok(())
    }
}

macro_rules! impl_codec_tuple {
    ( $($id:ident),+ ) => {
        #[allow(non_snake_case)]
        impl< $($id),+ > Codec for ( $($id,)+ )
        where
            $( $id: Codec, )+
        {
            type Value = ( $($id::Value,)+ );

            fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
                let ( $($id,)+ ) = message;
                $(
                    $id::encode(&$id, buffer)?;
                )+
                Ok(())
            }

            fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
                Ok((
                    $(
                        $id::decode(buffer)?,
                    )+
                ))
            }
        }
    };
}

impl_codec_tuple!(A);
impl_codec_tuple!(A, B);
impl_codec_tuple!(A, B, C);
impl_codec_tuple!(A, B, C, D);
impl_codec_tuple!(A, B, C, D, E);
impl_codec_tuple!(A, B, C, D, E, F);
impl_codec_tuple!(A, B, C, D, E, F, G);
impl_codec_tuple!(A, B, C, D, E, F, G, H);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I, J);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I, J, K);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
impl_codec_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);
