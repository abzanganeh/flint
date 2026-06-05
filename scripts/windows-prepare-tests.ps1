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
        if ($_.Exception.Message -match 'with itself') {
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

Write-Host "Staging runtime DLLs for cargo test (Windows)..."

# 1. Root of target/debug — ort copy-dylibs landing zone on copy-fallback.
Get-ChildItem -Path $TargetDebug -Filter "*.dll" -File -ErrorAction SilentlyContinue |
    Where-Object { $_.DirectoryName -ne $Deps } |
    ForEach-Object {
        Stage-Dll $_.FullName
    }

# 2. Always refresh ort runtime DLLs — cached deps/ copies go stale across
#    CI runs and Windows may fall back to System32\onnxruntime.dll (wrong version).
foreach ($pattern in @('onnxruntime*.dll', 'DirectML.dll')) {
    Get-ChildItem -Path $TargetDebug -Filter $pattern -File -ErrorAction SilentlyContinue |
        ForEach-Object { Stage-Dll $_.FullName -ForceRefresh }
}

# 3. ort.pyke.io prebuilt bundle cache (DirectML + any bundled dylibs).
$OrtCacheRoot = Join-Path $env:LOCALAPPDATA "ort.pyke.io\dfbin\x86_64-pc-windows-msvc"
if (Test-Path $OrtCacheRoot) {
    Get-ChildItem -Path $OrtCacheRoot -Recurse -Filter "*.dll" -File | ForEach-Object {
        Stage-Dll $_.FullName
    }
    foreach ($pattern in @('onnxruntime*.dll', 'DirectML.dll')) {
        Get-ChildItem -Path $OrtCacheRoot -Recurse -Filter $pattern -File -ErrorAction SilentlyContinue |
            ForEach-Object { Stage-Dll $_.FullName -ForceRefresh }
    }
}

# 4. Build-script outputs (whisper-rs, other native deps) — best-effort.
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
