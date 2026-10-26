//! The frame of the application's window. On Windows and macOS the window has
//! no system title bar: the title strip of the application is the only bar at
//! the top and carries the window buttons, and on Windows the edges of the
//! window are handed back to the system for resizing. On Linux the window
//! keeps the frame of its window manager, whose title bar takes the colours
//! of the OpenAEC palette.

/// Whether the window is drawn without the system's frame and title bar.
/// Linux keeps the frame of its window manager: without it a window could not
/// be resized there.
pub const FRAMELESS: bool = cfg!(any(windows, target_os = "macos"));

/// The width, in pixels at 96 dpi, of the edge along which a window without a
/// system frame can still be resized.
#[cfg_attr(not(windows), allow(dead_code))]
pub const RESIZE_BORDER: i32 = 6;

/// The code with which the system names the side or corner of a window that a
/// point lies on, for a point on the edge of the window rectangle
/// (`[left, top, right, bottom]` in screen pixels), or `None` for a point
/// inside the edge or outside the window. These are the answers to
/// `WM_NCHITTEST` that let the system resize a window.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn edge_hit(x: i32, y: i32, rect: [i32; 4], border: i32) -> Option<isize> {
    const LEFT: isize = 10;
    const RIGHT: isize = 11;
    const TOP: isize = 12;
    const TOP_LEFT: isize = 13;
    const TOP_RIGHT: isize = 14;
    const BOTTOM: isize = 15;
    const BOTTOM_LEFT: isize = 16;
    const BOTTOM_RIGHT: isize = 17;
    let [left, top, right, bottom] = rect;
    if x < left || x >= right || y < top || y >= bottom {
        return None;
    }
    let near_left = x < left + border;
    let near_right = x >= right - border;
    let near_top = y < top + border;
    let near_bottom = y >= bottom - border;
    match (near_top, near_bottom, near_left, near_right) {
        (true, _, true, _) => Some(TOP_LEFT),
        (true, _, _, true) => Some(TOP_RIGHT),
        (_, true, true, _) => Some(BOTTOM_LEFT),
        (_, true, _, true) => Some(BOTTOM_RIGHT),
        (true, _, _, _) => Some(TOP),
        (_, true, _, _) => Some(BOTTOM),
        (_, _, true, _) => Some(LEFT),
        (_, _, _, true) => Some(RIGHT),
        _ => None,
    }
}

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
type Window = *mut std::ffi::c_void;

/// The visible top-level windows of this process, as the system lists them,
/// without tool windows: the event loop keeps a hidden one of its own.
#[cfg(windows)]
fn own_windows() -> Vec<Window> {
    const OWNER: u32 = 4;
    const EXTENDED_STYLE: i32 = -20;
    const TOOL_WINDOW: i32 = 0x0000_0080;

    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(visit: unsafe extern "system" fn(Window, isize) -> i32, data: isize) -> i32;
        fn GetWindowThreadProcessId(window: Window, process: *mut u32) -> u32;
        fn IsWindowVisible(window: Window) -> i32;
        fn GetWindow(window: Window, relation: u32) -> Window;
        fn GetWindowLongW(window: Window, index: i32) -> i32;
    }

    unsafe extern "system" fn visit(window: Window, data: isize) -> i32 {
        // SAFETY: `data` is the address of the vector that `own_windows`
        // passes to `EnumWindows`, which calls back on the same thread before
        // it returns.
        let own = unsafe { &mut *(data as *mut Vec<Window>) };
        let mut process = 0;
        // SAFETY: `window` is a handle the system hands to this callback.
        let listed = unsafe {
            GetWindowThreadProcessId(window, &mut process);
            process == std::process::id()
                && IsWindowVisible(window) != 0
                && GetWindow(window, OWNER).is_null()
                && GetWindowLongW(window, EXTENDED_STYLE) & TOOL_WINDOW == 0
        };
        if listed {
            own.push(window);
        }
        1
    }

    let mut own: Vec<Window> = Vec::new();
    // SAFETY: the callback only uses the vector while `EnumWindows` runs.
    unsafe { EnumWindows(visit, &mut own as *mut Vec<Window> as isize) };
    own
}

#[cfg(windows)]
pub fn apply(dark: bool, caption: [u8; 3], text: [u8; 3]) -> bool {
    use std::ffi::c_void;

    const DARK_MODE: u32 = 20;
    const BORDER_COLOR: u32 = 34;
    const CAPTION_COLOR: u32 = 35;
    const TEXT_COLOR: u32 = 36;
    // Redraw the frame without moving, sizing, reordering or activating.
    const FRAME_CHANGED: u32 = 0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0020;

    #[link(name = "user32")]
    extern "system" {
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

    let color = |[red, green, blue]: [u8; 3]| -> u32 {
        u32::from(red) | u32::from(green) << 8 | u32::from(blue) << 16
    };
    let mut applied = false;
    for window in own_windows() {
        let set = |attribute: u32, value: u32| {
            // SAFETY: the value is four bytes that live for the call, and the
            // window belongs to this process.
            unsafe { DwmSetWindowAttribute(window, attribute, (&raw const value).cast(), 4) >= 0 }
        };
        applied |= set(DARK_MODE, u32::from(dark));
        // The border of a window without a system frame takes the colour of
        // the title strip; systems that cannot colour it keep their own.
        set(CAPTION_COLOR, color(caption));
        set(BORDER_COLOR, color(caption));
        set(TEXT_COLOR, color(text));
        // SAFETY: only the frame of this process's own window is redrawn.
        unsafe { SetWindowPos(window, std::ptr::null_mut(), 0, 0, 0, 0, FRAME_CHANGED) };
    }
    applied
}

/// Let the system resize the window from its edges although it has no system
/// frame: the window answers `WM_NCHITTEST` near an edge with the code of that
/// edge, as a framed window does, so the system draws the resize cursors,
/// resizes and snaps it. A maximized window has no such edge. Returns false
/// while the window does not exist yet; installing it again replaces it.
#[cfg(windows)]
pub fn install_resize_border() -> bool {
    const HIT_TEST: u32 = 0x0084;
    // Any number that is the same for every call: it names this subclass.
    const SUBCLASS: usize = 0x4f50_5352;

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetWindowRect(window: Window, rect: *mut Rect) -> i32;
        fn IsZoomed(window: Window) -> i32;
        fn GetDpiForWindow(window: Window) -> u32;
    }
    type Procedure = unsafe extern "system" fn(Window, u32, usize, isize, usize, usize) -> isize;
    #[link(name = "comctl32")]
    extern "system" {
        fn SetWindowSubclass(window: Window, procedure: Procedure, id: usize, data: usize) -> i32;
        fn DefSubclassProc(window: Window, message: u32, wparam: usize, lparam: isize) -> isize;
    }

    unsafe extern "system" fn procedure(
        window: Window,
        message: u32,
        wparam: usize,
        lparam: isize,
        _id: usize,
        _data: usize,
    ) -> isize {
        if message == HIT_TEST {
            let mut rect = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            // SAFETY: the window is the one this procedure is installed on,
            // and `rect` lives for the call.
            let edge = unsafe { IsZoomed(window) == 0 && GetWindowRect(window, &mut rect) != 0 };
            if edge {
                // SAFETY: as above.
                let dpi = unsafe { GetDpiForWindow(window) }.max(96) as i32;
                // The point is in screen pixels, x in the low and y in the
                // high word, each signed.
                let x = i32::from(lparam as u16 as i16);
                let y = i32::from((lparam >> 16) as u16 as i16);
                let rect = [rect.left, rect.top, rect.right, rect.bottom];
                if let Some(code) = edge_hit(x, y, rect, RESIZE_BORDER * dpi / 96) {
                    return code;
                }
            }
        }
        // SAFETY: every other message goes on to the window's own procedure
        // unchanged.
        unsafe { DefSubclassProc(window, message, wparam, lparam) }
    }

    let own = own_windows();
    for &window in &own {
        // SAFETY: the window belongs to this process, and this runs on the
        // thread of its event loop, which owns it.
        unsafe { SetWindowSubclass(window, procedure, SUBCLASS, 0) };
    }
    !own.is_empty()
}

#[cfg(not(windows))]
pub fn install_resize_border() -> bool {
    true
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn apply(_dark: bool, _caption: [u8; 3], _text: [u8; 3]) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::edge_hit;

    const WINDOW: [i32; 4] = [100, 50, 900, 650];

    #[test]
    fn the_sides_and_corners_of_a_window_are_named_as_the_system_names_them() {
        assert_eq!(edge_hit(101, 300, WINDOW, 6), Some(10));
        assert_eq!(edge_hit(898, 300, WINDOW, 6), Some(11));
        assert_eq!(edge_hit(500, 52, WINDOW, 6), Some(12));
        assert_eq!(edge_hit(102, 51, WINDOW, 6), Some(13));
        assert_eq!(edge_hit(899, 50, WINDOW, 6), Some(14));
        assert_eq!(edge_hit(500, 649, WINDOW, 6), Some(15));
        assert_eq!(edge_hit(100, 645, WINDOW, 6), Some(16));
        assert_eq!(edge_hit(895, 645, WINDOW, 6), Some(17));
    }

    #[test]
    fn points_inside_the_edge_or_outside_the_window_are_left_to_the_window() {
        assert_eq!(edge_hit(106, 300, WINDOW, 6), None);
        assert_eq!(edge_hit(500, 56, WINDOW, 6), None);
        assert_eq!(edge_hit(500, 300, WINDOW, 6), None);
        assert_eq!(edge_hit(99, 300, WINDOW, 6), None);
        assert_eq!(edge_hit(900, 300, WINDOW, 6), None);
        assert_eq!(edge_hit(500, 650, WINDOW, 6), None);
    }
}
