use crate::{buffer::Buffer, codec::Codec};
use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque},
    error::Error,
    hash::Hash,
    io::{Read, Write},
    mem::MaybeUninit,
    rc::Rc,
    sync::Arc,
};

impl<T, const N: usize> Codec for [T; N]
where
    T: Codec,
{
    type Value = [T::Value; N];

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        for item in message.iter() {
            T::encode(item, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut arr: [MaybeUninit<T::Value>; N] = unsafe { MaybeUninit::uninit().assume_init() };
        for elem in &mut arr {
            *elem = MaybeUninit::new(T::decode(buffer)?);
        }
        Ok(unsafe { std::mem::transmute_copy::<_, [T::Value; N]>(&arr) })
    }
}

impl<T> Codec for Option<T>
where
    T: Codec,
{
    type Value = Option<T::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        match message {
            Some(value) => {
                buffer.write_all(&[1u8])?;
                T::encode(value, buffer)?;
            }
            None => {
                buffer.write_all(&[0u8])?;
            }
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut flag_buf = [0u8; 1];
        buffer.read_exact(&mut flag_buf)?;
        if flag_buf[0] == 1 {
            Ok(Some(T::decode(buffer)?))
        } else {
            Ok(None)
        }
    }
}

impl<OK, ERR> Codec for Result<OK, ERR>
where
    OK: Codec,
    ERR: Codec,
{
    type Value = Result<OK::Value, ERR::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        match message {
            Ok(value) => {
                buffer.write_all(&[1u8])?;
                OK::encode(value, buffer)?;
            }
            Err(err) => {
                buffer.write_all(&[0u8])?;
                ERR::encode(err, buffer)?;
            }
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut flag_buf = [0u8; 1];
        buffer.read_exact(&mut flag_buf)?;
        if flag_buf[0] == 1 {
            Ok(Ok(OK::decode(buffer)?))
        } else {
            Ok(Err(ERR::decode(buffer)?))
        }
    }
}

impl<C> Codec for Box<C>
where
    C: Codec,
{
    type Value = Box<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        C::encode(message.as_ref(), buffer)
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        Ok(Box::new(C::decode(buffer)?))
    }
}

impl<C> Codec for Rc<C>
where
    C: Codec,
{
    type Value = Rc<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        C::encode(message.as_ref(), buffer)
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        Ok(Rc::new(C::decode(buffer)?))
    }
}

impl<C> Codec for Arc<C>
where
    C: Codec,
{
    type Value = Arc<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        C::encode(message.as_ref(), buffer)
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        Ok(Arc::new(C::decode(buffer)?))
    }
}

impl<C> Codec for Vec<C>
where
    C: Codec,
{
    type Value = Vec<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for item in message {
            C::encode(item, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut items = Vec::with_capacity(len);
        for _ in 0..len {
            items.push(C::decode(buffer)?);
        }
        Ok(items)
    }
}

impl<C> Codec for LinkedList<C>
where
    C: Codec,
{
    type Value = LinkedList<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for item in message {
            C::encode(item, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut list = LinkedList::new();
        for _ in 0..len {
            list.push_back(C::decode(buffer)?);
        }
        Ok(list)
    }
}

impl<C> Codec for VecDeque<C>
where
    C: Codec,
{
    type Value = VecDeque<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for item in message {
            C::encode(item, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut deque = VecDeque::with_capacity(len);
        for _ in 0..len {
            deque.push_back(C::decode(buffer)?);
        }
        Ok(deque)
    }
}

impl<C> Codec for BinaryHeap<C>
where
    C: Codec,
    C::Value: Ord,
{
    type Value = BinaryHeap<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for item in message {
            C::encode(item, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut heap = BinaryHeap::with_capacity(len);
        for _ in 0..len {
            heap.push(C::decode(buffer)?);
        }
        Ok(heap)
    }
}

impl<KC, KV> Codec for HashMap<KC, KV>
where
    KC: Codec,
    KC::Value: Eq + Hash,
    KV: Codec,
{
    type Value = HashMap<KC::Value, KV::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for (key, value) in message {
            KC::encode(key, buffer)?;
            KV::encode(value, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut map = HashMap::with_capacity(len);
        for _ in 0..len {
            let key = KC::decode(buffer)?;
            let value = KV::decode(buffer)?;
            map.insert(key, value);
        }
        Ok(map)
    }
}

impl<C> Codec for HashSet<C>
where
    C: Codec,
    C::Value: Eq + Hash,
{
    type Value = HashSet<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for item in message {
            C::encode(item, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut set = HashSet::with_capacity(len);
        for _ in 0..len {
            set.insert(C::decode(buffer)?);
        }
        Ok(set)
    }
}

impl<KC, KV> Codec for BTreeMap<KC, KV>
where
    KC: Codec,
    KC::Value: Ord,
    KV: Codec,
{
    type Value = BTreeMap<KC::Value, KV::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for (key, value) in message {
            KC::encode(key, buffer)?;
            KV::encode(value, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut map = BTreeMap::new();
        for _ in 0..len {
            let key = KC::decode(buffer)?;
            let value = KV::decode(buffer)?;
            map.insert(key, value);
        }
        Ok(map)
    }
}

impl<C> Codec for BTreeSet<C>
where
    C: Codec,
    C::Value: Ord,
{
    type Value = BTreeSet<C::Value>;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(message.len() as u64).to_le_bytes())?;
        for item in message {
            C::encode(item, buffer)?;
        }
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let mut len_buf = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        let mut set = BTreeSet::new();
        for _ in 0..len {
            set.insert(C::decode(buffer)?);
        }
        Ok(set)
    }
}
