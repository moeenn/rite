//! Wayland compositors only hand focus to a window when asked with an
//! xdg-activation token. A file manager passes one to the `rite` it launches;
//! this redeems it for the already running window.

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::{self, XdgActivationV1};

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &XdgActivationV1, _: xdg_activation_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

/// Asks the compositor to focus and raise the window. Does nothing on other
/// platforms or when the compositor lacks xdg-activation.
pub fn activate(window: &dyn iced::window::Window, token: &str) {
    let (Ok(window), Ok(display)) = (window.window_handle(), window.display_handle()) else {
        return;
    };
    let (RawWindowHandle::Wayland(window), RawDisplayHandle::Wayland(display)) = (window.as_raw(), display.as_raw())
    else {
        return;
    };

    // SAFETY: both pointers come from the live window, and are only used
    // before this call returns.
    let (conn, surface) = unsafe {
        let backend = Backend::from_foreign_display(display.display.as_ptr().cast());
        let conn = Connection::from_backend(backend);
        let id = ObjectId::from_ptr(WlSurface::interface(), window.surface.as_ptr().cast());
        let surface = id.ok().and_then(|id| WlSurface::from_id(&conn, id).ok());
        (conn, surface)
    };
    let Some(surface) = surface else { return };

    let Ok((globals, mut queue)) = registry_queue_init::<State>(&conn) else {
        return;
    };
    let Ok(activation) = globals.bind::<XdgActivationV1, _, _>(&queue.handle(), 1..=1, ()) else {
        return;
    };
    activation.activate(token.to_string(), &surface);
    let _ = queue.roundtrip(&mut State);
    activation.destroy();
    let _ = conn.flush();
}
