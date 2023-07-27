use {crate::object::Object, std::marker::PhantomData};

#[derive(Debug)]
enum ValueOrEmpty<V> {
    Value(V),
    Empty { next_empty: Option<u32> },
}

pub struct IndexMap<K, V> {
    map: Vec<ValueOrEmpty<V>>,
    next_empty: Option<u32>,
    _phantom: PhantomData<K>,
}

impl<K, V> std::fmt::Debug for IndexMap<K, V>
where
    K: std::fmt::Debug + From<usize>,
    V: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let map = self.map.iter().enumerate().map(|(index, v)| {
            let key = K::from(index);
            (key, v)
        });

        f.debug_map().entries(map).finish()
    }
}

impl<K, V> Default for IndexMap<K, V> {
    fn default() -> Self {
        Self {
            map: Vec::with_capacity(128),
            next_empty: None,
            _phantom: Default::default(),
        }
    }
}

impl<K, V> IndexMap<K, V> {
    pub fn insert(&mut self, value: V) -> K
    where
        K: From<usize>,
    {
        let empty_index = match self.next_empty {
            Some(empty) => empty,
            None => {
                let length = self.map.len();
                self.map.push(ValueOrEmpty::Value(value));
                return K::from(length);
            }
        };

        let empty = self.map.get_mut(empty_index as usize).unwrap();
        let next_empty = match empty {
            ValueOrEmpty::Empty { next_empty } => *next_empty,
            _ => panic!("invalid state"),
        };

        *empty = ValueOrEmpty::Value(value);
        self.next_empty = next_empty;

        K::from(empty_index as usize)
    }

    pub fn remove(&mut self, key: K) -> Option<V>
    where
        K: Into<usize>,
    {
        let index: usize = key.into();
        let entry = self.map.get_mut(index)?;

        let empty = ValueOrEmpty::Empty { next_empty: self.next_empty };
        let value = match std::mem::replace(entry, empty) {
            ValueOrEmpty::Value(value) => value,
            ValueOrEmpty::Empty { .. } => panic!("cannot remove twice"), // produce invalid state
        };
        self.next_empty = Some(index as u32);

        Some(value)
    }

    pub fn get(&self, key: K) -> &V
    where
        K: Into<usize>,
    {
        self.try_get(key).expect("invalid key")
    }

    pub fn try_get(&self, key: K) -> Option<&V>
    where
        K: Into<usize>,
    {
        let index: usize = key.into();
        match self.map.get(index)? {
            ValueOrEmpty::Value(value) => Some(value),
            ValueOrEmpty::Empty { .. } => None,
        }
    }

    pub fn get_mut(&mut self, key: K) -> &mut V
    where
        K: Into<usize>,
    {
        self.try_get_mut(key).expect("invalid key")
    }

    pub fn try_get_mut(&mut self, key: K) -> Option<&mut V>
    where
        K: Into<usize>,
    {
        let index: usize = key.into();
        match self.map.get_mut(index)? {
            ValueOrEmpty::Value(value) => Some(value),
            ValueOrEmpty::Empty { .. } => None,
        }
    }
}

index_map_id!(ObjectId);

#[derive(Default)]
pub struct State {
    objects: IndexMap<ObjectId, Box<dyn Object>>,
}
