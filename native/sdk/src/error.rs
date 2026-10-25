//! The status codes, the message of the last failed call on each thread, and
//! the wrapper every exported function runs its body in.

use std::cell::RefCell;
use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use pointcloud_core::{ops, LoadError};

use crate::{
    OpsStatus, OPS_ERROR_ARGUMENT, OPS_ERROR_CANCELLED, OPS_ERROR_DATA, OPS_ERROR_FORMAT,
    OPS_ERROR_INTERNAL, OPS_ERROR_IO, OPS_ERROR_LIMIT, OPS_OK,
};

thread_local! {
    /// Set by a failed call and cleared by the next call that returns a
    /// status. Borrowed only for a moment, never while a callback of the
    /// caller runs, so that a callback may ask for it or call the library.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Why a call failed: its status and the message `ops_last_error` returns.
#[derive(Debug)]
pub(crate) struct Failure {
    pub status: OpsStatus,
    pub message: String,
}

impl Failure {
    /// A null pointer, an unknown value, a `struct_size` that is too small or
    /// a path that is not UTF-8.
    pub fn argument(message: impl Into<String>) -> Self {
        Self {
            status: OPS_ERROR_ARGUMENT,
            message: message.into(),
        }
    }

    /// A callback of the caller returned something other than 0.
    pub fn stopped(callback: &str) -> Self {
        Self {
            status: OPS_ERROR_CANCELLED,
            message: format!("the {callback} callback asked to stop"),
        }
    }
}

/// The status of each kind of core error; the message is the error's own text.
impl From<LoadError> for Failure {
    fn from(error: LoadError) -> Self {
        let status = match &error {
            LoadError::Io(_) => OPS_ERROR_IO,
            LoadError::UnsupportedFormat(_) => OPS_ERROR_FORMAT,
            _ if ops::is_too_many_points(&error) => OPS_ERROR_LIMIT,
            LoadError::Las(_) | LoadError::E57(_) | LoadError::InvalidData(_) => OPS_ERROR_DATA,
            LoadError::Cancelled => OPS_ERROR_CANCELLED,
        };
        let message = match &error {
            // The refusal is not a fault of the source, which the prefix of
            // its text would suggest.
            LoadError::InvalidData(reason) if status == OPS_ERROR_LIMIT => reason.clone(),
            _ => error.to_string(),
        };
        Self { status, message }
    }
}

fn clear() {
    let _ = LAST_ERROR.try_with(|last| last.borrow_mut().take());
}

fn set(message: String) {
    // A message never holds a NUL; one from a file name would end it early.
    let message = CString::new(message.replace('\0', " ")).unwrap_or_default();
    let _ = LAST_ERROR.try_with(|last| *last.borrow_mut() = Some(message));
}

/// The message of the last failed call on this thread, or NULL. It stays
/// where it is until the next call of the library on this thread that
/// returns a status.
pub(crate) fn last() -> *const c_char {
    LAST_ERROR
        .try_with(|last| {
            last.borrow()
                .as_ref()
                .map_or(std::ptr::null(), |message| message.as_ptr())
        })
        .unwrap_or(std::ptr::null())
}

/// The text of a caught panic.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(text) = payload.downcast_ref::<&str>() {
        text
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text
    } else {
        "a panic without a message"
    }
}

/// Run the body of the exported function `name`: clear the message, catch a
/// panic, and turn the outcome into a status and a message. A success
/// leaves no message, also when a callback called the library and that
/// inner call failed. The first use of the library runs inside the catch as
/// well, so that nothing of the library unwinds into the caller.
pub(crate) fn run(name: &str, body: impl FnOnce() -> Result<(), Failure>) -> OpsStatus {
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        crate::first_use();
        clear();
        body()
    }));
    match outcome {
        Ok(Ok(())) => {
            clear();
            OPS_OK
        }
        Ok(Err(failure)) => {
            set(failure.message);
            failure.status
        }
        Err(payload) => {
            set(format!(
                "internal error in {name}: {}",
                panic_text(payload.as_ref())
            ));
            OPS_ERROR_INTERNAL
        }
    }
}

/// Run the body of an exported function that returns no status. A panic
/// cannot be reported there; it is caught so that it does not cross into
/// the caller, and the function returns `fallback`.
pub(crate) fn quietly<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(|| {
        crate::first_use();
        body()
    }))
    .unwrap_or(fallback)
}
