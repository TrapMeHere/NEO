# NEO performance validation script
#
# Reproduces all numbers in docs/14 and docs/prototypes/gate-1-prototype.md.
# Prerequisite: Rust stable toolchain, sample data downloaded (see README).
#
# Usage:
#   pwsh scripts/bench.ps1
#   pwsh scripts/bench.ps1 -Pbf .research/data/bremen-latest.osm.pbf -Synth 1000000 -Iterations 30 -Runs 3

param(
    [string]$Pbf = ".research/data/bremen-latest.osm.pbf",
    [int]$Synth = 1000000,
    [int]$Iterations = 30,
    [int]$Runs = 3
)

$ErrorActionPreference = 'Continue'
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$repo = (Get-Location).Path
$out = Join-Path $repo 'out'
New-Item -ItemType Directory -Force -Path $out | Out-Null

function Get-Median([double[]]$values) {
    if ($values.Count -eq 0) { return 0.0 }
    $sorted = $values | Sort-Object
    $mid = [math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 0) { return ($sorted[$mid - 1] + $sorted[$mid]) / 2.0 }
    return $sorted[$mid]
}

Write-Host "== build release ==" -ForegroundColor Cyan
cargo build --release -p neo-prototype 2>&1 | Select-Object -Last 3

$exe = Join-Path $repo 'out/cargo/release/neo-prototype.exe'
if (-not (Test-Path $exe)) { throw "missing $exe" }

# Run and capture: returns @{ ExitCode; PeakRssMB; StdOut }
function Invoke-Prototype {
    param([string[]]$Arguments, [string]$LogName)
    $log = Join-Path $out "$LogName.stdout.txt"
    $err = Join-Path $out "$LogName.stderr.txt"
    $p = Start-Process -FilePath $exe -ArgumentList $Arguments -PassThru -NoNewWindow `
        -RedirectStandardOutput $log -RedirectStandardError $err
    $p.WaitForExit()
    # PeakWorkingSet64 can still be read from the retained Process handle after exit (without -Wait)
    $peak = 0
    try { $peak = [int64]$p.PeakWorkingSet64 } catch { $peak = 0 }
    $text = Get-Content -Raw $log -ErrorAction SilentlyContinue
    return @{
        ExitCode = $p.ExitCode
        PeakRssMB = [math]::Round($peak / 1MB, 1)
        StdOut = $text
    }
}

Write-Host "== gpu info ==" -ForegroundColor Cyan
$gpu = Invoke-Prototype @('gpu-info') 'gpu-info'
$gpuLine = ($gpu.StdOut -split "`n" | Where-Object { $_ -match 'backend=' } | Select-Object -First 1)
Write-Host $gpuLine

# ---------------------------------------------------------------- Monaco
Write-Host "== monaco import/render ==" -ForegroundColor Cyan
$monaco = '.research/data/monaco-latest.osm.pbf'
$monacoImport = $null
$monacoRender = $null
if (Test-Path $monaco) {
    $monacoImport = Invoke-Prototype @('import', '--pbf', $monaco) 'monaco-import'
    Write-Host $monacoImport.StdOut
    $monacoRender = Invoke-Prototype @('render', '--pbf', $monaco, '--width', '1600', '--height', '1000', '--out', 'out/monaco.png') 'monaco-render'
    Write-Host $monacoRender.StdOut
} else {
    Write-Host "monaco pbf missing, skipping (see README)" -ForegroundColor Yellow
}

# ---------------------------------------------------------------- Bremen (real 1M+ PBF)
Write-Host "== bremen import (real PBF) ==" -ForegroundColor Cyan
$bremenRuns = @()
if (Test-Path $Pbf) {
    for ($i = 1; $i -le $Runs; $i++) {
        $r = Invoke-Prototype @('import', '--pbf', $Pbf) "bremen-import-$i"
        Write-Host "run $i : peak RSS $($r.PeakRssMB) MB"
        $bremenRuns += $r
    }
    $bremenRender = Invoke-Prototype @('render', '--pbf', $Pbf, '--width', '1920', '--height', '1200', '--out', 'out/bremen.png') 'bremen-render'
    Write-Host $bremenRender.StdOut
}

# ---------------------------------------------------------------- synthetic bench
Write-Host "== synthetic bench ($Synth objects) ==" -ForegroundColor Cyan
$benchRuns = @()
for ($i = 1; $i -le $Runs; $i++) {
    $r = Invoke-Prototype @('bench', '--synth', "$Synth", '--iterations', "$Iterations") "bench-$i"
    Write-Host "run $i : peak RSS $($r.PeakRssMB) MB"
    $benchRuns += $r
}

# ---------------------------------------------------------------- summary
# Parse the in-process self-reported peak working set from the prototype stdout (more reliable than the parent reading the Process handle)
function Get-PeakRss([string]$text) {
    if ($text -match 'peak working set: ([0-9.]+) MiB') { return [double]$Matches[1] }
    return 0.0
}

$summary = @()
$summary += "# NEO prototype: raw run summary"
$summary += ""
$summary += "- date: $(Get-Date -Format o)"
$summary += "- host: $([System.Environment]::MachineName), OS $([System.Environment]::OSVersion.VersionString)"
$summary += "- cpu: $((Get-CimInstance Win32_Processor | Select-Object -First 1).Name)"
$summary += "- gpu: $gpuLine"
$summary += "- runs per measurement: $Runs (median reported in docs/14)"
$summary += ""
$summary += "## Peak RSS (MB, reported by the process itself)"
$summary += ""
$summary += "| measurement | peak RSS MB |"
$summary += "|---|---|"
if ($monacoImport) { $summary += "| monaco import | $(Get-PeakRss $monacoImport.StdOut) |" }
if ($bremenRuns.Count) {
    $summary += "| bremen import (median of $($bremenRuns.Count)) | $([math]::Round((Get-Median ($bremenRuns | ForEach-Object { Get-PeakRss $_.StdOut })), 1)) |"
}
$summary += "| synth bench (median of $($benchRuns.Count)) | $([math]::Round((Get-Median ($benchRuns | ForEach-Object { Get-PeakRss $_.StdOut })), 1)) |"
$summary += ""
$summary += "## benchmark output of run 1"
$summary += ""
$summary += '```'
$summary += $benchRuns[0].StdOut
$summary += '```'
$summaryText = $summary -join "`n"
Set-Content -Path (Join-Path $out 'bench-summary.md') -Value $summaryText -Encoding utf8
Write-Host "`nsummary written to out/bench-summary.md" -ForegroundColor Green
Write-Host "next: fold these numbers into docs/14 and docs/prototypes/gate-1-prototype.md"
