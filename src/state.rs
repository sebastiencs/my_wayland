use crate::{
    drm::Drm,
    egl::EGL,
    globals::{Global, GlobalName, Globals},
    input::Input,
    object::Object,
    protocol_generated::WaylandId,
    seat::SeatState,
    xkb::{KeyboardState, Xkb},
};

use std::{collections::BTreeMap, marker::PhantomData, ptr::NonNull, rc::Rc};

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
        let value: Box<O> = value.downcast().ok().unwrap();
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
            let value = match v {
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
    K: Into<usize> + Copy,
{
    pub fn grabbed_count(&self) -> usize {
        Rc::strong_count(&self.ngrabbed).saturating_sub(1)
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let index: usize = key.into();
        let entry = self.map.get_mut(index)?;

        let empty = ValueOrEmpty::Empty { next_empty: self.next_empty };
        let value = match std::mem::replace(entry, empty) {
            ValueOrEmpty::Value(value) => value,
            ValueOrEmpty::Empty { .. } => panic!("cannot remove twice"), // produce invalid state
            ValueOrEmpty::Grabbed | ValueOrEmpty::GrabbedToRemove => panic!("invalid state"),
        };
        self.next_empty = Some(index as u32);

        Some(value)
    }

    /// Remove immediately if `key` holds a value; if the slot is grabbed, mark for removal and
    /// return `None` (value is dropped when the active `Grab` is released).
    ///
    /// Returns `None` if the slot is empty, out of range, or removal was deferred.
    pub fn remove_or_defer(&mut self, key: K) -> Option<V> {
        let index: usize = key.into();
        let entry = self.map.get_mut(index)?;

        match entry {
            ValueOrEmpty::Empty { .. } => None,
            ValueOrEmpty::Grabbed => {
                *entry = ValueOrEmpty::GrabbedToRemove;
                None
            }
            ValueOrEmpty::GrabbedToRemove => None,
            ValueOrEmpty::Value(_) => {
                let empty = ValueOrEmpty::Empty { next_empty: self.next_empty };
                let ValueOrEmpty::Value(value) = std::mem::replace(entry, empty) else {
                    unreachable!()
                };
                self.next_empty = Some(index as u32);
                Some(value)
            }
        }
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

    /// Like [`Self::grab`], but returns [`None`] if the slot is empty, out of range, or already grabbed.
    pub fn try_grab<Key: Into<K> + Copy>(self: &mut Box<Self>, key: Key) -> Option<Grab<K, V>>
    where
        K: Into<usize> + Copy + From<usize>,
    {
        let key: K = key.into();
        let index: usize = key.into();
        let entry = self.map.get_mut(index)?;
        if !matches!(entry, ValueOrEmpty::Value(_)) {
            return None;
        }
        let value = match std::mem::replace(entry, ValueOrEmpty::Grabbed) {
            ValueOrEmpty::Value(value) => value,
            _ => unreachable!(),
        };
        Some(Grab {
            key,
            value: Some(value),
            map: NonNull::from(&mut **self),
            ngrabbed: self.ngrabbed.clone(),
        })
    }

    fn release(&mut self, key: K, value: V) {
        let index: usize = key.into();
        let entry = self.map.get_mut(index).unwrap();

        match entry {
            ValueOrEmpty::Grabbed => {
                *entry = ValueOrEmpty::Value(value);
            }
            ValueOrEmpty::GrabbedToRemove => {
                drop(value);
                *entry = ValueOrEmpty::Empty { next_empty: self.next_empty };
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
            ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => None,
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
            ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => None,
            ValueOrEmpty::Grabbed => panic!("invalid state"),
        }
    }

    pub fn contains(&self, key: K) -> bool {
        let index: usize = key.into();
        match self.map.get(index) {
            Some(ValueOrEmpty::Value(_)) | Some(ValueOrEmpty::Grabbed) => true,
            Some(ValueOrEmpty::Empty { .. }) | Some(ValueOrEmpty::GrabbedToRemove) => false,
            None => false,
        }
    }
}

impl<K> IndexMap<K, Box<dyn Object>>
where
    K: Into<usize> + Copy,
{
    pub fn try_get_ref_of<T: Object>(&self, key: K) -> Option<&T> {
        self.try_get(key)
            .and_then(|object| object.downcast_ref::<T>())
    }
}

index_map_id!(ObjectId);
index_map_id!(InputId);
index_map_id!(DrmId);
index_map_id!(OutputId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendOutputRef {
    pub drm_id: DrmId,
}

impl BackendOutputRef {
    pub fn new(drm_id: DrmId) -> Self {
        Self { drm_id }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputMode {
    pub width: i32,
    pub height: i32,
    pub refresh_millihz: i32,
}

#[derive(Clone, Debug)]
pub struct Output {
    pub backend: BackendOutputRef,
    pub global_name: Option<GlobalName>,
    pub x: i32,
    pub y: i32,
    pub physical_width: i32,
    pub physical_height: i32,
    pub scale: i32,
    pub transform: i32,
    pub make: String,
    pub model: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub current_mode: OutputMode,
    pub preferred_mode: Option<OutputMode>,
}

impl Output {
    pub fn new(backend: BackendOutputRef, drm: &Drm) -> Self {
        let mut output = Self {
            backend,
            global_name: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: String::new(),
            description: String::new(),
            enabled: false,
            current_mode: OutputMode { width: 0, height: 0, refresh_millihz: 0 },
            preferred_mode: None,
        };
        output.refresh_from_backend(drm);
        output
    }

    pub fn backend(&self) -> BackendOutputRef {
        self.backend
    }

    pub fn refresh_from_backend(&mut self, drm: &Drm) {
        let in_use = &drm.output.in_use;
        let connector = &in_use.connector;

        let preferred_mode = connector
            .preferred_mode
            .as_ref()
            .map(Self::mode_from_connector);
        let current_mode = drm
            .current_output_mode()
            .or_else(|| preferred_mode.clone())
            .unwrap_or(OutputMode { width: 0, height: 0, refresh_millihz: 0 });

        let make = connector.manufacturer_id.clone();
        let model = connector.model_name.clone().unwrap_or_default();
        let name = connector.interface_name.clone();
        let description = match (make.is_empty(), model.is_empty()) {
            (false, false) => format!("{make} {model}"),
            (false, true) => make.clone(),
            (true, false) => model.clone(),
            (true, true) => name.clone(),
        };

        self.physical_width = connector.mm_width as i32;
        self.physical_height = connector.mm_height as i32;
        self.make = make;
        self.model = model;
        self.name = name;
        self.description = description;
        self.enabled = true;
        self.current_mode = current_mode;
        self.preferred_mode = preferred_mode;
    }

    pub fn logical_size(&self) -> (i32, i32) {
        let scale = self.scale.max(1);
        let (width, height) = match self.transform {
            1 | 3 | 5 | 7 => (self.current_mode.height, self.current_mode.width),
            _ => (self.current_mode.width, self.current_mode.height),
        };
        (width / scale, height / scale)
    }

    fn mode_from_connector(mode: &drm::control::Mode) -> OutputMode {
        let (width, height) = mode.size();
        let refresh_millihz = {
            let clock_milli_hertz = mode.clock() as u64 * 1_000_000;
            let (_, _, htotal) = mode.hsync();
            let (_, _, vtotal) = mode.vsync();
            (((clock_milli_hertz / htotal as u64) + (vtotal as u64 / 2)) / vtotal as u64) as i32
        };

        OutputMode { width: width as i32, height: height as i32, refresh_millihz }
    }
}

pub struct State {
    pub objects: Box<IndexMap<ObjectId, Box<dyn Object>>>,
    pub root_surfaces: Vec<ObjectId>,
    pub inputs: IndexMap<InputId, Input>,
    pub drm: IndexMap<DrmId, Drm>,
    pub outputs: IndexMap<OutputId, Output>,
    pub active_output: Option<OutputId>,
    pub backend_syncobj_globals: BTreeMap<DrmId, GlobalName>,
    pub globals: Globals,
    pub egl: EGL,
    pub seat: SeatState,
    pub keyboard: KeyboardState,
    pub xkb: Xkb,
}

impl State {
    pub fn insert_object<O: Object + 'static>(&mut self, object: O) -> (WaylandId, ObjectId) {
        let wayland_id = object.id();
        let object_id = self.objects.insert(Box::new(object));
        (wayland_id, object_id)
    }
    /// Removes the object now, or defers if the slot is currently grabbed.
    /// Returns `None` if deferred or if the slot was already empty.
    pub fn remove_object(&mut self, object_id: ObjectId) -> Option<Box<dyn Object>> {
        self.unregister_root_surface(object_id);
        self.objects.remove_or_defer(object_id)
    }

    pub fn register_root_surface(&mut self, object_id: ObjectId) {
        self.unregister_root_surface(object_id);
        self.root_surfaces.push(object_id);
    }

    pub fn unregister_root_surface(&mut self, object_id: ObjectId) {
        self.root_surfaces
            .retain(|candidate| *candidate != object_id);
    }

    pub fn root_surface_ids(&self) -> Vec<ObjectId> {
        self.root_surfaces
            .iter()
            .copied()
            .filter(|object_id| self.objects.contains(*object_id))
            .collect()
    }

    pub fn insert_global(&mut self, global: Box<dyn Global>) -> GlobalName {
        self.globals.insert(global)
    }

    pub fn remove_global(&mut self, name: GlobalName) -> Option<Box<dyn Global>> {
        self.globals.remove(name)
    }

    pub fn insert_output(&mut self, output: Output) -> OutputId {
        let output_id = self.outputs.insert(output);
        self.active_output.get_or_insert(output_id);
        output_id
    }

    pub fn remove_output(&mut self, output_id: OutputId) -> Option<Output> {
        let removed = self.outputs.remove(output_id)?;
        if self.active_output == Some(output_id) {
            self.active_output = self.outputs.iter().next().map(|(other_id, _)| other_id);
        }
        Some(removed)
    }

    pub fn active_output_id(&self) -> Option<OutputId> {
        self.active_output
            .or_else(|| self.outputs.iter().next().map(|(output_id, _)| output_id))
    }

    pub fn backend_for_output(&self, output_id: OutputId) -> Option<BackendOutputRef> {
        self.outputs
            .try_get(output_id)
            .map(|output| output.backend())
    }

    pub fn refresh_output_from_backend(&mut self, output_id: OutputId) -> bool {
        let Some(backend) = self.backend_for_output(output_id) else {
            return false;
        };
        let Some(drm) = self.drm.try_get(backend.drm_id) else {
            return false;
        };
        let Some(output) = self.outputs.try_get_mut(output_id) else {
            return false;
        };
        output.refresh_from_backend(drm);
        true
    }

    pub fn refresh_outputs_for_backend(&mut self, backend: BackendOutputRef) {
        let Some(drm) = self.drm.try_get(backend.drm_id) else {
            return;
        };
        let output_ids = self
            .outputs
            .iter()
            .filter_map(|(output_id, output)| (output.backend == backend).then_some(output_id))
            .collect::<Vec<_>>();

        for output_id in output_ids {
            if let Some(output) = self.outputs.try_get_mut(output_id) {
                output.refresh_from_backend(drm);
            }
        }
    }

    pub fn output_ids_for_backend(&self, backend: BackendOutputRef) -> Vec<OutputId> {
        self.outputs
            .iter()
            .filter_map(|(output_id, output)| (output.backend == backend).then_some(output_id))
            .collect()
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

    #[test]
    fn indexmap_try_grab_empty_slot() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        assert!(map.try_grab(ObjectId::from(0usize)).is_none());
    }

    #[test]
    fn indexmap_try_grab_roundtrip() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let k = map.insert(99);
        {
            let g = map.try_grab(k).unwrap();
            assert_eq!(*g, 99);
        }
        assert_eq!(*map.get(k), 99);
    }

    #[test]
    fn indexmap_remove_or_defer_while_grabbed() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let k = map.insert(42);
        assert!(map.contains(k));
        let g = map.grab(k);
        assert!(map.contains(k));
        assert!(map.remove_or_defer(k).is_none());
        assert!(!map.contains(k));
        drop(g);
        assert!(!map.contains(k));
    }
}
