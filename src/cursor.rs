use std::{collections::BTreeMap, env, fs};

use xcursor::{parser, CursorTheme};

use crate::{clients::ClientId, protocol_generated::WlSurfaceId, state::ObjectId};

const DEFAULT_ICON_NAMES: &[&str] = &["default", "left_ptr"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorImage {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub delay_msec: u32,
    pub pixels_rgba: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorSurface {
    pub surface_id: WlSurfaceId,
    pub object_id: ObjectId,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NamedCursor {
    owner_client_id: ClientId,
    name: String,
    animation: NamedCursorAnimation,
    started_at_msec: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NamedCursorAnimation {
    frames: Vec<CursorImage>,
    total_duration_msec: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CursorSource {
    Named(NamedCursor),
    Surface { owner_client_id: ClientId, cursor: CursorSurface },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorSourceRef<'a> {
    Named {
        owner_client_id: ClientId,
        name: &'a str,
        image: &'a CursorImage,
        next_frame_in_msec: Option<u32>,
    },
    Surface {
        owner_client_id: ClientId,
        cursor: &'a CursorSurface,
    },
}

#[derive(Debug)]
pub struct CursorState {
    source: Option<CursorSource>,
    theme: CursorTheme,
    theme_size: u32,
    cache: BTreeMap<String, Option<NamedCursorAnimation>>,
}

impl Default for CursorState {
    fn default() -> Self {
        let theme_name = env::var("XCURSOR_THEME").unwrap_or_else(|_| "default".to_string());
        let theme_size = env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|size| *size > 0)
            .unwrap_or(24);
        Self {
            source: None,
            theme: CursorTheme::load(&theme_name),
            theme_size,
            cache: BTreeMap::new(),
        }
    }
}

impl CursorState {
    pub fn source_at(&self, now_msec: u32) -> Option<CursorSourceRef<'_>> {
        match self.source.as_ref()? {
            CursorSource::Named(cursor) => {
                let (image, next_frame_in_msec) =
                    select_animation_frame(&cursor.animation, cursor.started_at_msec, now_msec)?;
                Some(CursorSourceRef::Named {
                    owner_client_id: cursor.owner_client_id,
                    name: &cursor.name,
                    image,
                    next_frame_in_msec,
                })
            }
            CursorSource::Surface { owner_client_id, cursor } => {
                Some(CursorSourceRef::Surface { owner_client_id: *owner_client_id, cursor })
            }
        }
    }

    pub fn source(&self) -> Option<CursorSourceRef<'_>> {
        self.source_at(monotonic_time_msec())
    }

    pub fn is_visible(&self) -> bool {
        self.source.is_some()
    }

    pub fn clear(&mut self) -> bool {
        if self.source.is_none() {
            return false;
        }
        self.source = None;
        true
    }

    pub fn clear_for_client(&mut self, client_id: ClientId) -> bool {
        if self.owner_client_id() != Some(client_id) {
            return false;
        }
        self.clear()
    }

    pub fn clear_for_focus(&mut self, focus_client_id: Option<ClientId>) -> bool {
        let Some(owner_client_id) = self.owner_client_id() else {
            return false;
        };
        if Some(owner_client_id) == focus_client_id {
            return false;
        }
        self.clear()
    }

    pub fn set_named_cursor(
        &mut self,
        owner_client_id: ClientId,
        icon_names: &[&'static str],
    ) -> bool {
        let fallback = if icon_names == DEFAULT_ICON_NAMES {
            None
        } else {
            self.resolve_icon(DEFAULT_ICON_NAMES)
        };
        let Some((resolved_name, animation)) = self.resolve_icon(icon_names).or(fallback) else {
            return self.clear();
        };

        let changed = !matches!(
            self.source.as_ref(),
            Some(CursorSource::Named(cursor))
                if cursor.owner_client_id == owner_client_id
                    && cursor.name == resolved_name
                    && cursor.animation == animation
        );
        if changed {
            self.source = Some(CursorSource::Named(NamedCursor {
                owner_client_id,
                name: resolved_name.to_string(),
                animation,
                started_at_msec: monotonic_time_msec(),
            }));
        }
        changed
    }

    pub fn set_surface_cursor(
        &mut self,
        owner_client_id: ClientId,
        surface_id: WlSurfaceId,
        object_id: ObjectId,
        hotspot_x: i32,
        hotspot_y: i32,
    ) -> bool {
        let next = CursorSource::Surface {
            owner_client_id,
            cursor: CursorSurface { surface_id, object_id, hotspot_x, hotspot_y },
        };
        let changed = self.source.as_ref() != Some(&next);
        if changed {
            self.source = Some(next);
        }
        changed
    }

    pub fn clear_surface(&mut self, object_id: ObjectId) -> bool {
        let Some(CursorSource::Surface { cursor, .. }) = self.source.as_ref() else {
            return false;
        };
        if cursor.object_id != object_id {
            return false;
        }
        self.clear()
    }

    pub fn apply_surface_offset(&mut self, object_id: ObjectId, dx: i32, dy: i32) -> bool {
        let Some(CursorSource::Surface { cursor, .. }) = self.source.as_mut() else {
            return false;
        };
        if cursor.object_id != object_id || (dx == 0 && dy == 0) {
            return false;
        }
        cursor.hotspot_x = clamp_i64_to_i32(i64::from(cursor.hotspot_x) - i64::from(dx));
        cursor.hotspot_y = clamp_i64_to_i32(i64::from(cursor.hotspot_y) - i64::from(dy));
        true
    }

    fn resolve_icon(
        &mut self,
        icon_names: &[&'static str],
    ) -> Option<(&'static str, NamedCursorAnimation)> {
        icon_names
            .iter()
            .find_map(|name| self.load_icon(name).map(|animation| (*name, animation)))
    }

    fn load_icon(&mut self, icon_name: &str) -> Option<NamedCursorAnimation> {
        if let Some(animation) = self.cache.get(icon_name) {
            return animation.clone();
        }

        let animation = self
            .theme
            .load_icon(icon_name)
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| parser::parse_xcursor(&bytes))
            .and_then(|images| self.pick_best_animation(icon_name, images));
        self.cache.insert(icon_name.to_string(), animation.clone());
        animation
    }

    fn pick_best_animation(
        &self,
        icon_name: &str,
        images: Vec<parser::Image>,
    ) -> Option<NamedCursorAnimation> {
        let target = images.iter().min_by_key(|image| {
            (image.size.abs_diff(self.theme_size), image.width.saturating_mul(image.height))
        })?;
        let (target_width, target_height) = (target.width, target.height);
        let mut frames = Vec::new();
        let mut total_duration_msec: u32 = 0;
        for image in images {
            if image.width != target_width || image.height != target_height {
                continue;
            }
            let frame = convert_cursor_image(icon_name, image)?;
            total_duration_msec = total_duration_msec.saturating_add(frame.delay_msec.max(1));
            frames.push(frame);
        }
        if frames.is_empty() {
            return None;
        }
        Some(NamedCursorAnimation { frames, total_duration_msec })
    }

    fn owner_client_id(&self) -> Option<ClientId> {
        match self.source.as_ref()? {
            CursorSource::Named(cursor) => Some(cursor.owner_client_id),
            CursorSource::Surface { owner_client_id, .. } => Some(*owner_client_id),
        }
    }
}

fn convert_cursor_image(icon_name: &str, image: parser::Image) -> Option<CursorImage> {
    let width = i32::try_from(image.width).ok()?;
    let height = i32::try_from(image.height).ok()?;
    let hotspot_x = i32::try_from(image.xhot).ok()?;
    let hotspot_y = i32::try_from(image.yhot).ok()?;
    Some(CursorImage {
        name: icon_name.to_string(),
        width,
        height,
        hotspot_x,
        hotspot_y,
        delay_msec: image.delay.max(1),
        pixels_rgba: image.pixels_rgba,
    })
}

fn select_animation_frame(
    animation: &NamedCursorAnimation,
    started_at_msec: u32,
    now_msec: u32,
) -> Option<(&CursorImage, Option<u32>)> {
    let first = animation.frames.first()?;
    if animation.frames.len() == 1 {
        return Some((first, None));
    }
    let total_duration_msec = animation.total_duration_msec.max(1);
    let elapsed_in_cycle = now_msec.wrapping_sub(started_at_msec) % total_duration_msec;
    let mut elapsed_before_frame = 0u32;
    for frame in &animation.frames {
        let frame_duration = frame.delay_msec.max(1);
        let frame_end = elapsed_before_frame.saturating_add(frame_duration);
        if elapsed_in_cycle < frame_end {
            return Some((frame, Some(frame_end.saturating_sub(elapsed_in_cycle))));
        }
        elapsed_before_frame = frame_end;
    }
    Some((animation.frames.last()?, Some(1)))
}

fn clamp_i64_to_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

fn monotonic_time_msec() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    let millis = (ts.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000);
    millis.min(u64::from(u32::MAX)) as u32
}

#[cfg(test)]
mod tests {
    use {
        super::{
            clamp_i64_to_i32, select_animation_frame, CursorImage, CursorSourceRef, CursorState,
            NamedCursorAnimation,
        },
        crate::{clients::ClientId, protocol_generated::WlSurfaceId, state::ObjectId},
        xcursor::parser,
    };

    #[test]
    fn surface_cursor_replaces_previous_source() {
        let mut cursor = CursorState::default();
        assert!(cursor.set_surface_cursor(
            ClientId::from_raw(1),
            WlSurfaceId::from(5),
            ObjectId::from(9usize),
            3,
            4,
        ));
        assert!(!cursor.set_surface_cursor(
            ClientId::from_raw(1),
            WlSurfaceId::from(5),
            ObjectId::from(9usize),
            3,
            4,
        ));
        match cursor.source().unwrap() {
            CursorSourceRef::Surface { owner_client_id, cursor } => {
                assert_eq!(owner_client_id, ClientId::from_raw(1));
                assert_eq!(cursor.surface_id, WlSurfaceId::from(5));
                assert_eq!(cursor.object_id, ObjectId::from(9usize));
            }
            other => panic!("unexpected cursor source: {other:?}"),
        }
    }

    #[test]
    fn clear_surface_only_clears_matching_surface() {
        let mut cursor = CursorState::default();
        cursor.set_surface_cursor(
            ClientId::from_raw(2),
            WlSurfaceId::from(7),
            ObjectId::from(11usize),
            0,
            0,
        );
        assert!(!cursor.clear_surface(ObjectId::from(12usize)));
        assert!(cursor.clear_surface(ObjectId::from(11usize)));
        assert!(cursor.source().is_none());
    }

    #[test]
    fn applying_surface_offset_updates_cursor_hotspot() {
        let mut cursor = CursorState::default();
        cursor.set_surface_cursor(
            ClientId::from_raw(3),
            WlSurfaceId::from(9),
            ObjectId::from(14usize),
            12,
            15,
        );

        assert!(cursor.apply_surface_offset(ObjectId::from(14usize), 3, -2));
        match cursor.source().unwrap() {
            CursorSourceRef::Surface { cursor, .. } => {
                assert_eq!(cursor.hotspot_x, 9);
                assert_eq!(cursor.hotspot_y, 17);
            }
            other => panic!("unexpected cursor source: {other:?}"),
        }
    }

    #[test]
    fn applying_surface_offset_clamps_hotspot_to_i32_range() {
        assert_eq!(clamp_i64_to_i32(i64::from(i32::MAX) + 20), i32::MAX);
        assert_eq!(clamp_i64_to_i32(i64::from(i32::MIN) - 20), i32::MIN);
    }

    #[test]
    fn pick_best_animation_keeps_all_frames_for_best_size_bucket() {
        let cursor = CursorState::default();
        let animation = cursor
            .pick_best_animation(
                "wait",
                vec![
                    parser::Image {
                        size: 24,
                        width: 24,
                        height: 24,
                        xhot: 1,
                        yhot: 2,
                        delay: 40,
                        pixels_rgba: vec![0; 24 * 24 * 4],
                        pixels_argb: vec![0; 24 * 24 * 4],
                    },
                    parser::Image {
                        size: 24,
                        width: 24,
                        height: 24,
                        xhot: 3,
                        yhot: 4,
                        delay: 60,
                        pixels_rgba: vec![1; 24 * 24 * 4],
                        pixels_argb: vec![1; 24 * 24 * 4],
                    },
                    parser::Image {
                        size: 48,
                        width: 48,
                        height: 48,
                        xhot: 5,
                        yhot: 6,
                        delay: 80,
                        pixels_rgba: vec![2; 48 * 48 * 4],
                        pixels_argb: vec![2; 48 * 48 * 4],
                    },
                ],
            )
            .unwrap();

        assert_eq!(animation.frames.len(), 2);
        assert_eq!(animation.frames[0].hotspot_x, 1);
        assert_eq!(animation.frames[1].hotspot_x, 3);
        assert_eq!(animation.total_duration_msec, 100);
    }

    #[test]
    fn select_animation_frame_respects_frame_delays() {
        let animation = NamedCursorAnimation {
            frames: vec![
                CursorImage {
                    name: "wait".to_string(),
                    width: 24,
                    height: 24,
                    hotspot_x: 1,
                    hotspot_y: 2,
                    delay_msec: 40,
                    pixels_rgba: vec![0; 4],
                },
                CursorImage {
                    name: "wait".to_string(),
                    width: 24,
                    height: 24,
                    hotspot_x: 3,
                    hotspot_y: 4,
                    delay_msec: 60,
                    pixels_rgba: vec![1; 4],
                },
            ],
            total_duration_msec: 100,
        };

        let (image0, next0) = select_animation_frame(&animation, 10, 10).unwrap();
        assert_eq!(image0.hotspot_x, 1);
        assert_eq!(next0, Some(40));

        let (image1, next1) = select_animation_frame(&animation, 10, 55).unwrap();
        assert_eq!(image1.hotspot_x, 3);
        assert_eq!(next1, Some(55));

        let (image_loop, _) = select_animation_frame(&animation, 10, 120).unwrap();
        assert_eq!(image_loop.hotspot_x, 1);
    }
}
