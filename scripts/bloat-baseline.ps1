# Record cargo-bloat crate breakdown for neo-desktop release.
# Requires: cargo install cargo-bloat --locked
# Usage: pwsh -File scripts/bloat-baseline.ps1

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CARGO_TARGET_DIR = Join-Path $root "out\cargo"

if (-not (Get-Command cargo-bloat -ErrorAction SilentlyContinue)) {
    Write-Host "installing cargo-bloat..." -ForegroundColor Cyan
    cargo install cargo-bloat --locked
}

$outDir = Join-Path $root "docs\prototypes"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$raw = Join-Path $outDir "binary-size-bloat-crates.txt"

Write-Host "== cargo bloat --release -p neo-desktop --crates ==" -ForegroundColor Cyan
cargo bloat --release -p neo-desktop --crates --n 40 | Tee-Object -FilePath $raw

$exe = Join-Path $root "out\cargo\release\neo-desktop.exe"
$exeMb = if (Test-Path $exe) { [math]::Round((Get-Item $exe).Length / 1MB, 2) } else { "n/a" }
$schema = Join-Path $root "assets\presets\schema.neo"
$suggestions = Join-Path $root "assets\presets\suggestions.neo"
$schemaMb = if (Test-Path $schema) { [math]::Round((Get-Item $schema).Length / 1MB, 2) } else { "missing" }
$suggestionMb = if (Test-Path $suggestions) { [math]::Round((Get-Item $suggestions).Length / 1MB, 2) } else { "missing" }
$fontMb = 0.0
Get-ChildItem (Join-Path $root "assets\fonts") -File -ErrorAction SilentlyContinue | ForEach-Object {
    $fontMb += $_.Length / 1MB
}
$fontMb = [math]::Round($fontMb, 2)

$md = Join-Path $outDir "binary-size-baseline.md"
$date = Get-Date -Format "yyyy-MM-dd"
@"
# 二进制体积基线（外置字体 + 外置 presets）

- **日期**：$date
- **命令**：``cargo bloat --release -p neo-desktop --crates --n 40``
- **复现**：``pwsh -File scripts/bloat-baseline.ps1``
- **profile**：``[profile.release]`` opt-level=3, lto=thin, codegen-units=1, strip=true

## 发行物体积

| 项 | 大小 |
|---|---:|
| ``out/cargo/release/neo-desktop.exe`` | $exeMb MB |
| ``assets/presets/schema.neo``（外置） | $schemaMb MB |
| ``assets/presets/suggestions.neo``（外置） | $suggestionMb MB |
| ``assets/fonts/*``（外置，合计） | $fontMb MB |

exe **不再**嵌入上述字体与预设。安装包 = exe + ``assets/``。

## crate 占用（``.text``）

原始输出见同目录 [binary-size-bloat-crates.txt](./binary-size-bloat-crates.txt)。

"@ | Set-Content -Path $md -Encoding utf8

Write-Host "wrote $md" -ForegroundColor Green
Write-Host "wrote $raw" -ForegroundColor Green
