//! The library is never unloaded. Its worker threads (the pool that reads and
//! writes in parallel) and its thread-local messages outlive any call, so
//! code of the library must stay mapped while the process lives.
//!
//! - Windows: the first call pins the module, so that `FreeLibrary` leaves it
//!   loaded.
//! - Linux: the library is linked with `-z nodelete` (build.rs).
//! - macOS: the loader does not unload an image with thread-local variables.

#[cfg(windows)]
mod windows {
    use std::ffi::c_void;

    const GET_MODULE_HANDLE_EX_FLAG_PIN: u32 = 0x0000_0001;
    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleExW(flags: u32, module_name: *const u16, module: *mut *mut c_void)
            -> i32;
    }

    /// Pin the module that holds this function. In a program that links the
    /// library statically that is the program itself, which changes nothing.
    pub fn pin_module() {
        let mut module = std::ptr::null_mut();
        let address = pin_module as *const () as *const u16;
        // SAFETY: with FROM_ADDRESS the name is an address inside a module,
        // and `module` is a valid place for the handle. A failure leaves the
        // library unpinned, which only matters to a host that unloads it.
        unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                address,
                &mut module,
            );
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::pin_module;

/// Elsewhere the linker and the loader see to it.
#[cfg(not(windows))]
pub(crate) fn pin_module() {}
