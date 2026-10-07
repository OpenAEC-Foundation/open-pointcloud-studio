//! The parts of the frame of the window that the system keeps: the light
//! or dark hint of the frame on X11 and the colours of the frame on
//! Windows, which the shadow around a window without its frame takes, and
//! the resize from an edge of a window that has no frame of the system.

use iced::window::raw_window_handle::WindowHandle;

use crate::title_bar::Edge;

/// Give the title bar of the application's window the dark or light variant
/// and, where the system allows it, the given caption and text colours.
/// Returns false while the window does not exist yet.
#[cfg(target_os = "linux")]
pub fn apply(dark: bool, _caption: [u8; 3], _text: [u8; 3]) -> bool {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    let Ok((connection, screen)) = x11rb::connect(None) else {
        return false;
    };
    let root = connection.setup().roots[screen].root;
    let atom = |name: &[u8]| {
        connection
            .intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|reply| reply.atom)
    };
    let (Some(clients_atom), Some(pid_atom), Some(theme_atom), Some(utf8_atom)) = (
        atom(b"_NET_CLIENT_LIST"),
        atom(b"_NET_WM_PID"),
        atom(b"_GTK_THEME_VARIANT"),
        atom(b"UTF8_STRING"),
    ) else {
        return false;
    };
    let Ok(cookie) = connection.get_property(false, root, clients_atom, AtomEnum::WINDOW, 0, 4096)
    else {
        return false;
    };
    let Ok(reply) = cookie.reply() else {
        return false;
    };
    let Some(windows) = reply.value32() else {
        return false;
    };
    for window in windows {
        let Ok(cookie) = connection.get_property(false, window, pid_atom, AtomEnum::CARDINAL, 0, 1)
        else {
            continue;
        };
        let Some(pid) = cookie
            .reply()
            .ok()
            .and_then(|reply| reply.value32()?.next())
        else {
            continue;
        };
        if pid != std::process::id() {
            continue;
        }
        let value = if dark {
            b"dark".as_slice()
        } else {
            b"light".as_slice()
        };
        return connection
            .change_property8(PropMode::REPLACE, window, theme_atom, utf8_atom, value)
            .is_ok_and(|cookie| cookie.check().is_ok())
            && connection.flush().is_ok();
    }
    false
}

#[cfg(windows)]
pub fn apply(dark: bool, caption: [u8; 3], text: [u8; 3]) -> bool {
    use std::ffi::c_void;

    type Window = *mut c_void;
    const OWNER: u32 = 4;
    const DARK_MODE: u32 = 20;
    const BORDER_COLOR: u32 = 34;
    const CAPTION_COLOR: u32 = 35;
    const TEXT_COLOR: u32 = 36;
    // Redraw the frame without moving, sizing, reordering or activating.
    const FRAME_CHANGED: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020;

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(visit: unsafe extern "system" fn(Window, isize) -> i32, data: isize) -> i32;
        fn GetWindowThreadProcessId(window: Window, process: *mut u32) -> u32;
        fn IsWindowVisible(window: Window) -> i32;
        fn GetWindow(window: Window, relation: u32) -> Window;
        fn SetWindowPos(
            window: Window,
            after: Window,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
    }
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            window: Window,
            attribute: u32,
            value: *const c_void,
            size: u32,
        ) -> i32;
    }

    unsafe extern "system" fn visit(window: Window, data: isize) -> i32 {
        // SAFETY: `data` is the address of the vector that `apply` passes to
        // `EnumWindows`, which calls back on the same thread before it returns.
        let own = unsafe { &mut *(data as *mut Vec<Window>) };
        let mut process = 0;
        // SAFETY: `window` is a handle the system hands to this callback.
        let listed = unsafe {
            GetWindowThreadProcessId(window, &mut process);
            process == std::process::id()
                && IsWindowVisible(window) != 0
                && GetWindow(window, OWNER).is_null()
        };
        if listed {
            own.push(window);
        }
        1
    }

    let mut own: Vec<Window> = Vec::new();
    // SAFETY: the callback only uses the vector while `EnumWindows` runs.
    unsafe { EnumWindows(visit, &mut own as *mut Vec<Window> as isize) };
    let color = |[red, green, blue]: [u8; 3]| -> u32 {
        u32::from(red) | u32::from(green) << 8 | u32::from(blue) << 16
    };
    let mut applied = false;
    for window in own {
        let set = |attribute: u32, value: u32| {
            // SAFETY: the value is four bytes that live for the call, and the
            // window belongs to this process.
            unsafe { DwmSetWindowAttribute(window, attribute, (&raw const value).cast(), 4) >= 0 }
        };
        applied |= set(DARK_MODE, u32::from(dark));
        // Systems that cannot colour the caption keep the dark or light bar.
        set(CAPTION_COLOR, color(caption));
        set(BORDER_COLOR, color(caption));
        set(TEXT_COLOR, color(text));
        // SAFETY: only the frame of this process's own window is redrawn.
        unsafe { SetWindowPos(window, std::ptr::null_mut(), 0, 0, 0, 0, FRAME_CHANGED) };
    }
    applied
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn apply(_dark: bool, _caption: [u8; 3], _text: [u8; 3]) -> bool {
    true
}

/// Let the system resize the window from `edge`, as a press on its frame
/// would, while the button that was pressed is held: the window has no
/// frame of the system, and iced 0.13 does not ask the system for a resize.
/// On Windows the press is handed to the window as a press on that part of
/// its frame.
#[cfg(windows)]
pub fn begin_resize(handle: WindowHandle<'_>, edge: Edge) {
    use iced::window::raw_window_handle::RawWindowHandle;
    use std::ffi::c_void;

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    const NON_CLIENT_BUTTON_DOWN: u32 = 0x00A1;
    #[link(name = "user32")]
    extern "system" {
        fn ReleaseCapture() -> i32;
        fn GetCursorPos(point: *mut Point) -> i32;
        fn PostMessageW(window: *mut c_void, message: u32, wparam: usize, lparam: isize) -> i32;
    }

    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    // The parts of the frame, as the system numbers them.
    let part: usize = match edge {
        Edge::West => 10,
        Edge::East => 11,
        Edge::North => 12,
        Edge::NorthWest => 13,
        Edge::NorthEast => 14,
        Edge::South => 15,
        Edge::SouthWest => 16,
        Edge::SouthEast => 17,
    };
    let window = win32.hwnd.get() as *mut c_void;
    let mut point = Point { x: 0, y: 0 };
    // SAFETY: the point lives for the call, and the window belongs to this
    // process; this runs on the thread of the window, which holds the
    // capture of the pointer that is released.
    unsafe {
        if GetCursorPos(&mut point) == 0 {
            return;
        }
        ReleaseCapture();
        let position = (point.x as i16 as u16 as u32) | ((point.y as i16 as u16 as u32) << 16);
        PostMessageW(window, NON_CLIENT_BUTTON_DOWN, part, position as isize);
    }
}

/// On X11 the window manager is asked to resize the window from the place
/// of the pointer (`_NET_WM_MOVERESIZE`).
#[cfg(target_os = "linux")]
pub fn begin_resize(handle: WindowHandle<'_>, edge: Edge) {
    use iced::window::raw_window_handle::RawWindowHandle;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};

    let window: u32 = match handle.as_raw() {
        RawWindowHandle::Xlib(xlib) => xlib.window as u32,
        RawWindowHandle::Xcb(xcb) => xcb.window.get(),
        _ => return,
    };
    let Ok((connection, screen)) = x11rb::connect(None) else {
        return;
    };
    let root = connection.setup().roots[screen].root;
    let Some(atom) = connection
        .intern_atom(false, b"_NET_WM_MOVERESIZE")
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.atom)
    else {
        return;
    };
    let Some(pointer) = connection
        .query_pointer(root)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
    else {
        return;
    };
    // The directions of the specification, from the top left clockwise.
    let direction: u32 = match edge {
        Edge::NorthWest => 0,
        Edge::North => 1,
        Edge::NorthEast => 2,
        Edge::East => 3,
        Edge::SouthEast => 4,
        Edge::South => 5,
        Edge::SouthWest => 6,
        Edge::West => 7,
    };
    let _ = connection.ungrab_pointer(x11rb::CURRENT_TIME);
    let event = ClientMessageEvent::new(
        32,
        window,
        atom,
        [
            pointer.root_x as i32 as u32,
            pointer.root_y as i32 as u32,
            direction,
            1,
            1,
        ],
    );
    let _ = connection.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    );
    let _ = connection.flush();
}

/// Elsewhere the system frames the window and resizes it.
#[cfg(not(any(target_os = "linux", windows)))]
pub fn begin_resize(_handle: WindowHandle<'_>, _edge: Edge) {}
