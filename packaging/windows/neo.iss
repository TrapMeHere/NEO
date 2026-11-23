; Inno Setup script for the Windows x64 zip produced by scripts/package.ps1.
; Run this on a machine with Inno Setup after the release binaries exist.

#define AppName "NEO"
#define AppVersion "0.1.0"
#define AppExe "neo-desktop.exe"

[Setup]
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={autopf}\NEO
OutputBaseFilename=neo-{#AppVersion}-windows-x64-setup
Compression=lzma2
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "neo-desktop.exe"; DestDir: "{app}"
Source: "neo-cli.exe"; DestDir: "{app}"
Source: "update.json"; DestDir: "{app}"
; 字体/预设是外置资源：漏打包会导致中文 tofu、预设为空。
Source: "assets\*"; DestDir: "{app}\assets"; Flags: recursesubdirs createallsubdirs
Source: "LICENSE"; DestDir: "{app}"
Source: "LICENSE-MIT"; DestDir: "{app}"
Source: "LICENSE-APACHE"; DestDir: "{app}"
Source: "NOTICE"; DestDir: "{app}"

[Icons]
Name: "{autoprograms}\NEO"; Filename: "{app}\{#AppExe}"

[Run]
Filename: "{app}\neo-cli.exe"; Parameters: "open --synth 1000000"; Flags: postinstall skipifsilent; Description: "Cold-start check with a 1M synthetic dataset"
