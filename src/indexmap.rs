use std::{marker::PhantomData, rc::Rc};

use crate::{
    object::Object,
    state::{ObjectId, State},
};

#[derive(Debug)]
enum ValueOrEmpty<V> {
    Value(V),
    Grabbed,
    /// `remove_or_defer` while grabbed: on [`Grab`] drop, drop value and free slot.
    GrabbedToRemove,
    Empty {
        next_empty: Option<u32>,
    },
}

pub trait IndexMapKey: Copy {
    fn from_parts(index: u32, generation: u32) -> Self;
    fn index(self) -> u32;
    fn generation(self) -> u32;
}

#[derive(Debug)]
struct Slot<V> {
    generation: u32,
    value: ValueOrEmpty<V>,
}

mod grab_anchor {
    use std::{marker::PhantomData, ptr::NonNull, rc::Rc};

    use super::{IndexMap, IndexMapKey};

    #[derive(Debug)]
    pub(super) struct MapAnchor<K, V> {
        map: NonNull<IndexMap<K, V>>,
        ngrabbed: Rc<()>,
        _not_send_sync: PhantomData<*mut ()>,
    }

    impl<K, V> MapAnchor<K, V>
    where
        K: IndexMapKey,
    {
        pub(super) fn new(map: &mut Box<IndexMap<K, V>>, ngrabbed: Rc<()>) -> Self {
            let map = NonNull::from(&mut **map);
            Self { map, ngrabbed, _not_send_sync: PhantomData }
        }

        pub(super) fn grabbed_token(&self) -> &Rc<()> {
            &self.ngrabbed
        }

        pub(super) fn restore(&mut self, key: K, value: V) {
            // SAFETY:
            // - `map` is captured from `&mut Box<IndexMap<..>>`, so its pointee address is stable.
            // - `IndexMap::drop` aborts if a `Grab` is still alive, so `map` cannot dangle here.
            // - `Grab` and `MapAnchor` are !Send/!Sync, so restore always happens on the same thread.
            let map: &mut IndexMap<K, V> = unsafe { self.map.as_mut() };
            map.release(key, value);
        }
    }
}

pub struct IndexMap<K, V> {
    map: Vec<Slot<V>>,
    next_empty: Option<u32>,
    /// See `Grab` for why we use an indirection (`Rc`) here.
    /// The number of grabs is the strong counter of that `Rc`.
    ngrabbed: Rc<()>,
    /// `*mut _` to make us !Sync and !Send.
    _phantom: PhantomData<*mut K>,
}

impl<K, V> Drop for IndexMap<K, V> {
    fn drop(&mut self) {
        if Rc::strong_count(&self.ngrabbed) == 1 {
            return;
        }
        // Ownership invariant: a grab must not outlive the map. This is not a
        // device or protocol failure, so it stays an abort.
        eprintln!("ABORT: Dropped `IndexMap` with remaining `Grab` alive");
        eprintln!("ABORT: {}", std::any::type_name::<Self>());
        std::process::abort();
    }
}

impl<K, V> std::ops::Index<K> for IndexMap<K, V>
where
    K: IndexMapKey,
{
    type Output = V;

    fn index(&self, index: K) -> &Self::Output {
        self.get(index)
    }
}

impl<K, V> std::ops::IndexMut<K> for IndexMap<K, V>
where
    K: IndexMapKey,
{
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.get_mut(index)
    }
}

#[derive(Debug)]
pub struct Grab<K, V>
where
    K: IndexMapKey,
{
    key: K,
    /// Store the value in an `Option` so we can `take` it during drop.
    pub(crate) value: Option<V>,
    anchor: grab_anchor::MapAnchor<K, V>,
}

static_assertions::assert_not_impl_all!(IndexMap<usize, usize>: Send, Sync);

impl<K, V> std::ops::Deref for Grab<K, V>
where
    K: IndexMapKey,
{
    type Target = V;

    fn deref(&self) -> &Self::Target {
        self.value.as_ref().unwrap()
    }
}

impl<K, V> std::ops::DerefMut for Grab<K, V>
where
    K: IndexMapKey,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.value.as_mut().unwrap()
    }
}

impl<K, V> Drop for Grab<K, V>
where
    K: IndexMapKey,
{
    fn drop(&mut self) {
        let Self { key, value, anchor } = self;
        assert!(Rc::strong_count(anchor.grabbed_token()) > 1);
        let value = value.take().unwrap();
        anchor.restore(*key, value);
    }
}

impl<K, V> std::fmt::Debug for IndexMap<K, V>
where
    K: IndexMapKey + std::fmt::Debug,
    V: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let map = self.map.iter().enumerate().map(|(index, slot)| {
            let key = K::from_parts(index.try_into().unwrap(), slot.generation);
            (key, &slot.value)
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
    K: IndexMapKey,
{
    fn key_for_slot(index: usize, generation: u32) -> K {
        K::from_parts(index.try_into().unwrap(), generation)
    }

    fn slot_ref(&self, key: K) -> Option<&Slot<V>> {
        let slot = self.map.get(key.index() as usize)?;
        (slot.generation == key.generation()).then_some(slot)
    }

    fn slot_mut(&mut self, key: K) -> Option<&mut Slot<V>> {
        let slot = self.map.get_mut(key.index() as usize)?;
        (slot.generation == key.generation()).then_some(slot)
    }

    pub fn insert(&mut self, value: V) -> K {
        let empty_index = match self.next_empty {
            Some(empty) => empty,
            None => {
                let length = self.map.len();
                self.map
                    .push(Slot { generation: 0, value: ValueOrEmpty::Value(value) });
                return Self::key_for_slot(length, 0);
            }
        };

        let slot = self.map.get_mut(empty_index as usize).unwrap();
        let next_empty = match &slot.value {
            ValueOrEmpty::Empty { next_empty } => *next_empty,
            _ => panic!("invalid state"),
        };
        slot.generation = slot.generation.wrapping_add(1);
        let generation = slot.generation;
        slot.value = ValueOrEmpty::Value(value);

        self.next_empty = next_empty;

        Self::key_for_slot(empty_index as usize, generation)
    }

    pub fn iter(&self) -> impl Iterator<Item = (K, &V)> {
        self.map.iter().enumerate().filter_map(|(index, slot)| {
            let key = Self::key_for_slot(index, slot.generation);
            let value = match &slot.value {
                ValueOrEmpty::Value(value) => value,
                ValueOrEmpty::Grabbed => panic!("iterating while grabbed"),
                ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => return None,
            };
            Some((key, value))
        })
    }
}

impl<K, V> IndexMap<K, V>
where
    K: IndexMapKey,
{
    pub fn grabbed_count(&self) -> usize {
        Rc::strong_count(&self.ngrabbed).saturating_sub(1)
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let index = key.index() as usize;
        let next_empty = self.next_empty;
        let slot = self.slot_mut(key)?;

        let empty = ValueOrEmpty::Empty { next_empty };
        let value = match std::mem::replace(&mut slot.value, empty) {
            ValueOrEmpty::Value(value) => value,
            ValueOrEmpty::Empty { .. } => return None,
            ValueOrEmpty::Grabbed | ValueOrEmpty::GrabbedToRemove => panic!("invalid state"),
        };
        self.next_empty = Some(index as u32);

        Some(value)
    }

    pub fn remove_or_defer(&mut self, key: K) -> Option<V> {
        let index = key.index() as usize;
        let next_empty = self.next_empty;
        let slot = self.slot_mut(key)?;

        match &mut slot.value {
            ValueOrEmpty::Empty { .. } => None,
            ValueOrEmpty::Grabbed => {
                slot.value = ValueOrEmpty::GrabbedToRemove;
                None
            }
            ValueOrEmpty::GrabbedToRemove => None,
            ValueOrEmpty::Value(_) => {
                let empty = ValueOrEmpty::Empty { next_empty };
                let ValueOrEmpty::Value(value) = std::mem::replace(&mut slot.value, empty) else {
                    unreachable!()
                };
                self.next_empty = Some(index as u32);
                Some(value)
            }
        }
    }

    /// Allow grabbing only on `Box<Self>`, so the pointee address stays stable.
    pub fn grab(self: &mut Box<Self>, key: K) -> Grab<K, V> {
        let value = {
            let slot = self.slot_mut(key).expect("invalid key");
            match std::mem::replace(&mut slot.value, ValueOrEmpty::Grabbed) {
                ValueOrEmpty::Value(value) => value,
                _ => panic!("invalid state"),
            }
        };
        let anchor = grab_anchor::MapAnchor::new(self, self.ngrabbed.clone());

        Grab { key, value: Some(value), anchor }
    }

    pub fn try_grab(self: &mut Box<Self>, key: K) -> Option<Grab<K, V>> {
        let value = {
            let slot = self.slot_mut(key)?;
            match &slot.value {
                ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => return None,
                ValueOrEmpty::Grabbed => panic!("invalid state"),
                ValueOrEmpty::Value(_) => {}
            }
            match std::mem::replace(&mut slot.value, ValueOrEmpty::Grabbed) {
                ValueOrEmpty::Value(value) => value,
                _ => unreachable!(),
            }
        };
        let anchor = grab_anchor::MapAnchor::new(self, self.ngrabbed.clone());
        Some(Grab { key, value: Some(value), anchor })
    }

    fn release(&mut self, key: K, value: V) {
        let index = key.index() as usize;
        let next_empty = self.next_empty;
        let slot = self
            .slot_mut(key)
            .expect("invalid state in IndexMap::release");

        match &mut slot.value {
            ValueOrEmpty::Grabbed => {
                slot.value = ValueOrEmpty::Value(value);
            }
            ValueOrEmpty::GrabbedToRemove => {
                drop(value);
                slot.value = ValueOrEmpty::Empty { next_empty };
                self.next_empty = Some(index as u32);
            }
            _ => panic!("invalid state in IndexMap::release"),
        }
    }

    pub fn with<F, R>(self: &mut Box<Self>, key: K, fun: F) -> R
    where
        F: FnOnce(&mut Grab<K, V>) -> R,
    {
        let mut grabbed = self.grab(key);
        fun(&mut grabbed)
    }

    pub fn get(&self, key: K) -> &V {
        self.try_get(key).expect("invalid key")
    }

    pub fn try_get(&self, key: K) -> Option<&V> {
        match &self.slot_ref(key)?.value {
            ValueOrEmpty::Value(value) => Some(value),
            ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => None,
            ValueOrEmpty::Grabbed => panic!("invalid state"),
        }
    }

    pub fn get_mut(&mut self, key: K) -> &mut V {
        self.try_get_mut(key).expect("invalid key")
    }

    pub fn try_get_mut(&mut self, key: K) -> Option<&mut V> {
        match &mut self.slot_mut(key)?.value {
            ValueOrEmpty::Value(value) => Some(value),
            ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => None,
            ValueOrEmpty::Grabbed => panic!("invalid state"),
        }
    }

    pub fn contains(&self, key: K) -> bool {
        match self.slot_ref(key).map(|slot| &slot.value) {
            Some(ValueOrEmpty::Value(_)) | Some(ValueOrEmpty::Grabbed) => true,
            Some(ValueOrEmpty::Empty { .. }) | Some(ValueOrEmpty::GrabbedToRemove) => false,
            None => false,
        }
    }
}

static_assertions::assert_not_impl_all!(IndexMap<usize, usize>: Send, Sync);
static_assertions::assert_not_impl_all!(Grab<ObjectId, usize>: Send, Sync);

pub struct GrabbedObject<O: Object> {
    value: Option<Box<O>>,
    grab: Grab<ObjectId, Box<dyn Object>>,
}
impl<O: Object> Drop for GrabbedObject<O> {
    fn drop(&mut self) {
        self.grab.value = Some(self.value.take().unwrap());
    }
}
impl<O: Object> GrabbedObject<O> {
    pub fn new(value: Box<O>, grab: Grab<ObjectId, Box<dyn Object>>) -> Self {
        Self { value: Some(value), grab }
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

impl Grab<ObjectId, Box<dyn Object>> {
    /// Helper to call `ObjectDynEtc::call_on` because `Object::on` isn't object safe
    pub fn on(
        mut self,
        client: &mut crate::clients::Client,
        state: &mut State,
        request: crate::clients::RawRequest,
        dispatcher: &mut crate::clients::ClientDispatcher,
    ) -> crate::object::RequestResult {
        let object = self.value.take().unwrap();
        object.call_on(self, client, state, request, dispatcher)
    }
}

impl IndexMap<ObjectId, Box<dyn Object>> {
    pub fn try_ref_object<T: Object>(&self, key: ObjectId) -> Option<&T> {
        self.try_get(key)
            .and_then(|object| object.downcast_ref::<T>())
    }

    pub fn try_ref_object_mut<T: Object>(&mut self, key: ObjectId) -> Option<&mut T> {
        self.try_get_mut(key)
            .and_then(|object| object.downcast_mut::<T>())
    }

    pub fn try_grab_object<T: Object>(
        self: &mut Box<Self>,
        key: ObjectId,
    ) -> Option<GrabbedObject<T>> {
        let mut grabbed = self.try_grab(key)?;
        let erased = grabbed.value.take().unwrap();
        match erased.downcast::<T>() {
            Ok(value) => Some(GrabbedObject::<T>::new(value, grabbed)),
            Err(erased) => {
                grabbed.value = Some(erased);
                None
            }
        }
    }

    pub fn grab_object<T: Object>(self: &mut Box<Self>, key: ObjectId) -> GrabbedObject<T> {
        self.try_grab_object(key).expect("invalid object type")
    }
}
