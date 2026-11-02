# Autodesk ReCap interoperability

Open Pointcloud Studio does not write `.rcp` or `.rcs` files. An RCP is a
project referencing indexed RCS scans; writing only an RCP would not create a
usable ReCap project. Autodesk's [Reality Solutions SDK][sdk] can write both
formats, but Autodesk distributes it under its Developer Network agreement.
No public, redistributable Rust writer or format specification has been
established for this project. A native RCS writer should therefore not be
claimed or built by guessing at its binary layout.

## Transfer a scan into ReCap

1. In Open Pointcloud Studio, export the scan as **E57**. For an unedited E57,
   the full export copies the source byte-for-byte. A filtered E57 export
   retains the separate scans, their poses and `Images2D` entries, including
   station and route photos. Save to a new filename.
2. Create a scan project in ReCap Pro and import the E57. Check the scanner
   positions, scale, orientation, colour and point count. ReCap's [supported
   formats][formats] list E57 as an input.
3. For panoramas, enable ReCap's E57 image import and check the RealView
   images. Autodesk's [E57 import specification][e57-import] says that ReCap
   imports **spherical** `Images2D` with a matching `associatedData3DGuid`
   and pose. Pinhole and cylindrical images may remain in the E57 file but
   should not be expected to appear as ReCap panoramas.
4. If scan imagery is not needed, LAS or LAZ is another point-data route.
   These formats do not carry E57 station photos.

The E57 export keeps image bytes and per-image metadata; it does not turn a
pinhole or cylindrical image into a spherical panorama. A filtered export can
contain fewer points than the original image depicts. Inspect the result in
ReCap before passing it on. An independent E57 reader should also verify the
point count, scan GUID associations and image bytes for a real static-scanner
and a real SLAM file.

## Direct RCP/RCS export decision

The [Reality Solutions SDK][sdk] is the supported Autodesk route to an
RCP/RCS writer. Before considering an optional integration, establish SDK
access, redistribution and license compatibility with the LGPL application,
current platform support, and how scanner photos and registration metadata
would map. Until those are verified, **E57 is the supported interchange
format**. Autodesk also describes an RCP as a project referencing its RCS
files in its [file-format guide][project].

[sdk]: https://help.autodesk.com/cloudhelp/ENU/Reality-Capture/files/Product_Overview/recap_sdk.html
[formats]: https://help.autodesk.com/view/RECAP/ENU/?contextId=supported_file_formats
[e57-import]: https://help.autodesk.com/cloudhelp/ENU/Reality-Capture/files/Creating_Scan_Projects/Importing_Scans/E57_Panorama_Images/import_e57_support.html
[project]: https://help.autodesk.com/cloudhelp/ENU/Reality-Capture/files/Getting_Started/Supported_File_Formats/scan_photogrammetry.html
