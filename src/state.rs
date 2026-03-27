use crate::{
    clients::ClientId,
    drm::Drm,
    egl::EGL,
    globals::{Global, GlobalName, Globals},
    input::Input,
    object::Object,
    protocol_generated::{
        WaylandId, WlDataOfferId, WlDataSourceId, WlSeatId, WlSurfaceId, ZwlrLayerSurfaceV1Id,
    },
    region::RectI,
    seat::SeatState,
    wayland_objects::wl_surface::WlSurface,
    xkb::{KeyboardState, Xkb},
};

use {
    rand::{rngs::OsRng, RngCore},
    std::{collections::BTreeMap, marker::PhantomData, ptr::NonNull, rc::Rc},
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

    /// Like [`Self::grab`], but returns [`None`] if the slot is empty, out of range.
    pub fn try_grab<Key: Into<K> + Copy>(self: &mut Box<Self>, key: Key) -> Option<Grab<K, V>>
    where
        K: Into<usize> + Copy + From<usize>,
    {
        let key: K = key.into();
        let index: usize = key.into();
        let entry = self.map.get_mut(index)?;
        match entry {
            ValueOrEmpty::Empty { .. } | ValueOrEmpty::GrabbedToRemove => return None,
            ValueOrEmpty::Grabbed => panic!("invalid state"),
            ValueOrEmpty::Value(_) => {}
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
    pub drm_crtc_id: u32,
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
    pub fn new(backend: BackendOutputRef, drm_crtc_id: u32, drm: &Drm) -> Self {
        let mut output = Self {
            backend,
            drm_crtc_id,
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

    pub fn drm_crtc_id(&self) -> u32 {
        self.drm_crtc_id
    }

    pub fn refresh_from_backend(&mut self, drm: &Drm) {
        let Some(in_use) = drm.output_connector_in_use(self.drm_crtc_id) else {
            self.enabled = false;
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
    pub backend_syncobj_globals: BTreeMap<DrmId, GlobalName>,
    pub globals: Globals,
    pub activation_tokens: ActivationTokenStore,
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
    }

    pub fn raise_root_surface(&mut self, object_id: ObjectId) -> bool {
        if !self.objects.contains(object_id) {
            return false;
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

    pub fn output_id_for_surface(&self, surface: &WlSurface) -> Option<OutputId> {
        self.layout_output_id_for_surface(surface)
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
    use {
        super::*,
        crate::{
            clients::{Client, ClientDispatcher, RawRequest},
            protocol_generated::WaylandId,
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
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
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
            x,
            y,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
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
            x: -100,
            y: 20,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
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
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
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
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
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
            x: 100,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
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
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
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
            x: 200,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: 1,
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
}
