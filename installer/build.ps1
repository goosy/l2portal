# installer/build.ps1
# Build l2portal.exe and compile the Inno Setup installer.
#
# Prerequisites:
#   - Rust toolchain (stable, target x86_64-pc-windows-msvc)
#   - npcap SDK in deps/npcap/sdk/
#   - Inno Setup 6 installed (iscc.exe in PATH or ISCC env var)
#
# Usage:
#   .\installer\build.ps1
#   .\installer\build.ps1 -Release      (default)
#   .\installer\build.ps1 -SkipInstaller

param(
    [switch]$Release   = $true,
    [switch]$SkipInstaller
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir

Write-Host "[build] Project root: $ProjectRoot"

# ── Set npcap SDK paths ──────────────────────────────────────────────────────
$NpcapSdk = Join-Path $ProjectRoot "deps\npcap\sdk"
if (Test-Path $NpcapSdk) {
    $env:LIB     = "$NpcapSdk\Lib\x64"
    $env:INCLUDE = "$NpcapSdk\Include"
    Write-Host "[build] npcap SDK: $NpcapSdk"
} else {
    Write-Warning "[build] deps\npcap\sdk not found — npcap SDK paths not set. Build may fail."
}

# ── Compile Rust ─────────────────────────────────────────────────────────────
Push-Location $ProjectRoot
try {
    $cargoArgs = @("build", "--target", "x86_64-pc-windows-msvc")
    if ($Release) { $cargoArgs += "--release" }
    Write-Host "[build] Running: cargo $($cargoArgs -join ' ')"
    cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "[build] cargo build failed (exit $LASTEXITCODE)" }
} finally {
    Pop-Location
}

# ── Compile Inno Setup installer ─────────────────────────────────────────────
if (-not $SkipInstaller) {
    $Iscc = if ($env:ISCC) { $env:ISCC } else { "iscc.exe" }
    $IssFile = Join-Path $ProjectRoot "installer\setup.iss"

    # Create output directory.
    $DistDir = Join-Path $ProjectRoot "dist"
    if (-not (Test-Path $DistDir)) { New-Item -ItemType Directory -Path $DistDir | Out-Null }

    Write-Host "[build] Compiling installer: $IssFile"
    & $Iscc $IssFile
    if ($LASTEXITCODE -ne 0) { throw "[build] Inno Setup compilation failed (exit $LASTEXITCODE)" }
    Write-Host "[build] Installer written to $DistDir"
}

Write-Host "[build] Done."
