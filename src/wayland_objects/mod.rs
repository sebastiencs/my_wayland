#![allow(unused)]

pub mod wl_buffer;
pub mod wl_callback;
pub mod wl_compositor;
pub mod wl_data_device;
pub mod wl_data_device_manager;
pub mod wl_data_source;
pub mod wl_display;
pub mod wl_keyboard;
pub mod wl_output;
pub mod wl_pointer;
pub mod wl_registry;
pub mod wl_seat;
pub mod wl_shm;
pub mod wl_shm_pool;
pub mod wl_subcompositor;
pub mod wl_surface;
pub mod wp_cursor_shape_manager_v1;
pub mod wp_presentation;
pub mod xdg_activation_v1;
pub mod xdg_surface;
pub mod xdg_wm_base;
pub mod zxdg_output_manager_v1;

macro_rules! fake_obj {
    ($({$module:ident, $name:ident },)+) => ($(
        pub mod $module {
            use crate::protocol_generated::WaylandId;
            pub struct $name;
            impl $name {
                pub fn id(&self) -> WaylandId { todo!() }
            }
        }
    )+)
}

pub use xdg_surface::xdg_toplevel;

/// Declaring objects, until we properly implement them
/// So `protocol_generated.rs` doesn't contain non-declared types
fake_obj! {
    // { wl_shm_pool, WlShmPool },
    // { wl_buffer, WlBuffer },
    { wl_data_offer, WlDataOffer },
    { wl_shell, WlShell },
    { wl_shell_surface, WlShellSurface },
    // { wl_surface, WlSurface },
    // { wl_seat, WlSeat },
    // { wl_keyboard, WlKeyboard },
    { wl_touch, WlTouch },
    // { wl_output, WlOutput },
    { wl_region, WlRegion },
    // { wl_subcompositor, WlSubcompositor },
    { wl_subsurface, WlSubsurface },
    { wl_fixes, WlFixes },
    // { wp_presentation, WpPresentation },
    { wp_presentation_feedback, WpPresentationFeedback },
    { wp_viewporter, WpViewporter },
    { wp_viewport, WpViewport },
    // { xdg_wm_base, XdgWmBase },
    { xdg_positioner, XdgPositioner },
    // { xdg_surface, XdgSurface },
    // { xdg_toplevel, XdgToplevel },
    { xdg_popup, XdgPopup },
    { wp_content_type_manager_v1, WpContentTypeManagerV1 },
    { wp_content_type_v1, WpContentTypeV1 },
    { wp_drm_lease_device_v1, WpDrmLeaseDeviceV1 },
    { wp_drm_lease_connector_v1, WpDrmLeaseConnectorV1 },
    { wp_drm_lease_request_v1, WpDrmLeaseRequestV1 },
    { wp_drm_lease_v1, WpDrmLeaseV1 },
    { ext_idle_notifier_v1, ExtIdleNotifierV1 },
    { ext_idle_notification_v1, ExtIdleNotificationV1 },
    { ext_session_lock_manager_v1, ExtSessionLockManagerV1 },
    { ext_session_lock_v1, ExtSessionLockV1 },
    { wp_fractional_scale_manager_v1, WpFractionalScaleManagerV1 },
    { wp_fractional_scale_v1, WpFractionalScaleV1 },
    { wp_single_pixel_buffer_manager_v1, WpSinglePixelBufferManagerV1 },
    { wp_tearing_control_manager_v1, WpTearingControlManagerV1 },
    { wp_tearing_control_v1, WpTearingControlV1 },
    // { xdg_activation_v1, XdgActivationV1 },
    { xdg_activation_token_v1, XdgActivationTokenV1 },
    { xwayland_shell_v1, XwaylandShellV1 },
    { ext_session_lock_surface_v1, ExtSessionLockSurfaceV1 },
    { xwayland_surface_v1, XwaylandSurfaceV1 },
    { zxdg_output_v1, ZxdgOutputV1 },
    { wp_cursor_shape_device_v1, WpCursorShapeDeviceV1 },
    { zwp_tablet_manager_v2, ZwpTabletManagerV2 },
    { zwp_tablet_pad_v2, ZwpTabletPadV2 },
    { zwp_tablet_pad_group_v2, ZwpTabletPadGroupV2 },
    { zwp_tablet_pad_strip_v2, ZwpTabletPadStripV2 },
    { zwp_tablet_pad_ring_v2, ZwpTabletPadRingV2 },
    { zwp_tablet_v2, ZwpTabletV2 },
    { zwp_tablet_tool_v2, ZwpTabletToolV2 },
    { zwp_tablet_seat_v2, ZwpTabletSeatV2 },
}
