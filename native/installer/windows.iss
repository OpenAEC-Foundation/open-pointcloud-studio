; Windows installer of Open Pointcloud Studio.
;
; Compile with the version and the folder that holds the application and
; its licence texts, for example:
;   ISCC /DAppVersion=0.8.0 /DSourceDir=..\target\packages\open-pointcloud-studio_0.8.0_windows-x64 windows.iss
;
; SourceDir is the folder that native/packaging/build-archive.sh leaves
; behind: the application, Open CAD Studio beside it and the licence texts.
; A pre-release such as 0.8.0-rc1 also needs /DAppNumericVersion=0.8.0,
; because the version details of the installer file take numbers only.

#ifndef AppVersion
  #error AppVersion is not set
#endif
#ifndef SourceDir
  #error SourceDir is not set
#endif
#ifndef OutputDir
  #define OutputDir "..\target\installer"
#endif
#ifndef AppNumericVersion
  #define AppNumericVersion AppVersion
#endif

#define AppName "Open Pointcloud Studio"
#define AppExe "open-pointcloud-studio.exe"
; Open CAD Studio, which shows exported drawings; the application looks for it
; beside itself.
#define CadExe "OpenCADStudio.exe"
#define ProgId "OpenPointcloudStudio.Scan"

[Setup]
AppId={{6D0B3C0E-52B7-4F0E-9D1C-3B7A7B2B8A41}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=OpenAEC Foundation
AppPublisherURL=https://github.com/OpenAEC-Foundation/open-pointcloud-studio
AppSupportURL=https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Installs for the current user without elevation; the dialog offers an
; installation for all users instead.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#SourceDir}\LICENSE-GPL-3.0.txt
SetupIconFile=..\assets\icons\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
; The ending _x64-setup.exe is what the download buttons of the foundation's
; website look for; native/packaging/expected-assets.sh lists all names.
OutputBaseFilename=open-pointcloud-studio_{#AppVersion}_x64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesAssociations=yes
CloseApplications=yes
VersionInfoVersion={#AppNumericVersion}
VersionInfoProductName={#AppName}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "dutch"; MessagesFile: "compiler:Languages\Dutch.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; Excludes: "{#CadExe}"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; Named on its own, so that an installer cannot be built without it.
Source: "{#SourceDir}\{#CadExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Offer the application under "Open with" for point-cloud files without
; taking over the program that opens them by default.
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueName: ""; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.e57\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.las\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.laz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ply\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pcd\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ptx\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.pts\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.xyz\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
; A scan project file lists the scans of a project; the application opens
; those it finds beside it.
Root: HKA; Subkey: "Software\Classes\.rcp\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
