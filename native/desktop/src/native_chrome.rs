//! Match the system title bar to the native OpenAEC palette on X11.
//! The window manager keeps its normal drag, resize and control behavior.

#[cfg(target_os = "linux")]
pub fn apply(dark: bool) -> bool {
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

#[cfg(not(target_os = "linux"))]
pub fn apply(_dark: bool) -> bool {
    true
}
