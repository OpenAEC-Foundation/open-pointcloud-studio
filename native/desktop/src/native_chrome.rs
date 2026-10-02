//! Match the system title bar to the native OpenAEC palette on X11 and
//! Windows. The window manager keeps its normal drag, resize and control
//! behavior.

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
