//! How the shared library is linked on each system, and the version details
//! of the Windows DLL. The build script runs on the build machine, so it asks
//! Cargo for the system it builds for instead of looking at its own.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => {
            // The name a program records when it links the library carries
            // the major version of the interface (docs/library.md).
            println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libops_pointcloud.so.1");
            // Never unloaded: its worker threads outlive any call.
            println!("cargo:rustc-cdylib-link-arg=-Wl,-z,nodelete");
            // Calls inside the library stay inside, even when the program
            // defines a function of the same name.
            println!("cargo:rustc-cdylib-link-arg=-Wl,-Bsymbolic-functions");
        }
        Ok("macos") => {
            println!(
                "cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/libops_pointcloud.dylib"
            );
        }
        Ok("windows") => version_resource(),
        _ => {}
    }
}

/// The version details that the file properties and the signature check of
/// Windows show.
fn version_resource() {
    const VFT_DLL: u64 = 2;
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let mut resource = winresource::WindowsResource::new();
    resource
        .set("ProductName", "Open Pointcloud Studio library")
        .set(
            "FileDescription",
            "C library of the point-cloud core of Open Pointcloud Studio",
        )
        .set("CompanyName", "OpenAEC Foundation")
        .set(
            "LegalCopyright",
            "OpenAEC Foundation and contributors. LGPL-3.0-or-later.",
        )
        .set("OriginalFilename", "ops_pointcloud.dll")
        .set("InternalName", "ops_pointcloud")
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set_version_info(winresource::VersionInfo::FILETYPE, VFT_DLL);
    if let Err(error) = resource.compile() {
        // The library works without them; only its file properties would be empty.
        println!("cargo:warning=Windows version details were not embedded: {error}");
    }
}
