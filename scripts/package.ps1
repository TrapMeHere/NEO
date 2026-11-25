# Build a Windows zip layout and write the update manifest the other platforms share.
# Linux and macOS wrappers live in packaging/. This script does not cross-compile.
#
# 不清理 out\cargo\release。编完后把 exe + fonts/presets/i18n 放到 out\neo-desktop\release，
# 再打进 dist 发行布局。
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$env:CARGO_TARGET_DIR = Join-Path $root "out\cargo"

Write-Host "== presets ==" -ForegroundColor Cyan
node scripts\build-presets.mjs
if ($LASTEXITCODE -ne 0) { throw "build-presets.mjs failed with exit $LASTEXITCODE" }

$packages = @("neo-cli", "neo", "neo-desktop", "neo-sync", "neo-imagery")
foreach ($pkg in $packages) {
    cargo build -p $pkg --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build $pkg failed" }
}

function Publish-Bundle([string]$BundleDir) {
    if (Test-Path $BundleDir) {
        try {
            Remove-Item -LiteralPath $BundleDir -Recurse -Force -ErrorAction Stop
        } catch {
            throw "无法重建 $BundleDir（可能 exe 仍在运行）。请先关闭后重试。`n$($_.Exception.Message)"
        }
    }
    New-Item -ItemType Directory -Force -Path $BundleDir | Out-Null

    $map = @{
        "neo.exe" = "neo.exe"
        "neo-desktop.exe" = "neo-editor.exe"
        "neo-sync.exe" = "neo-sync.exe"
        "neo-imagery.exe" = "neo-imagery.exe"
        "neo-cli.exe" = "neo-cli.exe"
    }
    foreach ($srcName in $map.Keys) {
        $src = Join-Path $root "out\cargo\release\$srcName"
        if (-not (Test-Path $src)) { throw "missing $src" }
        Copy-Item $src (Join-Path $BundleDir $map[$srcName]) -Force
    }

    $fontsSrc = Join-Path $root "assets\fonts"
    $presetDir = Join-Path $root "assets\presets"
    $i18nSrc = Join-Path $root "i18n"
    if (-not (Test-Path $fontsSrc)) { throw "missing assets\fonts" }
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
}

$bundleDir = Join-Path $root "out\neo-desktop\release"
Publish-Bundle $bundleDir
Write-Host "bundle -> $bundleDir" -ForegroundColor Green

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null

$stage = Join-Path $dist "neo-windows-x64"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
foreach ($name in @(
    "neo.exe",
    "neo-editor.exe",
    "neo-sync.exe",
    "neo-imagery.exe",
    "neo-cli.exe"
)) {
    Copy-Item (Join-Path $bundleDir $name) $stage
}
Copy-Item "packaging\windows\neo.iss" $stage
Copy-Item (Join-Path $bundleDir "assets") $stage -Recurse -Force
Copy-Item (Join-Path $bundleDir "i18n") $stage -Recurse -Force

$version = "0.1.0"
@{
    version = $version
    url     = "https://example.invalid/neo-$version-windows-x64.zip"
} | ConvertTo-Json | Set-Content (Join-Path $stage "update.json") -Encoding utf8

$zip = Join-Path $dist "neo-windows-x64.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "wrote $zip"
