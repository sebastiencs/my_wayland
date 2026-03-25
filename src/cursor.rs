use std::{collections::BTreeMap, env, fs};

use xcursor::{parser, CursorTheme};

use crate::clients::ClientId;

const DEFAULT_ICON_NAMES: &[&str] = &["default", "left_ptr"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorImage {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub pixels_rgba: Vec<u8>,
}

#[derive(Debug)]
pub struct CursorState {
    owner_client_id: Option<ClientId>,
    active_name: Option<String>,
    active_image: Option<CursorImage>,
    theme: CursorTheme,
    theme_size: u32,
    cache: BTreeMap<String, Option<CursorImage>>,
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
            owner_client_id: None,
            active_name: None,
            active_image: None,
            theme: CursorTheme::load(&theme_name),
            theme_size,
            cache: BTreeMap::new(),
        }
    }
}

impl CursorState {
    pub fn image(&self) -> Option<&CursorImage> {
        self.active_image.as_ref()
    }

    pub fn is_visible(&self) -> bool {
        self.active_image.is_some()
    }

    pub fn clear(&mut self) -> bool {
        if self.owner_client_id.is_none()
            && self.active_name.is_none()
            && self.active_image.is_none()
        {
            return false;
        }
        self.owner_client_id = None;
        self.active_name = None;
        self.active_image = None;
        true
    }

    pub fn clear_for_client(&mut self, client_id: ClientId) -> bool {
        if self.owner_client_id != Some(client_id) {
            return false;
        }
        self.clear()
    }

    pub fn clear_for_focus(&mut self, focus_client_id: Option<ClientId>) -> bool {
        if self.owner_client_id.is_none() || self.owner_client_id == focus_client_id {
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
        let Some((resolved_name, image)) = self.resolve_icon(icon_names).or(fallback) else {
            return self.clear();
        };

        let changed = self.owner_client_id != Some(owner_client_id)
            || self.active_name.as_deref() != Some(resolved_name)
            || self.active_image.as_ref() != Some(&image);
        if changed {
            self.owner_client_id = Some(owner_client_id);
            self.active_name = Some(resolved_name.to_string());
            self.active_image = Some(image);
        }
        changed
    }

    fn resolve_icon(&mut self, icon_names: &[&'static str]) -> Option<(&'static str, CursorImage)> {
        icon_names
            .iter()
            .find_map(|name| self.load_icon(name).map(|image| (*name, image)))
    }

    fn load_icon(&mut self, icon_name: &str) -> Option<CursorImage> {
        if let Some(image) = self.cache.get(icon_name) {
            return image.clone();
        }

        let image = self
            .theme
            .load_icon(icon_name)
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| parser::parse_xcursor(&bytes))
            .and_then(|images| self.pick_best_image(icon_name, images));
        self.cache.insert(icon_name.to_string(), image.clone());
        image
    }

    fn pick_best_image(&self, icon_name: &str, images: Vec<parser::Image>) -> Option<CursorImage> {
        let image = images.into_iter().min_by_key(|image| {
            (
                image.size.abs_diff(self.theme_size),
                image.delay,
                image.width.saturating_mul(image.height),
            )
        })?;
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
            pixels_rgba: image.pixels_rgba,
        })
    }
}
