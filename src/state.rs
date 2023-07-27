use crate::protocol_generated::WaylandId;

use {crate::object::Object, std::marker::PhantomData};

#[derive(Debug)]
enum ValueOrEmpty<V> {
    Value(V),
    Grabbed,
    Empty { next_empty: Option<u32> },
}

pub struct IndexMap<K, V> {
    map: Vec<ValueOrEmpty<V>>,
    next_empty: Option<u32>,
    _phantom: PhantomData<K>,
}

pub struct Grab<K, V> {
    key: K,
    value: V,
}

impl<K, V> std::ops::Deref for Grab<K, V> {
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<K, V> std::ops::DerefMut for Grab<K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
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

impl<K, V> IndexMap<K, V>
where
    K: Into<usize> + Copy,
    K: From<usize>,
{
    pub fn insert(&mut self, value: V) -> K {
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

    pub fn remove(&mut self, key: K) -> Option<V> {
        let index: usize = key.into();
        let entry = self.map.get_mut(index)?;

        let empty = ValueOrEmpty::Empty { next_empty: self.next_empty };
        let value = match std::mem::replace(entry, empty) {
            ValueOrEmpty::Value(value) => value,
            ValueOrEmpty::Empty { .. } => panic!("cannot remove twice"), // produce invalid state
            ValueOrEmpty::Grabbed => panic!("invalid state"),
        };
        self.next_empty = Some(index as u32);

        Some(value)
    }

    pub fn grab(&mut self, key: K) -> Grab<K, V> {
        let index: usize = key.into();
        let entry = self.map.get_mut(index).unwrap();

        let value = match std::mem::replace(entry, ValueOrEmpty::Grabbed) {
            ValueOrEmpty::Value(value) => value,
            _ => panic!("invalid state"),
        };

        Grab { key, value }
    }

    pub fn release(&mut self, grabbed: Grab<K, V>) {
        let Grab { key, value } = grabbed;

        let index: usize = key.into();
        let entry = self.map.get_mut(index).unwrap();

        let value = ValueOrEmpty::Value(value);
        let ValueOrEmpty::Grabbed = std::mem::replace(entry, value) else {
            panic!("invalid state");
        };
    }

    pub fn with<F, R>(&mut self, key: K, fun: F) -> R
    where
        F: FnOnce(&mut Grab<K, V>) -> R,
    {
        let mut grabbed = self.grab(key);
        let result = fun(&mut grabbed);
        self.release(grabbed);
        result
    }

    pub fn get(&self, key: K) -> &V
    where
        K: Into<usize>,
    {
        self.try_get(key).expect("invalid key")
    }

    pub fn try_get(&self, key: K) -> Option<&V> {
        let index: usize = key.into();
        match self.map.get(index)? {
            ValueOrEmpty::Value(value) => Some(value),
            ValueOrEmpty::Empty { .. } => None,
            ValueOrEmpty::Grabbed => panic!("invalid state"),
        }
    }

    pub fn get_mut(&mut self, key: K) -> &mut V {
        self.try_get_mut(key).expect("invalid key")
    }

    pub fn try_get_mut(&mut self, key: K) -> Option<&mut V> {
        let index: usize = key.into();
        match self.map.get_mut(index)? {
            ValueOrEmpty::Value(value) => Some(value),
            ValueOrEmpty::Empty { .. } => None,
            ValueOrEmpty::Grabbed => panic!("invalid state"),
        }
    }
}

index_map_id!(ObjectId);

#[derive(Default)]
pub struct State {
    pub objects: IndexMap<ObjectId, Box<dyn Object>>,
}

impl State {
    pub fn insert_object<O: Object + 'static>(&mut self, object: O) -> (WaylandId, ObjectId) {
        let wayland_id = object.id();
        let object_id = self.objects.insert(Box::new(object));
        (wayland_id, object_id)
    }
}
