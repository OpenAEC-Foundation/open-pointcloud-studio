use std::path::Path;

/// ReCap's supported interchange is E57; a companion file makes the handoff
/// explicit without labelling a different format as an Autodesk RCS file.
pub(crate) fn instructions(path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let destination = path.with_extension("ReCap.txt");
    let mut file = tempfile::NamedTempFile::new_in(destination.parent().unwrap_or(Path::new(".")))?;
    writeln!(file,"Open Pointcloud Studio → Autodesk ReCap\n\nInput: {}\n\n1. Open Autodesk ReCap Pro and create a new scan project.\n2. Import this E57. Keep its coordinates and units.\n3. Enable E57 image import to bring in spherical panoramas.\n4. Check scan stations, colours, scale and point count.\n5. Save the ReCap project as RCP with its RCS support files.\n\nE57 retains pinhole and cylindrical camera images too, but ReCap's documented panorama importer accepts spherical images with a pose and associated scan GUID.\n\nThis is an E57 handoff, not a direct RCP/RCS writer. Autodesk's supported direct writer is its Reality Solutions SDK, subject to SDK access and redistribution terms.\n\nOfficial import specification:\nhttps://help.autodesk.com/cloudhelp/ENU/Reality-Capture/files/Creating_Scan_Projects/Importing_Scans/E57_Panorama_Images/import_e57_support.html\n",path.file_name().unwrap_or_default().to_string_lossy())?;
    file.persist(destination).map_err(|e| e.error)?;
    Ok(())
}
