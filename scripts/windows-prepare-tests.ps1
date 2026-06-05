# Stage native runtime DLLs next to cargo test executables.
#
# ort's copy-dylibs step tries to symlink ONNX Runtime / DirectML DLLs into
# target/debug, target/debug/examples, and target/debug/deps. On Windows CI
# (no Developer Mode) symlinks fail, ort falls back to copying into the first
# folder only, then stops. Unit/integration test binaries live under deps/ and
# crash at startup with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) when DirectML
# or other ort dependencies are missing from that directory.
#
# Run after `cargo build --all-targets` and before any `cargo test` on Windows.

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$TargetDebug = Join-Path $RepoRoot "src-tauri\target\debug"
$Deps = Join-Path $TargetDebug "deps"

if (-not (Test-Path $Deps)) {
    New-Item -ItemType Directory -Path $Deps -Force | Out-Null
}

function Stage-Dll($SourcePath) {
    if (-not (Test-Path -LiteralPath $SourcePath)) {
        return
    }
    $name = Split-Path -Leaf $SourcePath
    $dest = Join-Path $Deps $name

    # Cargo may hard-link cdylib output; symlinks can make resolved paths equal.
    $srcFull = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $SourcePath).Path)
    $destFull = [System.IO.Path]::GetFullPath($dest)
    if ($srcFull -eq $destFull) {
        return
    }
    if (Test-Path -LiteralPath $dest) {
        return
    }

    Copy-Item -LiteralPath $SourcePath -Destination $dest -Force
    Write-Host "  staged $name -> deps/"
}

Write-Host "Staging runtime DLLs for cargo test (Windows)..."

# 1. Root of target/debug — ort copy-dylibs landing zone on copy-fallback.
# Exclude DLLs already living in deps/ (cdylib outputs land there directly).
Get-ChildItem -Path $TargetDebug -Filter "*.dll" -File -ErrorAction SilentlyContinue |
    Where-Object { $_.DirectoryName -ne $Deps } |
    ForEach-Object {
        Stage-Dll $_.FullName
    }

# 2. ort.pyke.io prebuilt bundle cache (DirectML + any bundled dylibs).
$OrtCacheRoot = Join-Path $env:LOCALAPPDATA "ort.pyke.io\dfbin\x86_64-pc-windows-msvc"
if (Test-Path $OrtCacheRoot) {
    Get-ChildItem -Path $OrtCacheRoot -Recurse -Filter "*.dll" -File | ForEach-Object {
        Stage-Dll $_.FullName
    }
}

# 3. Build-script outputs (whisper-rs, other native deps) — best-effort.
$BuildRoot = Join-Path $TargetDebug "build"
if (Test-Path $BuildRoot) {
    Get-ChildItem -Path $BuildRoot -Recurse -Filter "*.dll" -File -ErrorAction SilentlyContinue | ForEach-Object {
        Stage-Dll $_.FullName
    }
}

$count = (Get-ChildItem -Path $Deps -Filter "*.dll" -File -ErrorAction SilentlyContinue).Count
Write-Host "Done. $count DLL(s) in deps/."
