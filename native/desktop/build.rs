//! Gives the Windows executable its icon and version details, which the
//! taskbar, the file manager and the task switcher show.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../assets/icons/icon.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("../assets/icons/icon.ico")
        .set("ProductName", "Open Pointcloud Studio")
        .set("FileDescription", "Open Pointcloud Studio");
    if let Err(error) = resource.compile() {
        // The application works without them; only its icon would be generic.
        println!("cargo:warning=Windows icon and version details were not embedded: {error}");
    }
}
