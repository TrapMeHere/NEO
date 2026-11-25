# Installation and Running

## System requirements

| Item | Requirement |
|---|---|
| Operating system | Windows 10/11 x64 (primary support); Linux/macOS can build from source |
| GPU | DirectX 12 (Windows) or Vulkan/Metal support; integrated graphics are fine |
| Memory | 8 GB+ recommended; 16 GB+ recommended for editing 10 million objects |
| Disk | release exe (code) + side-by-side `assets/fonts` (~26 MB) + `assets/presets` (~15 MB) |

## Download the release (recommended)

1. Open <https://github.com/TrapMeHere/NEO/releases>
2. Download the latest `neo-desktop-vX.Y.Z-windows-x86_64.zip`
3. Extract to any directory (avoid directories that require administrator privileges)
4. Double-click `neo-desktop.exe`

Release packages also provide a `.sha256` checksum file. You can verify it in PowerShell:

```powershell
Get-FileHash .\neo-desktop-v0.1.0-windows-x86_64.zip -Algorithm SHA256
# compare with the contents of the .sha256 file
```

> The release build is a GUI program without a console: double-clicking it does not open a black command-line window.

## Building from source

Prerequisites: Rust stable (MSRV 1.97, the repository ships `rust-toolchain.toml`);
on Windows you need **Visual Studio Build Tools (MSVC)**.

```powershell
git clone https://github.com/TrapMeHere/NEO.git
cd NEO
cargo build --release -p neo-desktop
.\out\cargo\release\neo-desktop.exe
```

Build time depends on the machine; the first build takes about 3–6 minutes (including 20+ dependencies and bundled fonts).

## First launch

The map is empty after startup. Import local data via **File → Open PBF… / Open OSM XML…**,
or use the toolbar's **Download OSM Data** to fetch from the OSM API. No sample map is bundled.

## Uninstalling

NEO is a portable program: just delete the extracted directory. Edit records are written by default to
`out/neo-journal.jsonl` in the working directory; session files are saved explicitly by the user.

## Common installation problems

- **Double-click does nothing**: when the release build fails to start, it shows a dialog and writes `out/neo-startup-error.txt`;
  check whether the graphics driver supports DX12.
- **Chinese text becomes boxes**: 0.1.0 bundles CJK fonts, so this should not happen; if it does, file an issue with a screenshot.
- **NVIDIA "Alt+Z" prompt**: this is NVIDIA's global overlay prompt, see [FAQ](faq.md).
