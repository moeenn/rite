//! Wayland compositors only hand focus to a window when asked with an
//! xdg-activation token. A file manager passes one to the `rite` it launches;
//! this redeems it for the already running window. A terminal passes none, so
//! then the window asks for its own, which the compositor grants only with
//! the serial of an input event the window received.

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::cell::RefCell;
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::{wl_registry::WlRegistry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{self, XdgActivationTokenV1};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::{self, XdgActivationV1};

#[derive(Default)]
struct State {
    /// The latest input serial sent to the window.
    serial: Option<u32>,
    token: Option<String>,
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: <WlRegistry as Proxy>::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &XdgActivationV1, _: xdg_activation_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationTokenV1, ()> for State {
    fn event(state: &mut Self, _: &XdgActivationTokenV1, event: xdg_activation_token_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.token = Some(token);
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event
            && caps.contains(wl_seat::Capability::Keyboard)
        {
            seat.get_keyboard(qh, ());
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Enter { serial, .. }
            | wl_keyboard::Event::Leave { serial, .. }
            | wl_keyboard::Event::Key { serial, .. }
            | wl_keyboard::Event::Modifiers { serial, .. } => state.serial = Some(serial),
            _ => {}
        }
    }
}

/// A second view of the window's wayland connection, with its own queue.
/// Keyboard events pile up in the queue until an activation reads them.
struct Tracker {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    seat: Option<WlSeat>,
    activation: XdgActivationV1,
    surface: WlSurface,
}

thread_local! {
    static TRACKER: RefCell<Option<Tracker>> = const { RefCell::new(None) };
}

/// Starts recording the window's input serials. Does nothing on other
/// platforms, when the compositor lacks xdg-activation, or when already done.
pub fn track(window: &dyn iced::window::Window) {
    TRACKER.with_borrow_mut(|tracker| {
        if tracker.is_none() {
            *tracker = connect(window);
        }
    });
}

fn connect(window: &dyn iced::window::Window) -> Option<Tracker> {
    let (window, display) = (window.window_handle().ok()?, window.display_handle().ok()?);
    let (RawWindowHandle::Wayland(window), RawDisplayHandle::Wayland(display)) = (window.as_raw(), display.as_raw())
    else {
        return None;
    };

    // SAFETY: both pointers come from the live window, which outlives the
    // tracker as both belong to the event loop thread.
    let (conn, surface) = unsafe {
        let backend = Backend::from_foreign_display(display.display.as_ptr().cast());
        let conn = Connection::from_backend(backend);
        let id = ObjectId::from_ptr(WlSurface::interface(), window.surface.as_ptr().cast()).ok()?;
        let surface = WlSurface::from_id(&conn, id).ok()?;
        (conn, surface)
    };

    let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
    let qh = queue.handle();
    let activation = globals.bind::<XdgActivationV1, _, _>(&qh, 1..=1, ()).ok()?;
    let seat = globals.bind::<WlSeat, _, _>(&qh, 1..=5, ()).ok();
    let mut state = State::default();
    // Receives the seat's capabilities, and with them the keyboard.
    queue.roundtrip(&mut state).ok()?;
    Some(Tracker { conn, queue, state, seat, activation, surface })
}

/// Drops the second view of the window's connection. Must run before the
/// window and its display are gone: left to the thread-local destructor, the
/// drop would reach into the display winit has already freed.
pub fn untrack() {
    TRACKER.with_borrow_mut(|tracker| {
        if let Some(t) = tracker.take() {
            t.activation.destroy();
            let _ = t.conn.flush();
        }
    });
}

/// Asks the compositor to focus and raise the window, with the launcher's
/// token or else one of the window's own. Returns false when it could not ask.
pub fn activate(window: &dyn iced::window::Window, token: Option<&str>) -> bool {
    track(window);
    TRACKER.with_borrow_mut(|tracker| {
        let Some(t) = tracker else { return false };
        if t.queue.dispatch_pending(&mut t.state).is_err() {
            return false;
        }
        let token = match token {
            Some(token) => token.to_string(),
            None => {
                let (Some(serial), Some(seat)) = (t.state.serial, &t.seat) else {
                    return false;
                };
                let request = t.activation.get_activation_token(&t.queue.handle(), ());
                // No surface: wlroots refuses a token for a surface without
                // keyboard or pointer focus, which ours lacks when hidden.
                request.set_serial(serial, seat);
                request.commit();
                t.state.token = None;
                while t.state.token.is_none() {
                    if t.queue.blocking_dispatch(&mut t.state).is_err() {
                        break;
                    }
                }
                request.destroy();
                let Some(token) = t.state.token.take() else { return false };
                token
            }
        };
        t.activation.activate(token, &t.surface);
        t.conn.flush().is_ok()
    })
}
