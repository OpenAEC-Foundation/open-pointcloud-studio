//! Files that macOS hands to the application: a scan opened from the Finder,
//! dropped on the Dock icon or sent with `open -a`. They do not arrive on the
//! command line but as a message to the delegate of the application.
//!
//! The windowing library registers that delegate and looks it up by its class
//! on every turn of its event loop, so the delegate is not replaced here: its
//! class gets the one method it lacks.

use std::path::PathBuf;
use std::sync::OnceLock;

use objc2::ffi;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, NSDictionary, NSString, NSUserDefaults, NSURL};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// Where the files that were handed over go.
static OPENED: OnceLock<UnboundedSender<PathBuf>> = OnceLock::new();

/// The path of a file or folder the system asks to open; a link to anything
/// else is left out.
fn file_path(url: &NSURL) -> Option<PathBuf> {
    if url.isFileURL() {
        url.to_file_path()
    } else {
        None
    }
}

/// The signature of `application:openURLs:` as the runtime calls it: the
/// delegate, the selector, the application and the links to open.
type OpenUrls = extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, &NSArray<NSURL>);

/// `application:openURLs:` of the application delegate.
extern "C-unwind" fn application_open_urls(
    _delegate: &AnyObject,
    _selector: Sel,
    _application: &AnyObject,
    urls: &NSArray<NSURL>,
) {
    let Some(opened) = OPENED.get() else {
        return;
    };
    for index in 0..urls.count() {
        if let Some(path) = file_path(&urls.objectAtIndex(index)) {
            // The window may be closing; then nobody waits for the file.
            let _ = opened.send(path);
        }
    }
}

/// Make instances of `class` answer `application:openURLs:`. Returns whether
/// the method was added; a class that answers it already is left as it is.
fn answer_open_urls(class: &AnyClass) -> bool {
    let selector = sel!(application:openURLs:);
    if class.responds_to(selector) {
        return false;
    }
    // SAFETY: the runtime keeps a method as a pointer without a signature and
    // calls it with the signature of its selector, which is `OpenUrls`.
    let implementation = unsafe { std::mem::transmute::<OpenUrls, Imp>(application_open_urls) };
    // SAFETY: the class is registered, the selector takes the two objects the
    // function takes after its receiver and selector, and the type string
    // says the same: no result, the receiver, the selector and two objects.
    unsafe {
        ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            implementation,
            c"v@:@@".as_ptr(),
        )
    }
    .as_bool()
}

/// Leave the arguments of the command line to `main`. Without this the
/// application framework also reports each file among them as a file to open.
fn keep_arguments_out_of_open_requests() {
    let name = NSString::from_str("NSTreatUnknownArgumentsAsOpen");
    let no = NSString::from_str("NO");
    let no: &AnyObject = &no;
    let defaults = NSDictionary::<NSString, AnyObject>::from_slices(&[&*name], &[no]);
    // SAFETY: the dictionary maps a string to a string, which is a value the
    // user defaults accept.
    unsafe { NSUserDefaults::standardUserDefaults().registerDefaults(&defaults) };
}

/// Start receiving the files the system hands to the application. Call it on
/// the main thread once the event loop exists and before it runs, because the
/// first files arrive while the application finishes launching.
///
/// Returns `None` when nothing can be received: off the main thread, without
/// an application delegate, or when the delegate handles such files itself.
pub fn install() -> Option<UnboundedReceiver<PathBuf>> {
    let main_thread = MainThreadMarker::new()?;
    let application = NSApplication::sharedApplication(main_thread);
    let delegate = application.delegate()?;
    let object = AsRef::<AnyObject>::as_ref(&*delegate);
    if OPENED.get().is_some() || !answer_open_urls(object.class()) {
        return None;
    }
    let (sender, receiver) = unbounded_channel();
    OPENED.set(sender).ok()?;
    keep_arguments_out_of_open_requests();
    // The application may have noted what its delegate answers when the
    // delegate was set, so it is set once more now that it answers more.
    application.setDelegate(Some(&*delegate));
    Some(receiver)
}

#[cfg(test)]
mod tests {
    use objc2::rc::Retained;
    use objc2::runtime::ClassBuilder;
    use objc2::{class, msg_send};

    use super::*;

    #[test]
    fn file_links_become_paths_and_other_links_are_left_out() {
        // Characters that the file system stores as they are written.
        let path = "/tmp/Hal 1 – ø 点.e57";
        let file = NSURL::from_file_path(path).unwrap();
        assert_eq!(file_path(&file), Some(PathBuf::from(path)));
        let folder = NSURL::from_directory_path("/tmp/scan project").unwrap();
        assert_eq!(
            file_path(&folder),
            Some(PathBuf::from("/tmp/scan project/"))
        );
        let web =
            NSURL::URLWithString(&NSString::from_str("https://example.org/scan.e57")).unwrap();
        assert_eq!(file_path(&web), None);
    }

    #[test]
    fn a_delegate_class_learns_to_pass_on_the_files_it_is_asked_to_open() {
        let class = ClassBuilder::new(c"OpenedFilesTestDelegate", class!(NSObject))
            .unwrap()
            .register();
        assert!(answer_open_urls(class));
        assert!(class.responds_to(sel!(application:openURLs:)));
        assert!(!answer_open_urls(class), "the method is added once");

        let (sender, mut receiver) = unbounded_channel();
        OPENED.set(sender).unwrap();
        // SAFETY: `new` creates an instance of the class registered above.
        let delegate: Retained<AnyObject> = unsafe { msg_send![class, new] };
        let urls = NSArray::from_retained_slice(&[
            NSURL::from_file_path("/tmp/first scan.e57").unwrap(),
            NSURL::URLWithString(&NSString::from_str("https://example.org/")).unwrap(),
            NSURL::from_file_path("/tmp/second.laz").unwrap(),
        ]);
        // SAFETY: the method added above takes the application, for which any
        // object stands in here, and an array of links.
        let _: () = unsafe { msg_send![&*delegate, application: &*delegate, openURLs: &*urls] };
        assert_eq!(
            receiver.try_recv().ok(),
            Some(PathBuf::from("/tmp/first scan.e57"))
        );
        assert_eq!(
            receiver.try_recv().ok(),
            Some(PathBuf::from("/tmp/second.laz"))
        );
        assert!(receiver.try_recv().is_err());
    }
}
