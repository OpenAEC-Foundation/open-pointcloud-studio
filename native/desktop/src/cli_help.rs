//! The command line of the program: one list of every mode, which `--help`
//! prints and which a test holds against what `main` handles.

use std::ffi::OsStr;

use crate::i18n::{key, tr};

/// The name of the executable in the usage lines and the version line.
pub const PROGRAM: &str = "open-pointcloud-studio";

/// One way to start the program.
pub struct Mode {
    /// The first argument that chooses the mode; empty for opening the window
    /// with the files given.
    pub flag: &'static str,
    /// A one-letter spelling of the flag.
    pub short: Option<&'static str>,
    /// What follows the flag, as the usage lines of `main` spell it.
    pub arguments: &'static str,
    pub description: &'static str,
}

/// Every mode `main` handles. A new mode is one entry here and its branch in
/// `main`.
pub const MODES: &[Mode] = &[
    Mode {
        flag: "",
        short: None,
        arguments: "[INPUT ...]",
        description: key("Opens the window with the scans, scan folders and scan project files given."),
    },
    Mode {
        flag: "--api-port",
        short: None,
        arguments: "PORT [INPUT ...]",
        description: key("Opens the window with its local command API on a fixed port."),
    },
    Mode {
        flag: "--mcp",
        short: None,
        arguments: "",
        description: key(
            "Serves the commands as MCP tools on standard input and output, without a window of its own.",
        ),
    },
    Mode {
        flag: "--list-scans",
        short: None,
        arguments: "PATH [PATH ...]",
        description: key("Prints the scan files found in files, folders and scan project files."),
    },
    Mode {
        flag: "--index",
        short: None,
        arguments: "INPUT",
        description: key("Builds the octree index of a scan and keeps it in the cache."),
    },
    Mode {
        flag: "--scans",
        short: None,
        arguments: "INPUT",
        description: key("Prints the scanner positions stored in a scan."),
    },
    Mode {
        flag: "--photos",
        short: None,
        arguments: "INPUT OUTPUT_DIRECTORY",
        description: key("Saves the station photos of a scan as image files."),
    },
    Mode {
        flag: "--export",
        short: None,
        arguments: "INPUT OUTPUT",
        description: key("Converts a scan; the extension of OUTPUT chooses the format."),
    },
    Mode {
        flag: "--section",
        short: None,
        arguments: "INPUT XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX OUTPUT [--rotation DEGREES]",
        description: key("Exports the points of a scan that lie inside a box."),
    },
    Mode {
        flag: "--drawing",
        short: None,
        arguments: "INPUT XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX OUTPUT.dxf|.dwg [--view plan|front|back|left|right] [--rotation DEGREES] [--thickness METRES] [--units mm|m] [--fill on|off]",
        description: key(
            "Draws the slab behind one face of a box in a scan as a 2D drawing in DXF or DWG.",
        ),
    },
    Mode {
        flag: "--merge",
        short: None,
        arguments: "OUTPUT.laz INPUT1.las INPUT2.laz [...]",
        description: key("Merges LAS and LAZ scans into one file."),
    },
    Mode {
        flag: "--mesh",
        short: None,
        arguments: "INPUT OUTPUT.obj",
        description: key("Writes a terrain mesh of a scan."),
    },
    Mode {
        flag: "--surface",
        short: None,
        arguments: "INPUT OUTPUT.obj [--max-vertices N] [--neighbors N] [--edge-factor N] [--sample-percent P] [--mesh-size SIZE]",
        description: key("Writes a 3D surface mesh of a scan."),
    },
    Mode {
        flag: "--closed-mesh",
        short: None,
        arguments: "INPUT OUTPUT.obj|.ply|.stl|.dxf|.dwg|.ifc [--box XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX] [--rotation DEGREES] [--voxel METRES] [--max-hole METRES] [--simplify MILLIMETRES] [--sample-percent P] [--sides automatic|centre|upward]",
        description: key(
            "Writes a closed mesh of a scan, or of a box in it, as OBJ, PLY, STL, DXF, DWG or IFC.",
        ),
    },
    Mode {
        flag: "--faces",
        short: None,
        arguments: "INPUT OUTPUT.json|.obj|.dxf|.dwg|.ifc [--box XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX] [--rotation DEGREES] [--distance METRES] [--angle DEGREES] [--min-area SQUARE_METRES] [--cylinders on|off]",
        description: key(
            "Detects the flat faces and the cylinders of a scan, or of a box in it, and writes them as JSON, OBJ, DXF, DWG or IFC.",
        ),
    },
    Mode {
        flag: "--mesh-export",
        short: None,
        arguments: "INPUT OUTPUT",
        description: key(
            "Writes the faces of a mesh file as OBJ, PLY, STL, DXF, DWG or IFC; the extension of OUTPUT chooses the format.",
        ),
    },
    Mode {
        flag: "--bag3d",
        short: None,
        arguments: "XMIN,YMIN,XMAX,YMAX 1.2|1.3|2.2 OUTPUT.obj",
        description: key("Downloads the 3D BAG buildings inside an RD New box as OBJ."),
    },
    Mode {
        flag: "--version",
        short: Some("-V"),
        arguments: "",
        description: key("Prints the version."),
    },
    Mode {
        flag: "--help",
        short: Some("-h"),
        arguments: "",
        description: key("Prints this text."),
    },
];

/// The one line `--version` prints: the name of the executable and the
/// version, which the packages compare with the version they were built for.
pub fn version_line() -> String {
    format!("{PROGRAM} {}", env!("CARGO_PKG_VERSION"))
}

/// How a mode is started, as the usage line of its branch in `main` says it.
fn invocation(mode: &Mode) -> String {
    [PROGRAM, mode.flag, mode.arguments]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The text `--help` prints: every mode with what it does. It has no line
/// end of its own; printing it as a line adds one.
pub fn usage() -> String {
    let mut text = format!("{}\n\n{}", crate::app_title(), tr("Usage:"));
    for mode in MODES {
        text.push_str(&format!("\n  {}", invocation(mode)));
        if let Some(short) = mode.short {
            text.push_str(&format!(" | {short}"));
        }
        text.push_str(&format!("\n      {}", tr(mode.description)));
    }
    text
}

/// What the program prints, instead of doing anything else, when its first
/// argument asks for the version or the help text.
pub fn answer(first: Option<&OsStr>) -> Option<String> {
    let first = first?.to_str()?;
    let mode = MODES
        .iter()
        .find(|mode| !mode.flag.is_empty() && (mode.flag == first || mode.short == Some(first)))?;
    match mode.flag {
        "--version" => Some(version_line()),
        "--help" => Some(usage()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::i18n::source_scan::{literal_arguments, literal_at};

    /// The text of `fn main`, where the modes are told apart.
    fn dispatcher() -> &'static str {
        let source = include_str!("main.rs");
        let start = source
            .find("fn main() -> iced::Result {")
            .expect("main.rs has the entry point");
        let body = &source[start..];
        let end = body
            .find("\n}")
            .expect("the entry point ends at a closing brace in the first column");
        &body[..end]
    }

    #[test]
    fn version_line_is_the_executable_name_and_the_version() {
        assert_eq!(
            version_line(),
            format!("open-pointcloud-studio {}", env!("CARGO_PKG_VERSION"))
        );
        assert!(!version_line().contains('\n'));
        for flag in ["--version", "-V"] {
            assert_eq!(answer(Some(OsStr::new(flag))), Some(version_line()));
        }
    }

    #[test]
    fn help_names_every_mode_with_its_arguments() {
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let help = usage();
        for flag in ["--help", "-h"] {
            assert_eq!(answer(Some(OsStr::new(flag))).as_deref(), Some(&*help));
        }
        assert!(help.starts_with(&crate::app_title()));
        for mode in MODES {
            assert!(help.contains(&invocation(mode)), "{}", mode.flag);
            assert!(help.contains(mode.description), "{}", mode.flag);
            assert!(mode.description.ends_with('.'), "{}", mode.flag);
        }
        assert!(help.contains("--version | -V"));
        // A file to open, or nothing at all, is not answered with a text.
        assert_eq!(answer(None), None);
        assert_eq!(answer(Some(OsStr::new("scan.laz"))), None);
        assert_eq!(answer(Some(OsStr::new(""))), None);
        assert_eq!(answer(Some(OsStr::new("--export"))), None);
    }

    #[test]
    fn help_is_translated() {
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::Table(0));
        let help = usage();
        assert!(help.contains("Gebruik:"));
        assert!(help.contains("--export INPUT OUTPUT"));
        // The version line is compared by scripts and stays as it is.
        assert_eq!(
            version_line(),
            format!("open-pointcloud-studio {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn main_handles_every_listed_mode_and_lists_every_mode_it_handles() {
        let flags: Vec<&str> = MODES.iter().map(|mode| mode.flag).collect();
        let unique: BTreeSet<&str> = flags.iter().copied().collect();
        assert_eq!(unique.len(), flags.len(), "a flag is listed twice");

        let handled: BTreeSet<String> = literal_arguments(dispatcher(), "OsStr::new", 0)
            .into_iter()
            .collect();
        assert!(handled.len() >= 12, "{handled:?}");
        // The version and the help text are answered from the list itself,
        // and opening files needs no flag.
        let answered: BTreeSet<String> = MODES
            .iter()
            .filter(|mode| answer(Some(OsStr::new(mode.flag))).is_some())
            .map(|mode| mode.flag.to_owned())
            .collect();
        assert_eq!(
            answered,
            BTreeSet::from(["--help".to_owned(), "--version".to_owned()])
        );
        assert!(
            dispatcher().contains("cli_help::answer("),
            "main answers --version and --help before anything else"
        );
        let listed: BTreeSet<String> = MODES
            .iter()
            .filter(|mode| !mode.flag.is_empty() && !answered.contains(mode.flag))
            .map(|mode| mode.flag.to_owned())
            .collect();
        assert_eq!(
            handled, listed,
            "the flags main compares its first argument with, against the list"
        );
    }

    #[test]
    fn usage_lines_of_main_match_the_list() {
        let dispatcher = dispatcher();
        let mut checked = 0;
        for (at, _) in dispatcher.match_indices("\"Usage: ") {
            let (line, _) = literal_at(dispatcher, at).expect("a usage line is one literal");
            let flag = line
                .split_whitespace()
                .nth(2)
                .unwrap_or_else(|| panic!("{line}"));
            let mode = MODES
                .iter()
                .find(|mode| mode.flag == flag)
                .unwrap_or_else(|| panic!("{flag} is not listed"));
            assert_eq!(line, format!("Usage: {}", invocation(mode)));
            checked += 1;
        }
        assert!(checked >= 12, "only {checked} usage lines found");
    }

    /// The README at the root of the repository lists the modes for someone
    /// who has not started the program yet. Its command-line section names
    /// every flag the program accepts, each mode with its arguments as the
    /// list spells them, and no flag that does not exist.
    #[test]
    fn readme_lists_every_mode_and_no_flag_that_does_not_exist() {
        // Flags in a text: `--name` and the one-letter `-N`.
        fn flags(text: &str) -> BTreeSet<&str> {
            text.split(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
                .filter(|word| {
                    let long = word.strip_prefix("--").is_some_and(|name| {
                        name.starts_with(|first: char| first.is_ascii_alphabetic())
                    });
                    let short = word.len() == 2
                        && word.starts_with('-')
                        && word.ends_with(|letter: char| letter.is_ascii_alphabetic());
                    long || short
                })
                .collect()
        }

        let readme = include_str!("../../../README.md");
        let heading = "### Command line";
        let start = readme
            .find(heading)
            .expect("the README has a command-line section");
        let rest = &readme[start + heading.len()..];
        let section = &rest[..rest.find("\n### ").unwrap_or(rest.len())];
        // A table cell writes the bar between alternatives with a backslash.
        let section = section.replace("\\|", "|");

        let mut accepted = BTreeSet::new();
        for mode in MODES {
            let row = if mode.flag.is_empty() {
                invocation(mode)
            } else {
                [mode.flag, mode.arguments]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            assert!(
                section.contains(&format!("`{row}`")),
                "the README has no row `{row}`"
            );
            if let Some(short) = mode.short {
                assert!(
                    section.contains(&format!("`{short}`")),
                    "the README does not name {short}"
                );
                accepted.insert(short);
            }
            accepted.extend(flags(mode.flag));
            accepted.extend(flags(mode.arguments));
        }
        assert!(accepted.len() >= 20, "{accepted:?}");
        assert_eq!(
            flags(&section),
            accepted,
            "the flags the README names, against the flags the program accepts"
        );
    }
}
