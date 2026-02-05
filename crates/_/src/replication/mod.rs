pub mod containers;
pub mod primitives;
pub mod rpc;

use crate::codec::Codec;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    error::Error,
    hash::{Hash, Hasher},
    io::{Cursor, Read, Write},
    marker::PhantomData,
    ops::{Deref, DerefMut},
    sync::Mutex,
};

pub type BufferWrite = Cursor<Vec<u8>>;
pub type BufferRead<'a> = Cursor<&'a [u8]>;
pub type HashReplicated<T> = Replicated<HashRep, T>;
pub type MutReplicated<T> = Replicated<MutRep, T>;
pub type ManReplicated<T> = Replicated<ManRep, T>;
pub type CodecReplicated<P, T, C> = Replicated<P, CodecRep<T, C>>;
pub type HashCodecReplicated<T, C> = CodecReplicated<HashRep, T, C>;
pub type MutCodecReplicated<T, C> = CodecReplicated<MutRep, T, C>;
pub type ManCodecReplicated<T, C> = CodecReplicated<ManRep, T, C>;

pub trait ReplicationPolicy<T>
where
    Self: Sized + Default,
    T: Replicable,
{
    fn detect_change(&mut self, data: &T) -> bool;

    #[allow(unused_variables)]
    fn on_mutation(&mut self, data: &T) {}
}

/// Change when data hash changes.
#[derive(Default)]
pub struct HashRep(u64);

impl<T> ReplicationPolicy<T> for HashRep
where
    T: Replicable + Hash,
{
    fn detect_change(&mut self, data: &T) -> bool {
        let old_hash = self.0;
        let hash = crate::hash(data);
        self.0 = hash;
        old_hash != self.0
    }
}

/// Change when data is potentially mutated (via DerefMut).
pub struct MutRep(bool);

impl Default for MutRep {
    fn default() -> Self {
        Self(true)
    }
}

impl<T: Replicable> ReplicationPolicy<T> for MutRep {
    fn detect_change(&mut self, _data: &T) -> bool {
        let old_state = self.0;
        self.0 = false;
        old_state
    }

    fn on_mutation(&mut self, _data: &T) {
        self.0 = true;
    }
}

/// Change when manually marked as changed.
pub struct ManRep(bool);

impl Default for ManRep {
    fn default() -> Self {
        Self(true)
    }
}

impl<T: Replicable> ReplicationPolicy<T> for ManRep {
    fn detect_change(&mut self, _data: &T) -> bool {
        let old_state = self.0;
        self.0 = false;
        old_state
    }
}

pub trait Replicable: Sized {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>>;
    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>>;
}

impl Replicable for () {
    fn collect_changes(&self, _: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn apply_changes(&mut self, _: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        Ok(())
    }
}

macro_rules! impl_replicable_tuple {
    ( $( $id:ident ),+ ) => {
        #[allow(non_snake_case)]
        impl<$( $id ),+> Replicable for ( $( $id, )+ )
        where
            $( $id: Replicable ),+
        {
            fn collect_changes(&self, buffer: &mut $crate::replication::BufferWrite) -> Result<(), Box<dyn Error>> {
                let ( $( $id, )+ ) = self;
                $(
                    $id.collect_changes(buffer)?;
                )+
                Ok(())
            }

            fn apply_changes(&mut self, buffer: &mut $crate::replication::BufferRead) -> Result<(), Box<dyn Error>> {
                let ( $( $id, )+ ) = self;
                $(
                    $id.apply_changes(buffer)?;
                )+
                Ok(())
            }
        }
    };
}

impl_replicable_tuple!(A);
impl_replicable_tuple!(A, B);
impl_replicable_tuple!(A, B, C);
impl_replicable_tuple!(A, B, C, D);
impl_replicable_tuple!(A, B, C, D, E);
impl_replicable_tuple!(A, B, C, D, E, F);
impl_replicable_tuple!(A, B, C, D, E, F, G);
impl_replicable_tuple!(A, B, C, D, E, F, G, H);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I, J);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I, J, K);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
impl_replicable_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

pub struct Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable,
{
    data: T,
    meta: Mutex<P>,
}

impl<P, T> Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable,
{
    pub fn new(data: T) -> Self {
        Self {
            meta: Mutex::new(P::default()),
            data,
        }
    }

    pub fn into_inner(self) -> T {
        self.data
    }

    pub fn collect_changes(this: &Self, buffer: &mut BufferWrite) -> Result<bool, Box<dyn Error>> {
        if this
            .meta
            .lock()
            .map_err(|_| "Replicated meta lock error")?
            .detect_change(&this.data)
        {
            this.data.collect_changes(buffer)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn maybe_collect_changes<const TAG: u8>(
        this: &Self,
        buffer: &mut BufferWrite,
    ) -> Result<bool, Box<dyn Error>> {
        if this
            .meta
            .lock()
            .map_err(|_| "Replicated meta lock error")?
            .detect_change(&this.data)
        {
            let mut temp_buffer = Cursor::new(Vec::new());
            this.data.collect_changes(&mut temp_buffer)?;
            let data_length = temp_buffer.get_ref().len() as u64;
            buffer.write_all(&[TAG])?;
            leb128::write::unsigned(buffer, data_length)?;
            buffer.write_all(&temp_buffer.into_inner())?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        self.data.apply_changes(buffer)?;
        Ok(())
    }

    pub fn maybe_apply_changes<const TAG: u8>(
        &mut self,
        buffer: &mut BufferRead,
    ) -> Result<(), Box<dyn Error>> {
        let position = buffer.position();
        let mut tag_buf = [0u8; 1];
        buffer.read_exact(&mut tag_buf)?;
        if tag_buf[0] != TAG {
            buffer.set_position(position);
            return Ok(());
        }
        let data_length = leb128::read::unsigned(buffer)?;
        self.data.apply_changes(buffer)?;
        let new_position = buffer.position();
        if (new_position - position - 1) != data_length {
            buffer.set_position(position);
            return Err(format!(
                "Data length mismatch: expected {}, got {}",
                data_length,
                new_position - position - 1
            )
            .into());
        }
        Ok(())
    }

    pub fn mark_changed(&mut self) {
        *self
            .meta
            .lock()
            .unwrap_or_else(|_| panic!("Replicated meta lock error")) = P::default();
    }
}

impl<P, T> Serialize for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.data.serialize(serializer)
    }
}

impl<'de, P, T> Deserialize<'de> for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = T::deserialize(deserializer)?;
        let meta = Mutex::new(P::default());
        Ok(Self { data, meta })
    }
}

impl<P, T> std::fmt::Debug for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.data, f)
    }
}

impl<P, T> std::fmt::Display for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + std::fmt::Display,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.data, f)
    }
}

impl<P, T> AsRef<T> for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable,
{
    fn as_ref(&self) -> &T {
        &self.data
    }
}

impl<P, T> AsMut<T> for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable,
{
    fn as_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<P, T> From<T> for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable,
{
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

impl<P, T> Default for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + Default,
{
    fn default() -> Self {
        let data = T::default();
        Self {
            meta: Mutex::new(P::default()),
            data,
        }
    }
}

impl<P, T> Clone for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + Clone,
{
    fn clone(&self) -> Self {
        Self {
            meta: Mutex::new(P::default()),
            data: self.data.clone(),
        }
    }
}

impl<P, T> PartialEq for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl<P, T> Eq for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + Eq,
{
}

impl<P, T> PartialOrd for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.data.partial_cmp(&other.data)
    }
}

impl<P, T> Ord for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.data.cmp(&other.data)
    }
}

impl<P, T> Hash for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable + Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.data.hash(state);
    }
}

impl<P, T> Deref for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<P, T> DerefMut for Replicated<P, T>
where
    P: ReplicationPolicy<T>,
    T: Replicable,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.meta
            .lock()
            .unwrap_or_else(|_| panic!("Replicated meta lock error"))
            .on_mutation(&self.data);
        &mut self.data
    }
}

pub struct CodecRep<T, C: Codec<Value = T>> {
    data: T,
    _phantom: PhantomData<fn() -> C>,
}

impl<T: Replicable, C: Codec<Value = T>> CodecRep<T, C> {
    pub fn new(data: T) -> Self {
        Self {
            data,
            _phantom: PhantomData,
        }
    }

    pub fn into_inner(self) -> T {
        self.data
    }
}

impl<T: Serialize, C: Codec<Value = T>> Serialize for CodecRep<T, C> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.data.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, C: Codec<Value = T>> Deserialize<'de> for CodecRep<T, C> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let data = T::deserialize(deserializer)?;
        Ok(Self {
            data,
            _phantom: PhantomData,
        })
    }
}

impl<T, C: Codec<Value = T>> Replicable for CodecRep<T, C> {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        C::encode(&self.data, buffer)
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        self.data = C::decode(buffer)?;
        Ok(())
    }
}

impl<T: std::fmt::Debug, C: Codec<Value = T>> std::fmt::Debug for CodecRep<T, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.data, f)
    }
}

impl<T: std::fmt::Display, C: Codec<Value = T>> std::fmt::Display for CodecRep<T, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.data, f)
    }
}

impl<T, C: Codec<Value = T>> AsRef<T> for CodecRep<T, C> {
    fn as_ref(&self) -> &T {
        &self.data
    }
}

impl<T, C: Codec<Value = T>> AsMut<T> for CodecRep<T, C> {
    fn as_mut(&mut self) -> &mut T {
        &mut self.data
    }
}

impl<T, C: Codec<Value = T>> From<T> for CodecRep<T, C> {
    fn from(data: T) -> Self {
        Self {
            data,
            _phantom: PhantomData,
        }
    }
}

impl<T: Default, C: Codec<Value = T>> Default for CodecRep<T, C> {
    fn default() -> Self {
        Self {
            data: T::default(),
            _phantom: PhantomData,
        }
    }
}

impl<T: Clone, C: Codec<Value = T>> Clone for CodecRep<T, C> {
    fn clone(&self) -> Self {
        Self {
            data: self.data.clone(),
            _phantom: PhantomData,
        }
    }
}

impl<T: PartialEq, C: Codec<Value = T>> PartialEq for CodecRep<T, C> {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl<T: Eq, C: Codec<Value = T>> Eq for CodecRep<T, C> {}

impl<T: PartialOrd, C: Codec<Value = T>> PartialOrd for CodecRep<T, C> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.data.partial_cmp(&other.data)
    }
}

impl<T: Ord, C: Codec<Value = T>> Ord for CodecRep<T, C> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.data.cmp(&other.data)
    }
}

impl<T: Hash, C: Codec<Value = T>> Hash for CodecRep<T, C> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.data.hash(state);
    }
}

impl<T, C: Codec<Value = T>> Deref for CodecRep<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T, C: Codec<Value = T>> DerefMut for CodecRep<T, C> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codec::postcard::PostcardCodec,
        replication::primitives::{RepF32, RepI32},
    };
    use serde::{Deserialize, Serialize};
    use std::{error::Error, io::Cursor};

    pub type HashPostcardReplicated<T> = HashCodecReplicated<T, PostcardCodec<T>>;
    pub type MutPostcardReplicated<T> = MutCodecReplicated<T, PostcardCodec<T>>;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    struct Foo {
        a: u32,
        b: bool,
    }

    impl Replicable for Foo {
        fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
            self.a.collect_changes(buffer)?;
            self.b.collect_changes(buffer)?;
            Ok(())
        }

        fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
            self.a.apply_changes(buffer)?;
            self.b.apply_changes(buffer)?;
            Ok(())
        }
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Bar {
        a: f32,
        foo: HashPostcardReplicated<Foo>,
    }

    #[derive(Debug, Clone, PartialEq, Hash)]
    struct Zee {
        a: HashReplicated<RepI32>,
        b: HashReplicated<RepF32>,
    }

    impl Replicable for Zee {
        fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
            Replicated::maybe_collect_changes::<0>(&self.a, buffer)?;
            Replicated::maybe_collect_changes::<1>(&self.b, buffer)?;
            Ok(())
        }

        fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
            Replicated::maybe_apply_changes::<0>(&mut self.a, buffer)?;
            Replicated::maybe_apply_changes::<1>(&mut self.b, buffer)?;
            Ok(())
        }
    }

    #[test]
    fn test_hashed_replication() {
        let mut data = HashReplicated::new(Foo { a: 42, b: false });
        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 5);

        let mut buffer = Cursor::default();
        assert!(!Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 0);

        data.a = 100;
        data.b = true;

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 5);

        let mut data2 = HashReplicated::new(Foo { a: 42, b: false });
        let buffer = buffer.into_inner();
        let mut cursor = Cursor::new(buffer.as_slice());
        Replicated::apply_changes(&mut data2, &mut cursor).unwrap();
        assert_eq!(data2.a, 100);
        assert!(data2.b);
    }

    #[test]
    fn test_mutated_replication() {
        let mut data = MutReplicated::new(Foo { a: 42, b: false });
        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 5);

        let mut buffer = Cursor::default();
        assert!(!Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 0);

        data.a = 100;

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 5);

        let mut data2 = MutReplicated::new(Foo { a: 42, b: false });
        let buffer = buffer.into_inner();
        let mut cursor = Cursor::new(buffer.as_slice());
        Replicated::apply_changes(&mut data2, &mut cursor).unwrap();
        assert_eq!(data2.a, 100);
        assert!(!data2.b);
    }

    #[test]
    fn test_manual_replication() {
        let mut data = ManReplicated::new(Foo { a: 42, b: false });
        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 5);

        let mut buffer = Cursor::default();
        assert!(!Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 0);
        data.b = true;

        let mut buffer = Cursor::default();
        assert!(!Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 0);

        data.mark_changed();

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 5);

        let mut data2 = ManReplicated::new(Foo { a: 42, b: false });
        let buffer = buffer.into_inner();
        let mut cursor = Cursor::new(buffer.as_slice());
        Replicated::apply_changes(&mut data2, &mut cursor).unwrap();
        assert_eq!(data2.a, 42);
        assert!(data2.b);
    }

    #[test]
    fn test_codec_replication() {
        let mut data = HashPostcardReplicated::<Foo>::new(Foo { a: 42, b: false }.into());
        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 10);

        let mut buffer = Cursor::default();
        assert!(!Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 0);

        data.a = 100;
        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 10);

        let mut data2 = HashPostcardReplicated::<Foo>::new(Foo { a: 42, b: false }.into());
        let buffer = buffer.into_inner();
        let mut cursor = Cursor::new(buffer.as_slice());
        Replicated::apply_changes(&mut data2, &mut cursor).unwrap();
        assert_eq!(data2.a, 100);
        assert!(!data2.b);
    }

    #[test]
    fn test_nested_replication() {
        let mut data = MutPostcardReplicated::<Bar>::new(
            Bar {
                a: 4.2,
                foo: HashPostcardReplicated::<Foo>::new(Foo { a: 42, b: false }.into()),
            }
            .into(),
        );

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 14);

        let mut buffer = Cursor::default();
        assert!(!Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 0);

        data.a = 2.71;
        data.foo.a = 100;

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 14);

        let mut data2 = MutPostcardReplicated::<Bar>::new(
            Bar {
                a: 4.2,
                foo: HashPostcardReplicated::<Foo>::new(Foo { a: 42, b: false }.into()),
            }
            .into(),
        );
        let buffer = buffer.into_inner();
        let mut cursor = Cursor::new(buffer.as_slice());
        Replicated::apply_changes(&mut data2, &mut cursor).unwrap();
        assert_eq!(data2.a, 2.71);
        assert_eq!(data2.foo.a, 100);
        assert!(!data2.foo.b);
    }

    #[test]
    fn test_partial_replication() {
        let mut data = HashReplicated::new(Zee {
            a: HashReplicated::new(RepI32(10)),
            b: HashReplicated::new(RepF32(4.2)),
        });

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 9);

        let mut buffer = Cursor::default();
        assert!(!Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 0);

        **data.a = 20;

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 3);

        **data.b = 0.0;

        let mut buffer = Cursor::default();
        assert!(Replicated::collect_changes(&data, &mut buffer).unwrap());
        assert_eq!(buffer.get_ref().len(), 6);
    }
}
