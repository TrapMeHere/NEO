# Build the NEO desktop app. Double-click scripts\build.cmd, or run from the repo root:
#   pwsh -File scripts\build.ps1
#   pwsh -File scripts\build.ps1 -Release
#   pwsh -File scripts\build.ps1 -Run
#
# Does not clean the out\cargo build artifacts. After building, copies neo.exe / neo-editor.exe / neo-sync.exe /
# neo-imagery.exe plus runtime dependencies (fonts / presets / i18n) to
# out\neo-desktop\{debug|release}\, ready to run.

param(
    [switch]$Release,
    [switch]$Run,
    [switch]$Workspace
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$cargoHome = Join-Path $env:USERPROFILE ".cargo\bin"
$env:Path = "$cargoHome;$env:Path"

# Always write to out\cargo so an ambient temporary CARGO_TARGET_DIR cannot hijack it.
$env:CARGO_TARGET_DIR = Join-Path $root "out\cargo"

function Find-NodeHome {
    $cmd = Get-Command node -ErrorAction SilentlyContinue
    if ($cmd) {
        return Split-Path -Parent $cmd.Source
    }
    $portable = Join-Path $env:TEMP "neo-node\node-v22.21.1-win-x64"
    $exe = Join-Path $portable "node.exe"
    if (Test-Path $exe) {
        return $portable
    }
    return $null
}

function Publish-DesktopBundle([string]$CargoDir, [string]$BundleDir) {
    if (Test-Path $BundleDir) {
        try {
            Remove-Item -LiteralPath $BundleDir -Recurse -Force -ErrorAction Stop
        } catch {
            throw "cannot rebuild $BundleDir (the exe may still be running). Close neo / neo-editor and retry.`n$($_.Exception.Message)"
        }
    }
    New-Item -ItemType Directory -Force -Path $BundleDir | Out-Null

    $map = @{
        "neo.exe" = "neo.exe"
        "neo-desktop.exe" = "neo-editor.exe"
        "neo-sync.exe" = "neo-sync.exe"
        "neo-imagery.exe" = "neo-imagery.exe"
    }
    foreach ($srcName in $map.Keys) {
        $src = Join-Path $CargoDir $srcName
        if (-not (Test-Path $src)) {
            throw "missing $src"
        }
        Copy-Item $src (Join-Path $BundleDir $map[$srcName]) -Force
    }

    $fontsSrc = Join-Path $root "assets\fonts"
    $presetDir = Join-Path $root "assets\presets"
    $i18nSrc = Join-Path $root "i18n"
    if (-not (Test-Path $fontsSrc)) {
        throw "missing assets\fonts"
    }
    foreach ($name in @("schema.neo", "suggestions.neo")) {
        if (-not (Test-Path (Join-Path $presetDir $name))) {
            throw "missing assets\presets\$name — run node scripts/build-presets.mjs first"
        }
    }
    foreach ($name in @("zh-CN.json", "zh-TW.json", "en-US.json")) {
        if (-not (Test-Path (Join-Path $i18nSrc $name))) {
            throw "missing i18n\$name"
        }
    }

    $fontsDst = Join-Path $BundleDir "assets\fonts"
    $presetsDst = Join-Path $BundleDir "assets\presets"
    $i18nDst = Join-Path $BundleDir "i18n"
    New-Item -ItemType Directory -Force -Path $fontsDst | Out-Null
    New-Item -ItemType Directory -Force -Path $presetsDst | Out-Null
    New-Item -ItemType Directory -Force -Path $i18nDst | Out-Null
    Copy-Item (Join-Path $fontsSrc "*") $fontsDst -Force
    Copy-Item (Join-Path $presetDir "schema.neo") $presetsDst -Force
    Copy-Item (Join-Path $presetDir "suggestions.neo") $presetsDst -Force
    $presetReadme = Join-Path $root "assets\presets\README.md"
    if (Test-Path $presetReadme) {
        Copy-Item $presetReadme $presetsDst -Force
    }
    Copy-Item (Join-Path $i18nSrc "zh-CN.json") $i18nDst -Force
    Copy-Item (Join-Path $i18nSrc "zh-TW.json") $i18nDst -Force
    Copy-Item (Join-Path $i18nSrc "en-US.json") $i18nDst -Force

    $fontCount = @(Get-ChildItem $fontsDst -File).Count
    Write-Host ("bundle -> {0}  (fonts={1}, presets, i18n, 4 exes)" -f $BundleDir, $fontCount) -ForegroundColor Green
}

$nodeHome = Find-NodeHome
if (-not $nodeHome) {
    throw "Node.js 22 not found. Install Node, or place the executable in TEMP\neo-node\node-v22.21.1-win-x64\"
}
$env:Path = $nodeHome + ";" + $env:Path
$nodeVer = & node -v
$npmVer = & npm -v
Write-Host "node $nodeVer  npm $npmVer" -ForegroundColor Cyan

$schema = Join-Path $root "node_modules\@openstreetmap\id-tagging-schema\package.json"
$nsi = Join-Path $root "node_modules\name-suggestion-index\package.json"
if (-not ((Test-Path $schema) -and (Test-Path $nsi))) {
    Write-Host "== npm ci ==" -ForegroundColor Cyan
    npm ci --ignore-scripts --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw "npm ci failed with exit $LASTEXITCODE" }
} else {
    Write-Host "preset packages already installed" -ForegroundColor DarkGray
}

$subdir = if ($Release) { "release" } else { "debug" }
$profile = if ($Release) { "release" } else { "dev" }
$cargoArgs = @("build", "--locked")
if ($Release) {
    $cargoArgs += "--release"
}
if ($Workspace) {
    $cargoArgs += "--workspace"
    Write-Host "== cargo build --workspace ($profile) ==" -ForegroundColor Cyan
} else {
    $cargoArgs += @("-p", "neo", "-p", "neo-desktop", "-p", "neo-sync", "-p", "neo-imagery")
    Write-Host "== cargo build desktop suite ($profile) ==" -ForegroundColor Cyan
}

Write-Host "== presets ==" -ForegroundColor Cyan
node scripts\build-presets.mjs
if ($LASTEXITCODE -ne 0) { throw "build-presets.mjs failed with exit $LASTEXITCODE" }

& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit $LASTEXITCODE" }

$cargoDir = Join-Path $root "out\cargo\$subdir"
$bundleDir = Join-Path $root "out\neo-desktop\$subdir"
Publish-DesktopBundle $cargoDir $bundleDir

$bundleExe = Join-Path $bundleDir "neo.exe"
$editorExe = Join-Path $bundleDir "neo-editor.exe"

$info = Get-Item $editorExe
$sizeMb = [math]::Round($info.Length / 1MB, 1)
Write-Host ("ready {0} ({1} MB, {2})" -f $info.FullName, $sizeMb, $info.LastWriteTime) -ForegroundColor Green

if ($Run) {
    Write-Host "starting neo..." -ForegroundColor Cyan
    Start-Process -FilePath $bundleExe -WorkingDirectory $bundleDir
}
