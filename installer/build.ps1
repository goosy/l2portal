# installer/build.ps1
# Build l2portal.exe and compile the Inno Setup installer.
#
# Prerequisites:
#   - Rust toolchain (stable-x86_64-pc-windows-gnu)
#   - npcap SDK in deps/npcap/sdk/
#   - npcap installer (any version) in deps/npcap/installer/npcap-*.exe
#   - Inno Setup 6 installed (iscc.exe in PATH or ISCC env var)
#
# Usage:
#   .\installer\build.ps1
#   .\installer\build.ps1 -SkipInstaller

param(
    [switch]$SkipInstaller
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir

Write-Host "[build] Project root: $ProjectRoot"

$CargoToml = Join-Path $ProjectRoot "Cargo.toml"
$CargoTomlContent = Get-Content $CargoToml -Raw
$VersionMatch = [regex]::Match($CargoTomlContent, '(?m)^version\s*=\s*"([^"]+)"')
if (-not $VersionMatch.Success) {
    throw "[build] Failed to read package version from '$CargoToml'."
}
$AppVersion = $VersionMatch.Groups[1].Value
Write-Host "[build] Cargo version: $AppVersion"

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
    # No --target specified; relies on rustup default (stable-x86_64-pc-windows-gnu).
    # This keeps the output in target/release/ instead of target/<triple>/release/.
    $cargoArgs = @("build", "--release")
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

    # Locate the npcap installer — match any npcap-*.exe in deps/npcap/installer/.
    $NpcapDir = Join-Path $ProjectRoot "deps\npcap\installer"
    $NpcapExe = Get-ChildItem -Path $NpcapDir -Filter "npcap-*.exe" -ErrorAction SilentlyContinue |
                Select-Object -First 1
    if (-not $NpcapExe) {
        throw "[build] No npcap-*.exe found in '$NpcapDir'. " +
              "Download from https://npcap.com/#download and place it there."
    }
    Write-Host "[build] npcap installer: $($NpcapExe.Name)"

    # Update setup.iss so it stays in sync and can also be compiled
    # directly with iscc without this script.
    $IssContent = Get-Content $IssFile -Raw
    $IssUpdated = $IssContent `
        -replace '(?m)(#define\s+MyAppVersion\s+")([^"]+)(")', "`${1}$AppVersion`${3}" `
        -replace 'npcap-[\d.]+\.exe', $NpcapExe.Name
    if ($IssUpdated -ne $IssContent) {
        Set-Content $IssFile $IssUpdated -NoNewline
        Write-Host "[build] setup.iss updated: version -> $AppVersion, npcap installer -> $($NpcapExe.Name)"
    }

    # Create output directory.
    $DistDir = Join-Path $ProjectRoot "dist"
    if (-not (Test-Path $DistDir)) { New-Item -ItemType Directory -Path $DistDir | Out-Null }

    Write-Host "[build] Compiling installer: $IssFile"
    & $Iscc $IssFile
    if ($LASTEXITCODE -ne 0) { throw "[build] Inno Setup compilation failed (exit $LASTEXITCODE)" }
    Write-Host "[build] Installer written to $DistDir"
}

Write-Host "[build] Done."
