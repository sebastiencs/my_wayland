use crate::{
    drm::Drm,
    egl::EGL,
    globals::{Global, Globals},
    input::Input,
    object::Object,
    protocol_generated::WaylandId,
    xkb::Xkb,
};

use std::{any::Any, marker::PhantomData, ptr::NonNull, rc::Rc};

#[derive(Debug)]
enum ValueOrEmpty<V> {
    Value(V),
    Grabbed,
    Empty { next_empty: Option<u32> },
}

pub struct IndexMap<K, V> {
    map: Vec<ValueOrEmpty<V>>,
    next_empty: Option<u32>,
    /// See `Grab` for why we use an indirection (`Rc`) here
    /// The number of grab is the strong counter of that `Rc`
    ngrabbed: Rc<()>,
    /// `*mut _` to make us !Sync and !Send
    _phantom: PhantomData<*mut K>,
}

impl<K, V> Drop for IndexMap<K, V> {
    fn drop(&mut self) {
        if Rc::strong_count(&self.ngrabbed) == 1 {
            return;
        }
        // Abort when there are still grabbed items
        // Or the grabbed items will have invalid pointer to our `IndexMap`
        eprintln!("ABORT: Dropped `IndexMap` with remaining `Grab` alive");
        eprintln!("ABORT: {}", std::any::type_name::<Self>());
        // Abort because we cannot recover
        // Do not unwind the stack in any way here (such as panics) because it would
        // run the destructor of still-alive `Grab` (that contains pointer
        // to now invalid/deallocated `IndexMap`)
        std::process::abort();
    }
}

impl<K, V> std::ops::Index<K> for IndexMap<K, V>
where
    K: Into<usize> + Copy,
    K: From<usize>,
{
    type Output = V;

    fn index(&self, index: K) -> &Self::Output {
        self.get(index)
    }
}

impl<K, V> std::ops::IndexMut<K> for IndexMap<K, V>
where
    K: Into<usize> + Copy,
    K: From<usize>,
{
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.get_mut(index)
    }
}

#[derive(Debug)]
pub struct Grab<K, V>
where
    K: Into<usize> + Copy,
{
    key: K,
    /// Store the value in an `Option` so we can `take` it during drop and
    /// in `GrabbedObject`
    value: Option<V>,
    /// Pointer to our `IndexMap`, to re-insert the value inside it.
    /// We know that the pointer will always be valid because:
    /// - `IndexMap::grab` is taking a `Box<Self>` as self, so `self` is a stable pointer:
    ///   The `IndexMap` will not change memory location after creating a `Grab`.
    /// - `IndexMap::drop` ensure that there is no remaining `Grab` alive
    /// - `IndexMap` and `Grab` are !Synd and !Send
    map: NonNull<IndexMap<K, V>>,
    /// We don't manipulate `self.map->ngrabbed` because the original `IndexMap` could have
    /// been moved/replaced (such as with `std::mem::replace`).
    /// In other words, `Self::map` points to a valid `IndexMap`, but it might be another
    /// `IndexMap` than the one used to create us (`Grab`), and it's important we
    /// decrement the original `IndexMap::ngrabbed` on `Self::drop`.
    ngrabbed: Rc<()>,
}

static_assertions::assert_not_impl_all!(IndexMap<usize, usize>: Send, Sync);
static_assertions::assert_not_impl_all!(Grab<usize, usize>: Send, Sync);

impl<K, V> std::ops::Deref for Grab<K, V>
where
    K: Into<usize> + Copy,
{
    type Target = V;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref().unwrap()
    }
}

impl<K, V> std::ops::DerefMut for Grab<K, V>
where
    K: Into<usize> + Copy,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut().unwrap()
    }
}

impl<K, V> Drop for Grab<K, V>
where
    K: Into<usize> + Copy,
{
    fn drop(&mut self) {
        let Self { key, value, map, ngrabbed } = self;
        assert!(Rc::strong_count(ngrabbed) > 1);

        let value = value.take().unwrap();

        // Safety: See doc in `Grab`
        let indexmap: &mut IndexMap<K, V> = unsafe { map.as_mut() };
        indexmap.release(*key, value);

        // ngrabbed will be dropped and its counter decremented
    }
}

/// Small helper structure to downcast `dyn Object` into their concrete types
pub struct GrabbedObject<O: Object> {
    value: Option<Box<O>>,
    grabbed: Grab<ObjectId, Box<dyn Object>>,
    _phantom: PhantomData<O>,
}
impl<O: Object> Drop for GrabbedObject<O> {
    fn drop(&mut self) {
        self.grabbed.value = Some(self.value.take().unwrap());
    }
}
impl<O: Object> GrabbedObject<O> {
    pub fn new(mut grabbed: Grab<ObjectId, Box<dyn Object>>) -> Self {
        let value: Box<dyn Object> = grabbed.value.take().unwrap();
        let value: Box<dyn Any> = value.into_any();
        let value: Box<O> = value.downcast().unwrap();
        Self { value: Some(value), grabbed, _phantom: PhantomData }
    }
}
impl<O: Object> std::ops::Deref for GrabbedObject<O> {
    type Target = O;
    fn deref(&self) -> &Self::Target {
        self.value.as_ref().unwrap()
    }
}
impl<O: Object> std::ops::DerefMut for GrabbedObject<O> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut().unwrap()
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
            ngrabbed: Rc::default(),
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

    pub fn iter(&self) -> impl Iterator<Item = (K, &V)> {
        self.map.iter().enumerate().filter_map(|(index, v)| {
            let key: K = index.into();
            let ValueOrEmpty::Value(value) = v else {
                return None;
            };
            Some((key, value))
        })
    }
}

impl<K, V> IndexMap<K, V>
where
    K: Into<usize> + Copy,
{
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

    /// NOTE: Allow grabbing only on `Box<Self>`, so that self is a stable pointer
    pub fn grab<Key: Into<K>>(self: &mut Box<Self>, key: Key) -> Grab<K, V> {
        let key: K = key.into();
        let index: usize = key.into();
        let entry = self.map.get_mut(index).unwrap();

        let value = match std::mem::replace(entry, ValueOrEmpty::Grabbed) {
            ValueOrEmpty::Value(value) => value,
            _ => panic!("invalid state"),
        };

        Grab {
            key,
            value: Some(value),
            map: NonNull::from(&mut **self),
            ngrabbed: self.ngrabbed.clone(),
        }
    }

    fn release(&mut self, key: K, value: V) {
        let index: usize = key.into();
        let entry = self.map.get_mut(index).unwrap();

        let value = ValueOrEmpty::Value(value);
        let ValueOrEmpty::Grabbed = std::mem::replace(entry, value) else {
            panic!("invalid state");
        };
    }

    pub fn with<F, R>(self: &mut Box<Self>, key: K, fun: F) -> R
    where
        F: FnOnce(&mut Grab<K, V>) -> R,
    {
        let mut grabbed = self.grab(key);
        fun(&mut grabbed)
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

    pub fn contains(&self, key: K) -> bool {
        self.try_get(key).is_some()
    }
}

index_map_id!(ObjectId);
index_map_id!(InputId);
index_map_id!(DrmId);

pub struct State {
    pub objects: Box<IndexMap<ObjectId, Box<dyn Object>>>,
    pub inputs: IndexMap<InputId, Input>,
    pub drm: IndexMap<DrmId, Drm>,
    pub globals: Globals,
    pub egl: EGL,
    pub xkb: Xkb,
}

impl State {
    pub fn insert_object<O: Object + 'static>(&mut self, object: O) -> (WaylandId, ObjectId) {
        let wayland_id = object.id();
        let object_id = self.objects.insert(Box::new(object));
        (wayland_id, object_id)
    }
    pub fn remove_object(&mut self, object_id: ObjectId) -> Box<dyn Object> {
        self.objects.remove(object_id).unwrap()
    }

    pub fn insert_global(&mut self, global: Box<dyn Global>) {
        self.globals.insert(global);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[ignore]
    #[test]
    #[should_panic]
    fn indexmap_dropped() {
        let mut map = Box::new(IndexMap::<usize, usize>::default());
        let key = map.insert(10);
        let grabbed = map.grab(key);
        std::mem::drop(map);
        dbg!(key, grabbed);
    }
}
