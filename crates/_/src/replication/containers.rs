use crate::{buffer::Buffer, replication::Replicable};
use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque},
    error::Error,
    hash::Hash,
    io::{Read, Write},
    rc::Rc,
    sync::Arc,
};

impl<T, const N: usize> Replicable for [T; N]
where
    T: Replicable,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        for item in self.iter() {
            item.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        for item in self.iter_mut() {
            item.apply_changes(buffer)?;
        }
        Ok(())
    }
}

impl<T> Replicable for Option<T>
where
    T: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        match self {
            Some(value) => {
                buffer.write_all(&[1])?;
                value.collect_changes(buffer)
            }
            None => buffer.write_all(&[0]).map_err(Into::into),
        }
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut flag = [0u8];
        buffer.read_exact(&mut flag)?;
        match flag[0] {
            1 => {
                let mut value = T::default();
                value.apply_changes(buffer)?;
                *self = Some(value);
            }
            0 => *self = None,
            _ => return Err("Invalid flag for Option".into()),
        }
        Ok(())
    }
}

impl<OK, ERR> Replicable for Result<OK, ERR>
where
    OK: Replicable + Default,
    ERR: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        match self {
            Ok(value) => {
                buffer.write_all(&[1])?;
                value.collect_changes(buffer)
            }
            Err(err) => {
                buffer.write_all(&[0])?;
                err.collect_changes(buffer)
            }
        }
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut flag = [0u8];
        buffer.read_exact(&mut flag)?;
        match flag[0] {
            1 => {
                let mut value = OK::default();
                value.apply_changes(buffer)?;
                *self = Ok(value);
            }
            0 => {
                let mut err = ERR::default();
                err.apply_changes(buffer)?;
                *self = Err(err);
            }
            _ => return Err("Invalid flag for Result".into()),
        }
        Ok(())
    }
}

impl<T> Replicable for Box<T>
where
    T: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.as_ref().collect_changes(buffer)
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.as_mut().apply_changes(buffer)
    }
}

impl<T> Replicable for Rc<T>
where
    T: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.as_ref().collect_changes(buffer)
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        if let Some(inner) = Rc::get_mut(self) {
            inner.apply_changes(buffer)?;
        } else {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            *self = Rc::new(value);
        }
        Ok(())
    }
}

impl<T> Replicable for Arc<T>
where
    T: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.as_ref().collect_changes(buffer)
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        if let Some(inner) = Arc::get_mut(self) {
            inner.apply_changes(buffer)?;
        } else {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            *self = Arc::new(value);
        }
        Ok(())
    }
}

impl<T> Replicable for Vec<T>
where
    T: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for item in self {
            item.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            self.push(value);
        }
        Ok(())
    }
}

impl<T> Replicable for LinkedList<T>
where
    T: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for item in self {
            item.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            self.push_back(value);
        }
        Ok(())
    }
}

impl<T> Replicable for VecDeque<T>
where
    T: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for item in self {
            item.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            self.push_back(value);
        }
        Ok(())
    }
}

impl<T> Replicable for BinaryHeap<T>
where
    T: Replicable + Default + Ord,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for item in self {
            item.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            self.push(value);
        }
        Ok(())
    }
}

impl<K, V> Replicable for HashMap<K, V>
where
    K: Replicable + Default + Eq + Hash,
    V: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for (k, v) in self {
            k.collect_changes(buffer)?;
            v.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut k = K::default();
            k.apply_changes(buffer)?;
            let mut v = V::default();
            v.apply_changes(buffer)?;
            self.insert(k, v);
        }
        Ok(())
    }
}

impl<T> Replicable for HashSet<T>
where
    T: Replicable + Default + Eq + Hash,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for item in self {
            item.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            self.insert(value);
        }
        Ok(())
    }
}

impl<K, V> Replicable for BTreeMap<K, V>
where
    K: Replicable + Default + Ord,
    V: Replicable + Default,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for (k, v) in self {
            k.collect_changes(buffer)?;
            v.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut k = K::default();
            k.apply_changes(buffer)?;
            let mut v = V::default();
            v.apply_changes(buffer)?;
            self.insert(k, v);
        }
        Ok(())
    }
}

impl<T> Replicable for BTreeSet<T>
where
    T: Replicable + Default + Ord,
{
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&(self.len() as u64).to_le_bytes())?;
        for item in self {
            item.collect_changes(buffer)?;
        }
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        let mut len_buf = [0u8; 8];
        buffer.read_exact(&mut len_buf)?;
        let len = u64::from_le_bytes(len_buf) as usize;
        self.clear();
        for _ in 0..len {
            let mut value = T::default();
            value.apply_changes(buffer)?;
            self.insert(value);
        }
        Ok(())
    }
}
