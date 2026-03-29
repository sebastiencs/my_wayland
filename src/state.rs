use crate::{
    clients::ClientId,
    config::{OutputScaleSetting, RuntimeConfig},
    drm::Drm,
    egl::EGL,
    globals::{Global, GlobalName, Globals},
    input::Input,
    object::Object,
    protocol_generated::{
        WaylandId, WlDataOfferId, WlDataSourceId, WlSeatId, WlSurfaceId, XdgSurfaceId,
        ZwlrLayerSurfaceV1Id,
    },
    region::RectI,
    seat::SeatState,
    tiling::{self, TiledResizeState, WorkspaceLayout},
    wayland_objects::{wl_surface::WlSurface, xdg_surface::xdg_toplevel::XdgToplevel},
    xkb::{KeyboardState, Xkb},
};

use {
    rand::{rngs::OsRng, RngCore},
    std::{
        collections::{BTreeMap, BTreeSet},
        marker::PhantomData,
        ptr::NonNull,
        rc::Rc,
    },
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

pub struct IndexMap<K, V> {
    map: Vec<Slot<V>>,
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
static_assertions::assert_not_impl_all!(Grab<ObjectId, usize>: Send, Sync);

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

    pub fn try_new(
        mut grabbed: Grab<ObjectId, Box<dyn Object>>,
    ) -> Result<Self, Grab<ObjectId, Box<dyn Object>>> {
        let value: Box<dyn Object> = grabbed.value.take().unwrap();
        let value = match value.downcast() {
            Ok(value) => value,
            Err(value) => {
                grabbed.value = Some(value);
                return Err(grabbed);
            }
        };
        Ok(Self { value: Some(value), grabbed, _phantom: PhantomData })
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

    /// Remove immediately if `key` holds a value; if the slot is grabbed, mark for removal and
    /// return `None` (value is dropped when the active `Grab` is released).
    ///
    /// Returns `None` if the slot is empty, out of range, or removal was deferred.
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

    /// NOTE: Allow grabbing only on `Box<Self>`, so that self is a stable pointer
    pub fn grab(self: &mut Box<Self>, key: K) -> Grab<K, V> {
        let slot = self.slot_mut(key).expect("invalid key");

        let value = match std::mem::replace(&mut slot.value, ValueOrEmpty::Grabbed) {
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

    /// Like [`Self::grab`], but returns [`None`] if the slot is empty, out of range.
    pub fn try_grab(self: &mut Box<Self>, key: K) -> Option<Grab<K, V>> {
        let slot = self.slot_mut(key)?;
        match &slot.value {
            ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => return None,
            ValueOrEmpty::Grabbed => panic!("invalid state"),
            ValueOrEmpty::Value(_) => {}
        }
        let value = match std::mem::replace(&mut slot.value, ValueOrEmpty::Grabbed) {
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

impl IndexMap<ObjectId, Box<dyn Object>> {
    pub fn try_ref_object<T: Object>(&self, key: ObjectId) -> Option<&T> {
        self.try_get(key)
            .and_then(|object| object.downcast_ref::<T>())
    }

    pub fn try_grab_object<T: Object>(
        self: &mut Box<Self>,
        key: ObjectId,
    ) -> Option<GrabbedObject<T>> {
        let grabbed = self.try_grab(key)?;
        GrabbedObject::try_new(grabbed).ok()
    }

    pub fn grab_object<T: Object>(self: &mut Box<Self>, key: ObjectId) -> GrabbedObject<T> {
        GrabbedObject::new(self.grab(key))
    }
}

index_map_id!(ObjectId);
index_map_id!(InputId);
index_map_id!(DrmId);
index_map_id!(OutputId);
index_map_id!(WorkspaceGroupId);
index_map_id!(WorkspaceId);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct FractionalScale(u32);

impl FractionalScale {
    pub const DENOMINATOR: u32 = 120;
    pub const ONE: Self = Self(Self::DENOMINATOR);
    pub const ONE_POINT_FIVE: Self = Self(180);
    pub const TWO: Self = Self(240);

    pub const fn from_v120(raw: u32) -> Self {
        Self(if raw == 0 { 1 } else { raw })
    }

    pub const fn from_integer(scale: u32) -> Self {
        Self::from_v120(scale.saturating_mul(Self::DENOMINATOR))
    }

    pub fn try_from_f64(scale: f64) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let raw = (scale * f64::from(Self::DENOMINATOR)).round();
        (raw >= 1.0 && raw <= f64::from(u32::MAX)).then_some(Self(raw as u32))
    }

    pub const fn as_v120(self) -> u32 {
        self.0
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / f64::from(Self::DENOMINATOR)
    }

    pub fn ceil(self) -> i32 {
        self.0.div_ceil(Self::DENOMINATOR) as i32
    }

    pub fn logical_length(self, pixels: i32) -> i32 {
        if pixels <= 0 {
            return 0;
        }
        let pixels = i64::from(pixels);
        let scale = i64::from(self.0);
        ((pixels * i64::from(Self::DENOMINATOR)) + (scale / 2)).div_euclid(scale) as i32
    }
}

impl Default for FractionalScale {
    fn default() -> Self {
        Self::ONE
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTransformMatrix {
    pub values: [f32; 9],
}

impl ColorTransformMatrix {
    pub const fn identity() -> Self {
        Self { values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] }
    }
}

#[derive(Clone, Debug)]
pub struct Output {
    pub backend: BackendOutputRef,
    pub drm_crtc_id: u32,
    pub global_name: Option<GlobalName>,
    pub gamma_lut_size: u32,
    pub ctm_supported: bool,
    pub gamma_control_object_id: Option<ObjectId>,
    pub x: i32,
    pub y: i32,
    pub physical_width: i32,
    pub physical_height: i32,
    pub scale: FractionalScale,
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
    pub fn new(backend: BackendOutputRef, drm_crtc_id: u32, drm: &Drm) -> Self {
        let mut output = Self {
            backend,
            drm_crtc_id,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
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

    pub fn drm_crtc_id(&self) -> u32 {
        self.drm_crtc_id
    }

    pub fn refresh_from_backend(&mut self, drm: &Drm) {
        let Some(in_use) = drm.output_connector_in_use(self.drm_crtc_id) else {
            self.enabled = false;
            self.gamma_lut_size = 0;
            self.ctm_supported = false;
            return;
        };
        let connector = &in_use.connector;

        let preferred_mode = connector
            .preferred_mode
            .as_ref()
            .map(Self::mode_from_connector);
        let current_mode = drm
            .current_output_mode(self.drm_crtc_id)
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
        self.gamma_lut_size = drm.output_gamma_size(self.drm_crtc_id);
        self.ctm_supported = drm.output_supports_ctm(self.drm_crtc_id);
        self.current_mode = current_mode;
        self.preferred_mode = preferred_mode;
    }

    pub fn configured_scale(&self, config: &RuntimeConfig) -> FractionalScale {
        match config.output_scale(self.name.as_str()) {
            Some(OutputScaleSetting::Fixed(scale)) => scale,
            Some(OutputScaleSetting::Auto) => self.auto_scale(),
            None => FractionalScale::ONE,
        }
    }

    pub fn auto_scale(&self) -> FractionalScale {
        if !self.enabled || self.physical_width <= 0 || self.physical_height <= 0 {
            return FractionalScale::ONE;
        }

        let diagonal_px =
            f64::from(self.current_mode.width).hypot(f64::from(self.current_mode.height));
        if diagonal_px <= 0.0 {
            return FractionalScale::ONE;
        }

        let mm_per_inch = 25.4;
        let diagonal_in = (f64::from(self.physical_width) / mm_per_inch)
            .hypot(f64::from(self.physical_height) / mm_per_inch);
        if diagonal_in <= 0.0 {
            return FractionalScale::ONE;
        }

        let ppi = diagonal_px / diagonal_in;
        if ppi > 200.0 {
            FractionalScale::TWO
        } else if ppi > 140.0 {
            FractionalScale::ONE_POINT_FIVE
        } else {
            FractionalScale::ONE
        }
    }

    pub fn legacy_scale(&self) -> i32 {
        self.scale.ceil()
    }

    pub fn logical_size(&self) -> (i32, i32) {
        let (width, height) = match self.transform {
            1 | 3 | 5 | 7 => (self.current_mode.height, self.current_mode.width),
            _ => (self.current_mode.width, self.current_mode.height),
        };
        (self.scale.logical_length(width), self.scale.logical_length(height))
    }

    pub fn logical_bounds(&self) -> Option<RectI> {
        let (width, height) = self.logical_size();
        RectI::new(self.x, self.y, width, height)
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

#[derive(Clone, Debug, Default)]
pub struct WorkspaceGroup {
    pub outputs: BTreeSet<OutputId>,
    pub workspaces: Vec<WorkspaceId>,
    pub active_workspace: Option<WorkspaceId>,
}

#[derive(Debug)]
pub struct Workspace {
    pub stable_id: String,
    pub name: String,
    pub coordinates: Vec<u32>,
    pub group: Option<WorkspaceGroupId>,
    pub root_surfaces: BTreeSet<ObjectId>,
    pub layout: WorkspaceLayout,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ManagedWindowMode {
    #[default]
    Tiled,
    Floating,
}

#[derive(Clone, Copy, Debug)]
pub struct ManagedRootSurfaceState {
    pub workspace_id: WorkspaceId,
    pub mode: ManagedWindowMode,
    pub fullscreen: bool,
    pub mapped: bool,
    pub last_floating_rect: Option<RectI>,
    pub pending_floating_rect_restore: Option<RectI>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputLayoutEntry {
    pub output_id: OutputId,
    pub backend: BackendOutputRef,
    pub bounds: RectI,
}

impl OutputLayoutEntry {
    pub fn local_bounds(&self) -> RectI {
        self.bounds.translate(-self.bounds.x, -self.bounds.y)
    }

    pub fn global_to_local_point(&self, x: i32, y: i32) -> (i32, i32) {
        (x.saturating_sub(self.bounds.x), y.saturating_sub(self.bounds.y))
    }

    pub fn global_to_local_rect(&self, rect: RectI) -> RectI {
        rect.translate(-self.bounds.x, -self.bounds.y)
    }

    pub fn overlap_area(&self, rect: RectI) -> i64 {
        let Some(overlap) = self.bounds.intersection(rect) else {
            return 0;
        };
        overlap.width as i64 * overlap.height as i64
    }
}

#[derive(Clone, Debug, Default)]
pub struct OutputLayout {
    entries: Vec<OutputLayoutEntry>,
    extents: Option<RectI>,
}

impl OutputLayout {
    pub fn from_outputs(outputs: &IndexMap<OutputId, Output>) -> Self {
        let entries = outputs
            .iter()
            .filter_map(|(output_id, output)| {
                if !output.enabled {
                    return None;
                }
                Some(OutputLayoutEntry {
                    output_id,
                    backend: output.backend(),
                    bounds: output.logical_bounds()?,
                })
            })
            .collect::<Vec<_>>();
        let extents = entries
            .iter()
            .map(|entry| entry.bounds)
            .reduce(|acc, bounds| {
                RectI::from_extents(
                    acc.x.min(bounds.x),
                    acc.y.min(bounds.y),
                    acc.x2().max(bounds.x2()),
                    acc.y2().max(bounds.y2()),
                )
                .expect("output layout extents must stay valid")
            });
        Self { entries, extents }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[OutputLayoutEntry] {
        &self.entries
    }

    pub fn extents(&self) -> Option<RectI> {
        self.extents
    }

    pub fn entry(&self, output_id: OutputId) -> Option<&OutputLayoutEntry> {
        self.entries
            .iter()
            .find(|entry| entry.output_id == output_id)
    }

    pub fn bounds(&self, output_id: OutputId) -> Option<RectI> {
        self.entry(output_id).map(|entry| entry.bounds)
    }

    pub fn output_at(&self, x: i32, y: i32) -> Option<OutputId> {
        self.entries
            .iter()
            .find(|entry| entry.bounds.contains(x, y))
            .map(|entry| entry.output_id)
    }

    pub fn nearest_point(&self, x: i32, y: i32) -> Option<(OutputId, (i32, i32))> {
        let mut best = None;
        let mut best_distance = i64::MAX;
        for entry in &self.entries {
            let clamped_x = x.clamp(entry.bounds.x, entry.bounds.x2().saturating_sub(1));
            let clamped_y = y.clamp(entry.bounds.y, entry.bounds.y2().saturating_sub(1));
            let dx = i64::from(x.saturating_sub(clamped_x));
            let dy = i64::from(y.saturating_sub(clamped_y));
            let distance = dx * dx + dy * dy;
            if distance < best_distance {
                best_distance = distance;
                best = Some((entry.output_id, (clamped_x, clamped_y)));
            }
        }
        best
    }

    pub fn intersecting(&self, rect: RectI) -> Vec<OutputId> {
        self.entries
            .iter()
            .filter_map(|entry| entry.bounds.intersects(rect).then_some(entry.output_id))
            .collect()
    }

    pub fn primary_output_for_rect(
        &self,
        rect: RectI,
        preferred_output_id: Option<OutputId>,
    ) -> Option<OutputId> {
        let mut best = None;
        let mut best_area = -1i64;
        let mut best_is_preferred = false;
        for entry in &self.entries {
            let area = entry.overlap_area(rect);
            if area <= 0 {
                continue;
            }
            let is_preferred = Some(entry.output_id) == preferred_output_id;
            if area > best_area || (area == best_area && is_preferred && !best_is_preferred) {
                best = Some(entry.output_id);
                best_area = area;
                best_is_preferred = is_preferred;
            }
        }
        best.or(preferred_output_id.filter(|output_id| self.entry(*output_id).is_some()))
    }
}

const ACTIVATION_TOKEN_TTL_MSEC: u64 = 30_000;

#[derive(Clone, Debug)]
pub struct ActivationTokenData {
    pub client_id: ClientId,
    pub seat_id: Option<WlSeatId>,
    pub serial: Option<u32>,
    pub surface_id: Option<WlSurfaceId>,
    pub app_id: Option<String>,
    pub committed_at_msec: u64,
    pub expires_at_msec: u64,
}

#[derive(Debug, Default)]
pub struct ActivationTokenStore {
    tokens: BTreeMap<String, ActivationTokenData>,
}

impl ActivationTokenStore {
    fn monotonic_time_msec() -> u64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
            return 0;
        }
        (ts.tv_sec as u64)
            .saturating_mul(1000)
            .saturating_add((ts.tv_nsec as u64) / 1_000_000)
    }

    fn format_16_bytes_hex(bytes: &[u8; 16]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(32);
        for byte in bytes {
            s.push(HEX[(byte >> 4) as usize] as char);
            s.push(HEX[(byte & 0xf) as usize] as char);
        }
        s
    }

    /// 128 bits from the OS RNG, encoded as 32 lowercase hex digits (same shape as wlroots).
    fn random_hex_token() -> String {
        let mut buf = [0u8; 16];
        OsRng.fill_bytes(&mut buf);
        Self::format_16_bytes_hex(&buf)
    }

    fn random_activation_token(&mut self) -> String {
        loop {
            let s = Self::random_hex_token();
            if !self.tokens.contains_key(&s) {
                return s;
            }
        }
    }

    pub fn issue_tracked(
        &mut self,
        client_id: ClientId,
        seat_id: Option<WlSeatId>,
        serial: Option<u32>,
        surface_id: Option<WlSurfaceId>,
        app_id: Option<String>,
    ) -> String {
        self.purge_expired();
        let committed_at_msec = Self::monotonic_time_msec();
        let expires_at_msec = committed_at_msec.saturating_add(ACTIVATION_TOKEN_TTL_MSEC);
        let token = self.random_activation_token();
        self.tokens.insert(
            token.clone(),
            ActivationTokenData {
                client_id,
                seat_id,
                serial,
                surface_id,
                app_id,
                committed_at_msec,
                expires_at_msec,
            },
        );
        token
    }

    pub fn issue_untracked(&mut self) -> String {
        self.purge_expired();
        Self::random_hex_token()
    }

    pub fn consume(&mut self, token: &str) -> Option<ActivationTokenData> {
        self.purge_expired();
        self.tokens.remove(token)
    }

    pub fn purge_expired(&mut self) {
        let now = Self::monotonic_time_msec();
        self.tokens.retain(|_, data| data.expires_at_msec > now);
    }

    pub fn remove_client(&mut self, client_id: ClientId) {
        self.tokens.retain(|_, data| data.client_id != client_id);
    }

    #[cfg(test)]
    pub fn expire_now(&mut self, token: &str) -> bool {
        let now = Self::monotonic_time_msec();
        let Some(data) = self.tokens.get_mut(token) else {
            return false;
        };
        data.expires_at_msec = now;
        true
    }

    #[cfg(test)]
    pub fn first_token(&self) -> Option<String> {
        self.tokens.keys().next().cloned()
    }

    #[cfg(test)]
    pub fn tracked_count(&self) -> usize {
        self.tokens.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataOfferRole {
    Selection,
    Drag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionSourceState {
    pub source_object_id: ObjectId,
    pub source_id: WlDataSourceId,
    pub owner_client_id: ClientId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DragTargetState {
    pub client_id: ClientId,
    pub surface_id: WlSurfaceId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPhase {
    Dragging,
    DropSent,
    Finished,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DragNegotiationState {
    pub offer_ids: Vec<WlDataOfferId>,
    pub accepted_mime: Option<String>,
    pub destination_actions: u32,
    pub preferred_action: u32,
    pub compositor_action: u32,
    pub chosen_action: u32,
    pub finished: bool,
    pub has_finish_capable_offer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragState {
    pub source_object_id: Option<ObjectId>,
    pub source_id: Option<WlDataSourceId>,
    pub source_client_id: ClientId,
    pub origin_surface_id: WlSurfaceId,
    pub icon_surface_object_id: Option<ObjectId>,
    pub icon_surface_id: Option<WlSurfaceId>,
    pub target: Option<DragTargetState>,
    pub negotiation: DragNegotiationState,
    pub phase: DragPhase,
}

pub struct State {
    pub objects: Box<IndexMap<ObjectId, Box<dyn Object>>>,
    pub root_surfaces: Vec<ObjectId>,
    pub layer_surface_objects: BTreeMap<ZwlrLayerSurfaceV1Id, ObjectId>,
    pub inputs: IndexMap<InputId, Input>,
    pub drm: IndexMap<DrmId, Drm>,
    pub outputs: IndexMap<OutputId, Output>,
    pub active_output: Option<OutputId>,
    pub workspace_groups: IndexMap<WorkspaceGroupId, WorkspaceGroup>,
    pub workspaces: IndexMap<WorkspaceId, Workspace>,
    pub output_workspace_groups: BTreeMap<OutputId, WorkspaceGroupId>,
    pub root_surface_workspaces: BTreeMap<ObjectId, WorkspaceId>,
    pub managed_root_surfaces: BTreeMap<ObjectId, ManagedRootSurfaceState>,
    pub xdg_toplevel_roles: BTreeMap<(ClientId, XdgSurfaceId), ObjectId>,
    pub next_workspace_serial: u32,
    pub backend_syncobj_globals: BTreeMap<DrmId, GlobalName>,
    pub globals: Globals,
    pub activation_tokens: ActivationTokenStore,
    pub egl: EGL,
    pub config: RuntimeConfig,
    pub seat: SeatState,
    pub keyboard: KeyboardState,
    pub xkb: Xkb,
}

impl State {
    fn next_workspace_stable_id(&mut self) -> String {
        let serial = self.next_workspace_serial;
        self.next_workspace_serial = self.next_workspace_serial.saturating_add(1);
        format!("workspace-{serial}")
    }

    fn primary_output_for_workspace_group(&self, group_id: WorkspaceGroupId) -> Option<OutputId> {
        self.workspace_groups
            .try_get(group_id)
            .and_then(|group| group.outputs.iter().copied().next())
    }

    fn default_workspace_name_for_group(&self, group_id: WorkspaceGroupId) -> String {
        self.workspace_groups
            .try_get(group_id)
            .map(|group| group.workspaces.len() + 1)
            .unwrap_or(1)
            .to_string()
    }

    fn refresh_workspace_coordinates_for_group(&mut self, group_id: WorkspaceGroupId) {
        let Some(workspace_ids) = self
            .workspace_groups
            .try_get(group_id)
            .map(|group| group.workspaces.clone())
        else {
            return;
        };
        for (index, workspace_id) in workspace_ids.into_iter().enumerate() {
            let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
                continue;
            };
            workspace.coordinates = vec![(index + 1) as u32];
        }
    }

    fn clear_workspace_coordinates(&mut self, workspace_id: WorkspaceId) {
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            return;
        };
        workspace.coordinates.clear();
    }

    pub fn workspace_group_for_output(&self, output_id: OutputId) -> Option<WorkspaceGroupId> {
        self.output_workspace_groups.get(&output_id).copied()
    }

    pub fn active_workspace_for_group(&self, group_id: WorkspaceGroupId) -> Option<WorkspaceId> {
        self.workspace_groups
            .try_get(group_id)
            .and_then(|group| group.active_workspace)
    }

    pub fn active_workspace_for_output(&self, output_id: OutputId) -> Option<WorkspaceId> {
        self.workspace_group_for_output(output_id)
            .and_then(|group_id| self.active_workspace_for_group(group_id))
    }

    pub fn root_surface_workspace(&self, object_id: ObjectId) -> Option<WorkspaceId> {
        self.root_surface_workspaces.get(&object_id).copied()
    }

    pub fn managed_root_surface_state(
        &self,
        object_id: ObjectId,
    ) -> Option<&ManagedRootSurfaceState> {
        self.managed_root_surfaces.get(&object_id)
    }

    pub fn take_pending_floating_rect_restore(&mut self, object_id: ObjectId) -> Option<RectI> {
        self.managed_root_surfaces
            .get_mut(&object_id)
            .and_then(|managed| managed.pending_floating_rect_restore.take())
    }

    pub fn register_xdg_toplevel_role(
        &mut self,
        client_id: ClientId,
        xdg_surface_id: XdgSurfaceId,
        object_id: ObjectId,
    ) {
        self.xdg_toplevel_roles
            .insert((client_id, xdg_surface_id), object_id);
    }

    pub fn unregister_xdg_toplevel_role(
        &mut self,
        client_id: ClientId,
        xdg_surface_id: XdgSurfaceId,
        object_id: ObjectId,
    ) {
        if self.xdg_toplevel_roles.get(&(client_id, xdg_surface_id)) == Some(&object_id) {
            self.xdg_toplevel_roles.remove(&(client_id, xdg_surface_id));
        }
    }

    pub fn xdg_toplevel_object_id(
        &self,
        client_id: ClientId,
        xdg_surface_id: XdgSurfaceId,
    ) -> Option<ObjectId> {
        self.xdg_toplevel_roles
            .get(&(client_id, xdg_surface_id))
            .copied()
            .filter(|object_id| self.objects.contains(*object_id))
    }

    pub fn workspace_ids(&self) -> Vec<WorkspaceId> {
        self.workspaces
            .iter()
            .map(|(workspace_id, _)| workspace_id)
            .collect()
    }

    pub fn workspace_group_ids(&self) -> Vec<WorkspaceGroupId> {
        self.workspace_groups
            .iter()
            .map(|(group_id, _)| group_id)
            .collect()
    }

    pub fn create_workspace_group_for_output(&mut self, output_id: OutputId) -> WorkspaceGroupId {
        if let Some(group_id) = self.workspace_group_for_output(output_id) {
            return group_id;
        }
        let group_id = self.workspace_groups.insert(WorkspaceGroup {
            outputs: BTreeSet::from([output_id]),
            workspaces: Vec::new(),
            active_workspace: None,
        });
        self.output_workspace_groups.insert(output_id, group_id);
        let workspace_id = self.create_workspace(Some(group_id), None);
        if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
            group.active_workspace = Some(workspace_id);
        }
        group_id
    }

    pub fn create_workspace(
        &mut self,
        group_id: Option<WorkspaceGroupId>,
        requested_name: Option<String>,
    ) -> WorkspaceId {
        let group_id =
            group_id.filter(|group_id| self.workspace_groups.try_get(*group_id).is_some());
        let name = requested_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                group_id
                    .map(|group_id| self.default_workspace_name_for_group(group_id))
                    .unwrap_or_else(|| "workspace".to_string())
            });
        let stable_id = self.next_workspace_stable_id();
        let workspace_id = self.workspaces.insert(Workspace {
            stable_id,
            name,
            coordinates: Vec::new(),
            group: group_id,
            root_surfaces: BTreeSet::new(),
            layout: WorkspaceLayout::default(),
        });
        if let Some(group_id) = group_id {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group.workspaces.push(workspace_id);
                if group.active_workspace.is_none() {
                    group.active_workspace = Some(workspace_id);
                }
            }
            self.refresh_workspace_coordinates_for_group(group_id);
        }
        workspace_id
    }

    fn ensure_group_has_active_workspace(&mut self, group_id: WorkspaceGroupId) {
        let Some(group_snapshot) = self.workspace_groups.try_get(group_id).cloned() else {
            return;
        };
        let still_valid = group_snapshot
            .active_workspace
            .is_some_and(|workspace_id| group_snapshot.workspaces.contains(&workspace_id));
        if still_valid {
            return;
        }
        if let Some(next_active) = group_snapshot.workspaces.first().copied() {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group.active_workspace = Some(next_active);
            }
            return;
        }
        if group_snapshot.outputs.is_empty() {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group.active_workspace = None;
            }
            return;
        }
        let workspace_id = self.create_workspace(Some(group_id), None);
        if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
            group.active_workspace = Some(workspace_id);
        }
    }

    pub fn set_workspace_name(
        &mut self,
        workspace_id: WorkspaceId,
        requested_name: impl Into<String>,
    ) -> bool {
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            return false;
        };
        let name = requested_name.into();
        if name.trim().is_empty() || workspace.name == name {
            return false;
        }
        workspace.name = name;
        true
    }

    pub fn assign_workspace_to_group(
        &mut self,
        workspace_id: WorkspaceId,
        new_group_id: Option<WorkspaceGroupId>,
    ) -> bool {
        let new_group_id = match new_group_id {
            Some(group_id) if self.workspace_groups.try_get(group_id).is_some() => Some(group_id),
            Some(_) => return false,
            None => None,
        };
        let Some(old_group_id) = self.workspaces.try_get(workspace_id).and_then(|w| w.group) else {
            if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
                workspace.group = new_group_id;
            }
            if let Some(group_id) = new_group_id {
                if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                    if !group.workspaces.contains(&workspace_id) {
                        group.workspaces.push(workspace_id);
                    }
                    if group.active_workspace.is_none() {
                        group.active_workspace = Some(workspace_id);
                    }
                }
                self.refresh_workspace_coordinates_for_group(group_id);
            } else {
                self.clear_workspace_coordinates(workspace_id);
            }
            return true;
        };
        if Some(old_group_id) == new_group_id {
            return false;
        }
        if let Some(old_group) = self.workspace_groups.try_get_mut(old_group_id) {
            old_group
                .workspaces
                .retain(|candidate| *candidate != workspace_id);
            if old_group.active_workspace == Some(workspace_id) {
                old_group.active_workspace = old_group.workspaces.first().copied();
            }
        }
        if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
            workspace.group = new_group_id;
        }
        if let Some(group_id) = new_group_id {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                if !group.workspaces.contains(&workspace_id) {
                    group.workspaces.push(workspace_id);
                }
                if group.active_workspace.is_none() {
                    group.active_workspace = Some(workspace_id);
                }
            }
        }
        self.refresh_workspace_coordinates_for_group(old_group_id);
        self.ensure_group_has_active_workspace(old_group_id);
        if let Some(group_id) = new_group_id {
            self.refresh_workspace_coordinates_for_group(group_id);
        } else {
            self.clear_workspace_coordinates(workspace_id);
        }
        true
    }

    pub fn activate_workspace(&mut self, workspace_id: WorkspaceId) -> bool {
        let previous_workspace = self
            .workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.active_workspace_for_group(group_id));
        let Some(group_id) = self
            .workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
        else {
            return false;
        };
        let Some(group) = self.workspace_groups.try_get_mut(group_id) else {
            return false;
        };
        if group.active_workspace == Some(workspace_id) {
            return false;
        }
        group.active_workspace = Some(workspace_id);
        if let Some(previous_workspace) = previous_workspace {
            tiling::mark_workspace_dirty(self, previous_workspace);
        }
        tiling::mark_workspace_dirty(self, workspace_id);
        true
    }

    pub fn assign_root_surface_to_workspace(
        &mut self,
        object_id: ObjectId,
        workspace_id: WorkspaceId,
    ) -> bool {
        if self.root_surface_workspaces.get(&object_id).copied() == Some(workspace_id) {
            return false;
        }
        if let Some(previous_workspace_id) =
            self.root_surface_workspaces.insert(object_id, workspace_id)
        {
            if let Some(previous_workspace) = self.workspaces.try_get_mut(previous_workspace_id) {
                previous_workspace.root_surfaces.remove(&object_id);
                if previous_workspace
                    .layout
                    .fullscreen
                    .as_ref()
                    .is_some_and(|fullscreen| fullscreen.surface_object_id == object_id)
                {
                    previous_workspace.layout.set_fullscreen(None);
                }
            }
            tiling::mark_workspace_dirty(self, previous_workspace_id);
        }
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            self.root_surface_workspaces.remove(&object_id);
            return false;
        };
        workspace.root_surfaces.insert(object_id);
        workspace.layout.mark_dirty();
        self.managed_root_surfaces
            .entry(object_id)
            .and_modify(|managed| managed.workspace_id = workspace_id)
            .or_insert(ManagedRootSurfaceState {
                workspace_id,
                mode: ManagedWindowMode::Tiled,
                fullscreen: false,
                mapped: false,
                last_floating_rect: None,
                pending_floating_rect_restore: None,
            });
        true
    }

    pub fn detach_root_surface_from_workspace(
        &mut self,
        object_id: ObjectId,
    ) -> Option<WorkspaceId> {
        let workspace_id = self.root_surface_workspaces.remove(&object_id)?;
        if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
            workspace.root_surfaces.remove(&object_id);
            workspace.layout.remove_surface(object_id);
            if workspace
                .layout
                .fullscreen
                .as_ref()
                .is_some_and(|fullscreen| fullscreen.surface_object_id == object_id)
            {
                workspace.layout.set_fullscreen(None);
            }
        }
        self.managed_root_surfaces.remove(&object_id);
        Some(workspace_id)
    }

    pub fn inherit_root_surface_workspace(
        &mut self,
        object_id: ObjectId,
        parent_object_id: ObjectId,
    ) -> Option<WorkspaceId> {
        let workspace_id = self.root_surface_workspace(parent_object_id)?;
        self.assign_root_surface_to_workspace(object_id, workspace_id);
        Some(workspace_id)
    }

    pub fn ensure_root_surface_workspace(&mut self, object_id: ObjectId) -> Option<WorkspaceId> {
        if let Some(workspace_id) = self.root_surface_workspace(object_id) {
            return Some(workspace_id);
        }
        let workspace_id = self
            .active_output_id()
            .and_then(|output_id| self.active_workspace_for_output(output_id))
            .or_else(|| {
                self.workspace_groups
                    .iter()
                    .find_map(|(_, group)| group.active_workspace)
            })?;
        self.assign_root_surface_to_workspace(object_id, workspace_id);
        Some(workspace_id)
    }

    pub fn workspace_output_id(&self, workspace_id: WorkspaceId) -> Option<OutputId> {
        self.workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.primary_output_for_workspace_group(group_id))
    }

    pub fn workspace_output_ids(&self, workspace_id: WorkspaceId) -> Vec<OutputId> {
        self.workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.workspace_groups.try_get(group_id))
            .map(|group| group.outputs.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn workspace_is_active(&self, workspace_id: WorkspaceId) -> bool {
        self.workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.workspace_groups.try_get(group_id))
            .is_some_and(|group| group.active_workspace == Some(workspace_id))
    }

    pub fn root_surface_is_visible(&self, object_id: ObjectId) -> bool {
        self.root_surface_workspace(object_id)
            .map(|workspace_id| {
                if !self.workspace_is_active(workspace_id) {
                    return false;
                }
                let Some(workspace) = self.workspaces.try_get(workspace_id) else {
                    return false;
                };
                workspace
                    .layout
                    .fullscreen
                    .as_ref()
                    .map(|fullscreen| fullscreen.surface_object_id == object_id)
                    .unwrap_or(true)
            })
            .unwrap_or(true)
    }

    pub fn relayout_dirty_workspaces(&mut self) {
        tiling::relayout_dirty_workspaces(self);
    }

    pub fn tiling_scene_root_surface_ids(&self) -> Vec<ObjectId> {
        tiling::managed_scene_order(self)
    }

    pub fn tiled_root_surface_rect(&self, object_id: ObjectId) -> Option<RectI> {
        tiling::tiled_rect_for_surface(self, object_id)
    }

    pub fn tiled_root_surface_edges(&self, object_id: ObjectId) -> Option<u32> {
        tiling::tiled_edges_for_surface(self, object_id)
    }

    pub fn root_surface_is_fullscreen(&self, object_id: ObjectId) -> bool {
        let Some(workspace_id) = self.root_surface_workspace(object_id) else {
            return false;
        };
        let Some(workspace) = self.workspaces.try_get(workspace_id) else {
            return false;
        };
        workspace
            .layout
            .fullscreen
            .as_ref()
            .is_some_and(|fullscreen| fullscreen.surface_object_id == object_id)
    }

    pub fn is_tiled_root_surface(&self, object_id: ObjectId) -> bool {
        tiling::is_tiled_surface(self, object_id)
    }

    pub fn is_floating_root_surface(&self, object_id: ObjectId) -> bool {
        tiling::is_floating_surface(self, object_id)
    }

    pub fn set_root_surface_floating(&mut self, object_id: ObjectId, floating: bool) -> bool {
        let Some(workspace_id) = self
            .managed_root_surfaces
            .get(&object_id)
            .map(|managed| managed.workspace_id)
            .or_else(|| self.root_surface_workspace(object_id))
        else {
            return false;
        };
        let current_bounds = self
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .and_then(|surface| surface.global_logical_bounds());
        let Some(managed) = self.managed_root_surfaces.get_mut(&object_id) else {
            return false;
        };
        let was_floating = managed.mode == ManagedWindowMode::Floating;
        let new_mode =
            if floating { ManagedWindowMode::Floating } else { ManagedWindowMode::Tiled };
        let mut changed = managed.mode != new_mode;
        if !floating && was_floating {
            if let Some(bounds) = current_bounds {
                managed.last_floating_rect = Some(bounds);
            }
        }
        managed.mode = new_mode;
        let restore_rect = floating.then_some(managed.last_floating_rect).flatten();
        managed.pending_floating_rect_restore = if floating { restore_rect } else { None };
        if floating {
            managed.fullscreen = false;
            if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
                if workspace
                    .layout
                    .fullscreen
                    .as_ref()
                    .is_some_and(|fullscreen| fullscreen.surface_object_id == object_id)
                {
                    changed = true;
                    workspace.layout.set_fullscreen(None);
                }
            }
        }
        if changed {
            tiling::mark_workspace_dirty(self, workspace_id);
            self.relayout_dirty_workspaces();
        }
        if floating {
            if let Some(rect) = restore_rect {
                self.restore_root_surface_floating_rect(object_id, rect);
            }
        }
        changed
    }

    pub fn set_root_surface_fullscreen(&mut self, object_id: ObjectId, fullscreen: bool) -> bool {
        let Some(workspace_id) = self
            .managed_root_surfaces
            .get(&object_id)
            .map(|managed| managed.workspace_id)
            .or_else(|| self.root_surface_workspace(object_id))
        else {
            return false;
        };
        let current_bounds = self
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .and_then(|surface| surface.global_logical_bounds());
        let mut restore_rect = None;
        if fullscreen {
            let managed_ids = self
                .managed_root_surfaces
                .iter()
                .filter_map(|(candidate_object_id, managed)| {
                    (managed.workspace_id == workspace_id && managed.fullscreen)
                        .then_some(*candidate_object_id)
                })
                .collect::<Vec<_>>();
            for candidate_object_id in managed_ids {
                if let Some(managed) = self.managed_root_surfaces.get_mut(&candidate_object_id) {
                    if candidate_object_id != object_id {
                        if managed.mode == ManagedWindowMode::Floating {
                            managed.pending_floating_rect_restore = managed.last_floating_rect;
                        }
                        managed.fullscreen = false;
                    }
                }
                if candidate_object_id != object_id {
                    self.sync_root_surface_requested_fullscreen(candidate_object_id, false);
                }
            }
            self.managed_root_surfaces
                .entry(object_id)
                .and_modify(|managed| {
                    if !managed.fullscreen && managed.mode == ManagedWindowMode::Floating {
                        if let Some(bounds) = current_bounds {
                            managed.last_floating_rect = Some(bounds);
                        }
                    }
                    managed.pending_floating_rect_restore = None;
                    managed.fullscreen = true;
                })
                .or_insert(ManagedRootSurfaceState {
                    workspace_id,
                    mode: ManagedWindowMode::Tiled,
                    fullscreen: true,
                    mapped: false,
                    last_floating_rect: None,
                    pending_floating_rect_restore: None,
                });
        } else {
            {
                let Some(managed) = self.managed_root_surfaces.get_mut(&object_id) else {
                    return false;
                };
                if !managed.fullscreen {
                    return false;
                }
                managed.fullscreen = false;
                if managed.mode == ManagedWindowMode::Floating {
                    restore_rect = managed.last_floating_rect;
                    managed.pending_floating_rect_restore = restore_rect;
                } else {
                    managed.pending_floating_rect_restore = None;
                }
            }
            self.sync_root_surface_requested_fullscreen(object_id, false);
        }
        tiling::mark_workspace_dirty(self, workspace_id);
        self.relayout_dirty_workspaces();
        if let Some(rect) = restore_rect {
            self.restore_root_surface_floating_rect(object_id, rect);
        }
        true
    }

    pub fn resize_tiled_root_surface(
        &mut self,
        object_id: ObjectId,
        edges: u32,
        dx: i32,
        dy: i32,
    ) -> bool {
        let changed = tiling::resize_tiled_surface(self, object_id, edges, dx, dy);
        if changed {
            self.relayout_dirty_workspaces();
        }
        changed
    }

    pub fn begin_tiled_root_surface_resize(
        &mut self,
        object_id: ObjectId,
        edges: u32,
    ) -> Option<TiledResizeState> {
        self.relayout_dirty_workspaces();
        let workspace_id = self.root_surface_workspace(object_id)?;
        let workspace = self.workspaces.try_get(workspace_id)?;
        workspace.layout.begin_tiled_resize(object_id, edges)
    }

    pub fn resize_tiled_root_surface_from_state(
        &mut self,
        object_id: ObjectId,
        resize_state: TiledResizeState,
        dx: i32,
        dy: i32,
    ) -> bool {
        let Some(workspace_id) = self.root_surface_workspace(object_id) else {
            return false;
        };
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            return false;
        };
        let changed =
            workspace
                .layout
                .resize_tiled_surface_from_state(object_id, resize_state, dx, dy);
        if changed {
            self.relayout_dirty_workspaces();
        }
        changed
    }

    pub fn move_workspace_to_group_output(
        &mut self,
        workspace_id: WorkspaceId,
        new_group_id: WorkspaceGroupId,
    ) {
        let old_output = self.workspace_output_id(workspace_id);
        if !self.assign_workspace_to_group(workspace_id, Some(new_group_id)) {
            return;
        }
        let Some(new_output) = self.workspace_output_id(workspace_id) else {
            return;
        };
        let Some(old_output) = old_output else {
            return;
        };
        if old_output == new_output {
            return;
        }
        let Some(old_bounds) = self.output_bounds(old_output) else {
            return;
        };
        let Some(new_bounds) = self.output_bounds(new_output) else {
            return;
        };
        let dx = new_bounds.x - old_bounds.x;
        let dy = new_bounds.y - old_bounds.y;
        if dx == 0 && dy == 0 {
            return;
        }
        for managed in self.managed_root_surfaces.values_mut() {
            if managed.workspace_id != workspace_id {
                continue;
            }
            managed.last_floating_rect = managed
                .last_floating_rect
                .map(|rect| rect.translate(dx, dy));
            managed.pending_floating_rect_restore = managed
                .pending_floating_rect_restore
                .map(|rect| rect.translate(dx, dy));
        }
        let Some(surface_ids) = self
            .workspaces
            .try_get(workspace_id)
            .map(|workspace| workspace.root_surfaces.iter().copied().collect::<Vec<_>>())
        else {
            return;
        };
        for surface_object_id in surface_ids {
            if self.is_tiled_root_surface(surface_object_id) {
                continue;
            }
            let Some(mut surface) = self.objects.try_grab_object::<WlSurface>(surface_object_id)
            else {
                continue;
            };
            let (x, y) = surface.root_position();
            surface.set_root_position(x + dx, y + dy);
        }
        tiling::mark_workspace_dirty(self, workspace_id);
        self.relayout_dirty_workspaces();
    }

    fn restore_root_surface_floating_rect(&mut self, object_id: ObjectId, rect: RectI) -> bool {
        let Some(mut surface) = self.objects.try_grab_object::<WlSurface>(object_id) else {
            return false;
        };
        surface.set_root_position(rect.x, rect.y);
        true
    }

    fn sync_root_surface_requested_fullscreen(&mut self, object_id: ObjectId, fullscreen: bool) {
        let Some((client_id, xdg_surface_id)) = self
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .and_then(|surface| {
                surface
                    .xdg_surface_id()
                    .map(|xdg_surface_id| (surface.client_id(), xdg_surface_id))
            })
        else {
            return;
        };
        let Some(toplevel_object_id) = self.xdg_toplevel_object_id(client_id, xdg_surface_id)
        else {
            return;
        };
        let Some(mut toplevel) = self
            .objects
            .try_grab_object::<XdgToplevel>(toplevel_object_id)
        else {
            return;
        };
        toplevel.set_requested_fullscreen(fullscreen);
    }

    pub fn can_remove_workspace(&self, workspace_id: WorkspaceId) -> bool {
        let Some(workspace) = self.workspaces.try_get(workspace_id) else {
            return false;
        };
        if !workspace.root_surfaces.is_empty() {
            return false;
        }
        let Some(group_id) = workspace.group else {
            return true;
        };
        let Some(group) = self.workspace_groups.try_get(group_id) else {
            return true;
        };
        group.workspaces.len() > 1 && group.active_workspace != Some(workspace_id)
    }

    pub fn remove_workspace(&mut self, workspace_id: WorkspaceId) -> bool {
        if !self.can_remove_workspace(workspace_id) {
            return false;
        }
        let old_group_id = self
            .workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group);
        if let Some(group_id) = old_group_id {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group
                    .workspaces
                    .retain(|candidate| *candidate != workspace_id);
                if group.active_workspace == Some(workspace_id) {
                    group.active_workspace = group.workspaces.first().copied();
                }
            }
            self.refresh_workspace_coordinates_for_group(group_id);
            self.ensure_group_has_active_workspace(group_id);
        }
        self.workspaces.remove(workspace_id).is_some()
    }

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
        if let Some(workspace_id) = self.ensure_root_surface_workspace(object_id) {
            tiling::mark_workspace_dirty(self, workspace_id);
        }
    }

    pub fn register_layer_surface_object(
        &mut self,
        layer_surface_id: ZwlrLayerSurfaceV1Id,
        object_id: ObjectId,
    ) {
        self.layer_surface_objects
            .insert(layer_surface_id, object_id);
    }

    pub fn unregister_layer_surface_object(&mut self, layer_surface_id: ZwlrLayerSurfaceV1Id) {
        self.layer_surface_objects.remove(&layer_surface_id);
    }

    pub fn layer_surface_object_id(
        &self,
        layer_surface_id: ZwlrLayerSurfaceV1Id,
    ) -> Option<ObjectId> {
        self.layer_surface_objects
            .get(&layer_surface_id)
            .copied()
            .filter(|object_id| self.objects.contains(*object_id))
    }

    pub fn unregister_root_surface(&mut self, object_id: ObjectId) {
        self.root_surfaces
            .retain(|candidate| *candidate != object_id);
        if let Some(workspace_id) = self.detach_root_surface_from_workspace(object_id) {
            tiling::mark_workspace_dirty(self, workspace_id);
        }
    }

    pub fn raise_root_surface(&mut self, object_id: ObjectId) -> bool {
        if !self.objects.contains(object_id) {
            return false;
        }
        if tiling::raise_surface(self, object_id) {
            return true;
        }
        self.register_root_surface(object_id);
        true
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
        let mut output = output;
        output.scale = output.configured_scale(&self.config);
        let output_id = self.outputs.insert(output);
        self.active_output.get_or_insert(output_id);
        self.create_workspace_group_for_output(output_id);
        output_id
    }

    pub fn remove_output(&mut self, output_id: OutputId) -> Option<Output> {
        let removed = self.outputs.remove(output_id)?;
        if let Some(group_id) = self.output_workspace_groups.remove(&output_id) {
            let fallback_group_id = self
                .output_workspace_groups
                .values()
                .copied()
                .find(|candidate| *candidate != group_id);
            let workspace_ids = self
                .workspace_groups
                .try_get(group_id)
                .map(|group| group.workspaces.clone())
                .unwrap_or_default();
            for workspace_id in workspace_ids {
                match fallback_group_id {
                    Some(fallback_group_id) => {
                        self.move_workspace_to_group_output(workspace_id, fallback_group_id)
                    }
                    None => {
                        let _ = self.assign_workspace_to_group(workspace_id, None);
                    }
                }
            }
            let _ = self.workspace_groups.remove(group_id);
        }
        if self.active_output == Some(output_id) {
            self.active_output = self.outputs.iter().next().map(|(other_id, _)| other_id);
        }
        Some(removed)
    }

    pub fn active_output_id(&self) -> Option<OutputId> {
        let (pointer_x, pointer_y) = self.seat.pointer_position();
        self.output_at_point(pointer_x.round() as i32, pointer_y.round() as i32)
            .or_else(|| {
                self.clamp_point_to_outputs(pointer_x.round() as i32, pointer_y.round() as i32)
                    .and_then(|(x, y)| self.output_at_point(x, y))
            })
            .or_else(|| {
                self.seat.keyboard_focus().and_then(|focus| {
                    self.surface_object_id(focus.surface_id)
                        .and_then(|object_id| self.objects.try_ref_object::<WlSurface>(object_id))
                        .and_then(|surface| self.layout_output_id_for_surface(surface))
                })
            })
            .or_else(|| self.fallback_output_id())
    }

    pub fn output_layout(&self) -> OutputLayout {
        OutputLayout::from_outputs(&self.outputs)
    }

    pub fn output_layout_bounds(&self) -> Option<RectI> {
        self.output_layout().extents()
    }

    pub fn output_bounds(&self, output_id: OutputId) -> Option<RectI> {
        self.output_layout().bounds(output_id)
    }

    pub fn output_at_point(&self, x: i32, y: i32) -> Option<OutputId> {
        self.output_layout().output_at(x, y)
    }

    pub fn clamp_point_to_outputs(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        self.output_layout()
            .nearest_point(x, y)
            .map(|(_, point)| point)
    }

    pub fn outputs_intersecting_rect(&self, rect: RectI) -> Vec<OutputId> {
        self.output_layout().intersecting(rect)
    }

    pub fn primary_output_for_rect(
        &self,
        rect: RectI,
        preferred_output_id: Option<OutputId>,
    ) -> Option<OutputId> {
        let preferred_output_id = preferred_output_id.or(self.active_output_id());
        self.output_layout()
            .primary_output_for_rect(rect, preferred_output_id)
    }

    pub fn output_id_for_surface_object(
        &self,
        surface_object_id: ObjectId,
        surface: &WlSurface,
    ) -> Option<OutputId> {
        self.layout_output_id_for_surface(surface)
            .or_else(|| {
                self.root_surface_workspace(surface_object_id)
                    .and_then(|workspace_id| self.workspace_output_id(workspace_id))
            })
            .or_else(|| self.active_output_id())
    }

    pub fn output_id_for_surface(&self, surface: &WlSurface) -> Option<OutputId> {
        let surface_object_id = self.surface_object_id(surface.surface_id());
        self.layout_output_id_for_surface(surface)
            .or_else(|| {
                surface_object_id
                    .and_then(|object_id| self.root_surface_workspace(object_id))
                    .and_then(|workspace_id| self.workspace_output_id(workspace_id))
            })
            .or_else(|| self.active_output_id())
    }

    pub fn backend_for_output(&self, output_id: OutputId) -> Option<BackendOutputRef> {
        self.outputs
            .try_get(output_id)
            .map(|output| output.backend())
    }

    pub fn output_crtc_id(&self, output_id: OutputId) -> Option<u32> {
        self.outputs.try_get(output_id).map(Output::drm_crtc_id)
    }

    pub fn output_gamma_lut_size(&self, output_id: OutputId) -> Option<u32> {
        self.outputs
            .try_get(output_id)
            .map(|output| output.gamma_lut_size)
    }

    pub fn output_supports_gamma_control(&self, output_id: OutputId) -> bool {
        self.output_gamma_lut_size(output_id)
            .is_some_and(|size| size > 0)
    }

    pub fn output_supports_ctm_control(&self, output_id: OutputId) -> bool {
        self.outputs
            .try_get(output_id)
            .is_some_and(|output| output.ctm_supported)
    }

    pub fn any_output_supports_gamma_control(&self) -> bool {
        self.outputs
            .iter()
            .any(|(_, output)| output.enabled && output.gamma_lut_size > 0)
    }

    pub fn any_output_supports_ctm_control(&self) -> bool {
        self.outputs
            .iter()
            .any(|(_, output)| output.enabled && output.ctm_supported)
    }

    pub fn client_allows_ctm_control(&self, _client: &crate::clients::Client) -> bool {
        true
    }

    pub fn gamma_control_owner(&self, output_id: OutputId) -> Option<ObjectId> {
        self.outputs
            .try_get(output_id)
            .and_then(|output| output.gamma_control_object_id)
    }

    pub fn set_gamma_control_owner(
        &mut self,
        output_id: OutputId,
        object_id: ObjectId,
    ) -> Result<(), std::io::Error> {
        let Some(output) = self.outputs.try_get_mut(output_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "output disappeared"));
        };
        output.gamma_control_object_id = Some(object_id);
        Ok(())
    }

    pub fn clear_gamma_control_owner(&mut self, output_id: OutputId, object_id: ObjectId) {
        let Some(output) = self.outputs.try_get_mut(output_id) else {
            return;
        };
        if output.gamma_control_object_id == Some(object_id) {
            output.gamma_control_object_id = None;
        }
    }

    pub fn set_output_gamma_lut(
        &mut self,
        output_id: OutputId,
        lut: Option<Vec<u16>>,
    ) -> Result<(), std::io::Error> {
        let Some(output) = self.outputs.try_get(output_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "output disappeared"));
        };
        let backend = output.backend;
        let drm_crtc_id = output.drm_crtc_id;
        let Some(drm) = self.drm.try_get_mut(backend.drm_id) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "backend disappeared for output",
            ));
        };
        drm.set_output_gamma_lut(drm_crtc_id, lut)
    }

    pub fn set_output_ctm(
        &mut self,
        output_id: OutputId,
        ctm: Option<ColorTransformMatrix>,
    ) -> Result<(), std::io::Error> {
        let Some(output) = self.outputs.try_get(output_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "output disappeared"));
        };
        let backend = output.backend;
        let drm_crtc_id = output.drm_crtc_id;
        let Some(drm) = self.drm.try_get_mut(backend.drm_id) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "backend disappeared for output",
            ));
        };
        drm.set_output_ctm(drm_crtc_id, ctm)
    }

    pub fn output_ids_for_drm(&self, drm_id: DrmId) -> Vec<OutputId> {
        self.outputs
            .iter()
            .filter_map(|(output_id, output)| {
                (output.backend.drm_id == drm_id).then_some(output_id)
            })
            .collect()
    }

    pub fn output_id_for_drm_crtc(&self, drm_id: DrmId, drm_crtc_id: u32) -> Option<OutputId> {
        self.outputs.iter().find_map(|(output_id, output)| {
            (output.backend.drm_id == drm_id && output.drm_crtc_id == drm_crtc_id)
                .then_some(output_id)
        })
    }

    pub fn active_output_id_for_drm(&self, drm_id: DrmId) -> Option<OutputId> {
        self.active_output_id()
            .filter(|output_id| {
                self.backend_for_output(*output_id)
                    .is_some_and(|backend| backend.drm_id == drm_id)
            })
            .or_else(|| self.output_ids_for_drm(drm_id).into_iter().next())
    }

    fn fallback_output_id(&self) -> Option<OutputId> {
        self.active_output
            .filter(|output_id| {
                self.outputs
                    .try_get(*output_id)
                    .is_some_and(|output| output.enabled)
            })
            .or_else(|| {
                self.output_layout()
                    .entries()
                    .first()
                    .map(|entry| entry.output_id)
            })
    }

    fn layout_output_id_for_surface(&self, surface: &WlSurface) -> Option<OutputId> {
        let preferred_output_id = surface.active_output_id().filter(|output_id| {
            self.outputs
                .try_get(*output_id)
                .is_some_and(|output| output.enabled)
        });
        surface
            .global_logical_bounds()
            .and_then(|bounds| {
                self.output_layout()
                    .primary_output_for_rect(bounds, preferred_output_id)
            })
            .or(preferred_output_id)
    }

    fn surface_object_id(&self, surface_id: WlSurfaceId) -> Option<ObjectId> {
        self.objects.iter().find_map(|(object_id, object)| {
            object
                .downcast_ref::<WlSurface>()
                .is_some_and(|surface| surface.surface_id() == surface_id)
                .then_some(object_id)
        })
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
        output.scale = output.configured_scale(&self.config);
        true
    }

    pub fn apply_output_scales_from_config(&mut self) -> Vec<OutputId> {
        let output_ids = self
            .outputs
            .iter()
            .map(|(output_id, _)| output_id)
            .collect::<Vec<_>>();
        let mut changed = Vec::new();
        for output_id in output_ids {
            let desired_scale = self.outputs[output_id].configured_scale(&self.config);
            let Some(output) = self.outputs.try_get_mut(output_id) else {
                continue;
            };
            if output.scale != desired_scale {
                output.scale = desired_scale;
                changed.push(output_id);
            }
        }
        changed
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
                output.scale = output.configured_scale(&self.config);
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
    use {
        super::*,
        crate::{
            clients::{Client, ClientDispatcher, RawRequest},
            protocol_generated::{WaylandId, WlSurfaceId},
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn make_output(
        backend: BackendOutputRef,
        drm_crtc_id: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Output {
        Output {
            backend,
            drm_crtc_id,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x,
            y,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: format!("output-{drm_crtc_id}"),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width, height, refresh_millihz: 60_000 },
            preferred_mode: None,
        }
    }

    #[derive(Debug)]
    struct TestObject {
        id: WaylandId,
    }

    impl Object for TestObject {
        fn id(&self) -> WaylandId {
            self.id
        }

        fn on(
            &mut self,
            _client: &mut Client,
            _state: &mut State,
            _request: RawRequest,
            _dispatcher: &mut ClientDispatcher,
        ) {
        }
    }

    #[derive(Debug)]
    struct OtherTestObject {
        id: WaylandId,
    }

    impl Object for OtherTestObject {
        fn id(&self) -> WaylandId {
            self.id
        }

        fn on(
            &mut self,
            _client: &mut Client,
            _state: &mut State,
            _request: RawRequest,
            _dispatcher: &mut ClientDispatcher,
        ) {
        }
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
    fn indexmap_stale_key_after_reuse_is_invalid() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let old_key = map.insert(10);

        assert_eq!(map.remove(old_key), Some(10));

        let new_key = map.insert(20);
        assert_eq!(new_key.index(), old_key.index());
        assert_ne!(new_key.generation(), old_key.generation());
        assert_eq!(map.try_get(old_key), None);
        assert!(!map.contains(old_key));
        assert_eq!(map.remove(old_key), None);
        assert_eq!(map.remove_or_defer(old_key), None);
        assert_eq!(*map.get(new_key), 20);
    }

    #[test]
    fn indexmap_deferred_remove_reuse_invalidates_previous_generation() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let old_key = map.insert(42);

        {
            let grabbed = map.grab(old_key);
            assert!(map.remove_or_defer(old_key).is_none());
            drop(grabbed);
        }

        assert_eq!(map.try_get(old_key), None);
        let new_key = map.insert(77);
        assert_eq!(new_key.index(), old_key.index());
        assert_ne!(new_key.generation(), old_key.generation());
        assert_eq!(map.try_get(old_key), None);
        assert_eq!(*map.get(new_key), 77);
    }

    #[test]
    fn indexmap_try_grab_object_roundtrip() {
        let mut map = Box::new(IndexMap::<ObjectId, Box<dyn Object>>::default());
        let key = map.insert(Box::new(TestObject { id: WaylandId::from(1u32) }));
        {
            let object = map.try_grab_object::<TestObject>(key).unwrap();
            assert_eq!(object.id(), WaylandId::from(1u32));
        }
        assert!(map.try_ref_object::<TestObject>(key).is_some());
    }

    #[test]
    fn indexmap_try_grab_object_wrong_type_restores_slot() {
        let mut map = Box::new(IndexMap::<ObjectId, Box<dyn Object>>::default());
        let key = map.insert(Box::new(TestObject { id: WaylandId::from(1u32) }));
        assert!(map.try_grab_object::<OtherTestObject>(key).is_none());
        assert!(map.try_ref_object::<TestObject>(key).is_some());
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

    #[test]
    fn output_layout_tracks_extents_and_lookup() {
        let mut outputs = IndexMap::<OutputId, Output>::default();
        let left_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(1usize)),
            drm_crtc_id: 1,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: -100,
            y: 20,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "left".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 50, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let right_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(2usize)),
            drm_crtc_id: 2,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "right".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 120, height: 80, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let layout = OutputLayout::from_outputs(&outputs);
        assert_eq!(layout.extents(), RectI::from_extents(-100, 0, 120, 80));
        assert_eq!(layout.output_at(-50, 40), Some(left_id));
        assert_eq!(layout.output_at(10, 10), Some(right_id));
        assert_eq!(layout.output_at(1000, 1000), None);
    }

    #[test]
    fn output_layout_primary_output_prefers_overlap_then_hint() {
        let mut outputs = IndexMap::<OutputId, Output>::default();
        let first_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(1usize)),
            drm_crtc_id: 1,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "first".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let second_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(2usize)),
            drm_crtc_id: 2,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 100,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "second".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let layout = OutputLayout::from_outputs(&outputs);
        let even_split = RectI::new(50, 0, 100, 100).unwrap();
        assert_eq!(layout.primary_output_for_rect(even_split, Some(second_id)), Some(second_id));
        let mostly_first = RectI::new(10, 0, 120, 100).unwrap();
        assert_eq!(layout.primary_output_for_rect(mostly_first, Some(second_id)), Some(first_id));
    }

    #[test]
    fn output_layout_clamps_points_to_nearest_output_region() {
        let mut outputs = IndexMap::<OutputId, Output>::default();
        outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(1usize)),
            drm_crtc_id: 1,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "left".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let right_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(2usize)),
            drm_crtc_id: 2,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 200,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "right".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let layout = OutputLayout::from_outputs(&outputs);
        assert_eq!(layout.nearest_point(150, 10), Some((right_id, (200, 10))));
        assert_eq!(layout.nearest_point(250, 50), Some((right_id, (250, 50))));
    }

    #[test]
    fn active_output_prefers_pointer_location_over_stored_fallback() {
        let mut state = make_state();
        let left = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            100,
            100,
        ));
        let right = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            2,
            100,
            0,
            100,
            100,
        ));

        assert_eq!(state.active_output, Some(left));
        assert_eq!(state.active_output_id(), Some(left));

        state.outputs.try_get_mut(left).unwrap().x = -200;
        state.outputs.try_get_mut(right).unwrap().x = 0;

        assert_eq!(state.active_output_id(), Some(right));
    }

    #[test]
    fn output_for_surface_falls_back_to_current_output_without_bounds() {
        let mut state = make_state();
        let output_id = state.insert_output(make_output(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            100,
            100,
        ));
        let surface = WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(2), 6);

        assert_eq!(state.output_id_for_surface(&surface), Some(output_id));
    }

    #[test]
    fn insert_output_creates_workspace_group_and_default_workspace() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));

        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let group = state.workspace_groups.try_get(group_id).unwrap();
        let workspace = state.workspaces.try_get(workspace_id).unwrap();

        assert_eq!(group.active_workspace, Some(workspace_id));
        assert_eq!(group.workspaces, vec![workspace_id]);
        assert_eq!(workspace.group, Some(group_id));
        assert_eq!(workspace.coordinates, vec![1]);
    }

    #[test]
    fn root_surface_visibility_tracks_active_workspace() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let first_workspace = state.active_workspace_for_output(output_id).unwrap();
        let second_workspace = state.create_workspace(Some(group_id), Some("2".to_string()));

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(10u32), ClientId::from_raw(1), 6));
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(object_id, second_workspace);

        assert!(!state.root_surface_is_visible(object_id));
        assert!(state.activate_workspace(second_workspace));
        assert!(state.root_surface_is_visible(object_id));
        assert!(!state.workspace_is_active(first_workspace));
    }

    #[test]
    fn assigning_root_surface_creates_persistent_managed_state() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(12u32), ClientId::from_raw(3), 6));
        state.register_root_surface(object_id);
        assert_eq!(state.root_surface_workspace(object_id), Some(workspace_id));

        let managed = state.managed_root_surface_state(object_id).unwrap();
        assert_eq!(managed.workspace_id, workspace_id);
        assert_eq!(managed.mode, ManagedWindowMode::Tiled);
        assert!(!managed.fullscreen);
        assert_eq!(managed.pending_floating_rect_restore, None);

        assert!(state.set_root_surface_floating(object_id, true));
        assert_eq!(
            state.managed_root_surface_state(object_id).unwrap().mode,
            ManagedWindowMode::Floating
        );
    }

    #[test]
    fn floating_mode_restores_last_floating_rect_when_reenabled() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(15u32), ClientId::from_raw(6), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 20, 30, 120, 90);
        }
        state.register_root_surface(object_id);

        assert!(state.set_root_surface_floating(object_id, true));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.set_root_position(250, 180);
        }

        assert!(state.set_root_surface_floating(object_id, false));
        assert_eq!(
            state
                .managed_root_surface_state(object_id)
                .unwrap()
                .last_floating_rect,
            Some(RectI { x: 250, y: 180, width: 120, height: 90 })
        );

        assert!(state.set_root_surface_floating(object_id, true));
        let surface = state
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .unwrap();
        assert_eq!(surface.root_position(), (250, 180));
        assert_eq!(
            state
                .managed_root_surface_state(object_id)
                .unwrap()
                .pending_floating_rect_restore,
            Some(RectI { x: 250, y: 180, width: 120, height: 90 })
        );
    }

    #[test]
    fn fullscreen_state_is_workspace_scoped_and_replaced_cleanly() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, first_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(13u32), ClientId::from_raw(4), 6));
        let (_, second_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(14u32), ClientId::from_raw(5), 6));
        state.register_root_surface(first_object_id);
        state.register_root_surface(second_object_id);
        assert_eq!(state.root_surface_workspace(first_object_id), Some(workspace_id));
        assert_eq!(state.root_surface_workspace(second_object_id), Some(workspace_id));

        assert!(state.set_root_surface_fullscreen(first_object_id, true));
        assert!(
            state
                .managed_root_surface_state(first_object_id)
                .unwrap()
                .fullscreen
        );
        assert!(
            !state
                .managed_root_surface_state(second_object_id)
                .unwrap()
                .fullscreen
        );

        assert!(state.set_root_surface_fullscreen(second_object_id, true));
        assert!(
            !state
                .managed_root_surface_state(first_object_id)
                .unwrap()
                .fullscreen
        );
        assert!(
            state
                .managed_root_surface_state(second_object_id)
                .unwrap()
                .fullscreen
        );
    }

    #[test]
    fn fullscreen_round_trip_restores_floating_rect() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(18u32), ClientId::from_raw(9), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 30, 40, 160, 120);
        }
        state.register_root_surface(object_id);
        assert!(state.set_root_surface_floating(object_id, true));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.set_root_position(320, 240);
        }

        assert!(state.set_root_surface_fullscreen(object_id, true));
        assert!(state.set_root_surface_fullscreen(object_id, false));

        let surface = state
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .unwrap();
        assert_eq!(surface.root_position(), (320, 240));
        assert_eq!(
            state
                .managed_root_surface_state(object_id)
                .unwrap()
                .pending_floating_rect_restore,
            Some(RectI { x: 320, y: 240, width: 160, height: 120 })
        );
    }

    #[test]
    fn moving_workspace_between_outputs_translates_assigned_root_surfaces() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left_output = state.insert_output(make_output(backend, 1, 0, 0, 100, 100));
        let right_output = state.insert_output(make_output(backend, 2, 200, 0, 100, 100));
        let left_workspace = state.active_workspace_for_output(left_output).unwrap();
        let right_group = state.workspace_group_for_output(right_output).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(11u32), ClientId::from_raw(2), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 10, 20, 50, 50);
        }
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(object_id, left_workspace);

        state.move_workspace_to_group_output(left_workspace, right_group);

        let surface = state
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .unwrap();
        assert_eq!(surface.root_position(), (210, 20));
        assert_eq!(state.workspace_output_id(left_workspace), Some(right_output));
    }

    #[test]
    fn moving_workspace_between_outputs_translates_stored_floating_rects() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left_output = state.insert_output(make_output(backend, 1, 0, 0, 100, 100));
        let right_output = state.insert_output(make_output(backend, 2, 200, 0, 100, 100));
        let left_workspace = state.active_workspace_for_output(left_output).unwrap();
        let right_group = state.workspace_group_for_output(right_output).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(16u32), ClientId::from_raw(7), 6));
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(object_id, left_workspace);
        state
            .managed_root_surfaces
            .get_mut(&object_id)
            .unwrap()
            .last_floating_rect = Some(RectI { x: 10, y: 20, width: 50, height: 50 });

        state.move_workspace_to_group_output(left_workspace, right_group);

        assert_eq!(
            state
                .managed_root_surface_state(object_id)
                .unwrap()
                .last_floating_rect,
            Some(RectI { x: 210, y: 20, width: 50, height: 50 })
        );
    }

    #[test]
    fn moving_workspace_between_outputs_translates_pending_floating_restore_rects() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left_output = state.insert_output(make_output(backend, 1, 0, 0, 100, 100));
        let right_output = state.insert_output(make_output(backend, 2, 200, 0, 100, 100));
        let left_workspace = state.active_workspace_for_output(left_output).unwrap();
        let right_group = state.workspace_group_for_output(right_output).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(17u32), ClientId::from_raw(8), 6));
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(object_id, left_workspace);
        state
            .managed_root_surfaces
            .get_mut(&object_id)
            .unwrap()
            .pending_floating_rect_restore = Some(RectI { x: 10, y: 20, width: 50, height: 50 });

        state.move_workspace_to_group_output(left_workspace, right_group);

        assert_eq!(
            state
                .managed_root_surface_state(object_id)
                .unwrap()
                .pending_floating_rect_restore,
            Some(RectI { x: 210, y: 20, width: 50, height: 50 })
        );
    }

    #[test]
    fn create_workspace_with_missing_group_stays_unassigned() {
        let mut state = make_state();

        let workspace_id = state.create_workspace(Some(WorkspaceGroupId::from(999usize)), None);
        let workspace = state.workspaces.try_get(workspace_id).unwrap();

        assert_eq!(workspace.group, None);
        assert!(workspace.coordinates.is_empty());
    }

    #[test]
    fn assign_workspace_to_missing_group_is_rejected() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let original_group = state.workspaces.try_get(workspace_id).unwrap().group;

        assert!(
            !state.assign_workspace_to_group(workspace_id, Some(WorkspaceGroupId::from(999usize)))
        );
        assert_eq!(state.workspaces.try_get(workspace_id).unwrap().group, original_group);
    }

    #[test]
    fn unassigning_workspace_clears_coordinates() {
        let mut state = make_state();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(make_output(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();

        assert_eq!(state.workspaces.try_get(workspace_id).unwrap().coordinates, vec![1]);
        assert!(state.assign_workspace_to_group(workspace_id, None));

        let workspace = state.workspaces.try_get(workspace_id).unwrap();
        assert_eq!(workspace.group, None);
        assert!(workspace.coordinates.is_empty());
    }
}
