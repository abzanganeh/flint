# Stage native runtime DLLs next to cargo test executables.
#
# ort's copy-dylibs step tries to symlink ONNX Runtime / DirectML DLLs into
# target/debug, target/debug/examples, and target/debug/deps. On Windows CI
# (no Developer Mode) symlinks fail, ort falls back to copying into the first
# folder only, then stops. Unit/integration test binaries live under deps/ and
# crash at startup with STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) when DirectML
# or other ort dependencies are missing from that directory — or when a stale
# onnxruntime.dll from the cached target/ dir or C:\Windows\System32 wins.
#
# Run after `cargo test --no-run` (or `cargo build --all-targets`) and
# immediately before any `cargo test` on Windows.

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$TargetDebug = Join-Path $RepoRoot "src-tauri\target\debug"
$Deps = Join-Path $TargetDebug "deps"

# Cargo-built cdylibs (e.g. flint_lib.dll) are already in deps/ and are locked
# by the linker / test harness immediately after `cargo test --no-run`.
$SkipNames = [System.Collections.Generic.HashSet[string]]::new(
    [string[]]@('flint_lib.dll', 'flint.dll'),
    [StringComparer]::OrdinalIgnoreCase
)

# Third-party runtime DLLs that ort/Tauri place under target/debug/ but not deps/.
$RuntimePatterns = @(
    'onnxruntime*.dll',
    'DirectML.dll',
    'WebView2Loader.dll'
)

if (-not (Test-Path $Deps)) {
    New-Item -ItemType Directory -Path $Deps -Force | Out-Null
}

function Copy-DllToDeps {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourcePath,
        [switch]$ForceRefresh
    )

    if (-not (Test-Path -LiteralPath $SourcePath)) {
        return $false
    }

    $name = Split-Path -Leaf $SourcePath
    if ($SkipNames.Contains($name)) {
        return $false
    }

    $dest = Join-Path $Deps $name
    $srcFull = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $SourcePath).Path)
    $destFull = [System.IO.Path]::GetFullPath($dest)

    if ($srcFull -eq $destFull) {
        return $false
    }

    if (-not $ForceRefresh -and (Test-Path -LiteralPath $dest)) {
        $destResolved = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $dest).Path)
        if ($srcFull -eq $destResolved) {
            return $false
        }
    }

    try {
        [System.IO.File]::Copy($srcFull, $destFull, $true)
        return $true
    } catch {
        $msg = $_.Exception.Message
        if ($msg -match 'with itself|being used by another process') {
            Write-Host "  skip $name (already present or locked)"
            return $false
        }
        throw
    }
}

function Stage-Dll {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourcePath,
        [switch]$ForceRefresh
    )

    if (Copy-DllToDeps -SourcePath $SourcePath -ForceRefresh:$ForceRefresh) {
        Write-Host "  staged $(Split-Path -Leaf $SourcePath) -> deps/"
    }
}

function Stage-RuntimeDllsFrom {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Root,
        [switch]$Recurse,
        [switch]$ForceRefresh
    )

    foreach ($pattern in $RuntimePatterns) {
        $items = if ($Recurse) {
            Get-ChildItem -Path $Root -Recurse -Filter $pattern -File -ErrorAction SilentlyContinue
        } else {
            Get-ChildItem -Path $Root -Filter $pattern -File -ErrorAction SilentlyContinue
        }
        foreach ($item in $items) {
            Stage-Dll $item.FullName -ForceRefresh:$ForceRefresh
        }
    }
}

Write-Host "Staging runtime DLLs for cargo test (Windows)..."

# 1. target/debug — ort copy-dylibs landing zone on copy-fallback.
Stage-RuntimeDllsFrom -Root $TargetDebug -ForceRefresh

# 2. ort.pyke.io prebuilt bundle cache (DirectML + any bundled dylibs).
$OrtCacheRoot = Join-Path $env:LOCALAPPDATA "ort.pyke.io\dfbin\x86_64-pc-windows-msvc"
if (Test-Path $OrtCacheRoot) {
    Stage-RuntimeDllsFrom -Root $OrtCacheRoot -Recurse -ForceRefresh
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

# Point ort at the staged copy so we never pick up System32's older runtime.
$ortDll = Get-ChildItem -Path $Deps -Filter "onnxruntime*.dll" -File -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $ortDll) {
    $ortDll = Get-ChildItem -Path $TargetDebug -Filter "onnxruntime*.dll" -File -ErrorAction SilentlyContinue |
        Select-Object -First 1
}
if ($ortDll) {
    Write-Host "ORT_DYLIB_PATH=$($ortDll.FullName)"
    if ($env:GITHUB_ENV) {
        Add-Content -Path $env:GITHUB_ENV -Value "ORT_DYLIB_PATH=$($ortDll.FullName)"
    } else {
        $env:ORT_DYLIB_PATH = $ortDll.FullName
    }
}

# Belt-and-suspenders: prepend target/debug to PATH for any DLL ort only placed there.
$debugFull = [System.IO.Path]::GetFullPath($TargetDebug)
if ($env:GITHUB_ENV) {
    Add-Content -Path $env:GITHUB_ENV -Value "PATH=$debugFull;$env:PATH"
} else {
    $env:PATH = "$debugFull;$env:PATH"
}
