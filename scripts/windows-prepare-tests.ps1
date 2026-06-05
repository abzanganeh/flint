# Stage native runtime DLLs next to cargo test executables.
#
# ort's copy-dylibs step places onnxruntime/DirectML/etc. in target/debug/
# but not in deps/ on Windows CI (symlinks require Developer Mode).
# Unit and integration test executables live in deps/ and crash with
# STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139) when those DLLs are absent.
#
# Dot-source this script so $env:PATH changes take effect in the caller:
#   . "$env:GITHUB_WORKSPACE/scripts/windows-prepare-tests.ps1"

$ErrorActionPreference = "Stop"

$RepoRoot   = Split-Path -Parent $PSScriptRoot
$TargetDebug = Join-Path $RepoRoot "src-tauri\target\debug"
$Deps        = Join-Path $TargetDebug "deps"

if (-not (Test-Path $Deps)) {
    New-Item -ItemType Directory -Path $Deps -Force | Out-Null
}

Write-Host "Staging runtime DLLs for cargo test (Windows)..."

$staged = 0

# Copy every DLL from target/debug/ into deps/.
# Cargo-owned cdylibs (flint_lib.dll etc.) are already in deps/ and may be
# locked — catching and ignoring those is intentional.
Get-ChildItem -Path $TargetDebug -Filter "*.dll" -File -ErrorAction SilentlyContinue |
    ForEach-Object {
        $srcFull  = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $_.FullName).Path)
        $destFull = [System.IO.Path]::GetFullPath((Join-Path $Deps $_.Name))

        if ($srcFull -eq $destFull) { return }

        try {
            [System.IO.File]::Copy($srcFull, $destFull, $true)
            Write-Host "  staged $($_.Name)"
            $staged++
        } catch {
            # File is locked (already linked into deps/ by cargo) — safe to skip.
        }
    }

Write-Host "  $staged DLL(s) copied from target/debug/"

# Also check the ort.pyke.io download cache for any DLLs ort placed there.
$OrtCache = Join-Path $env:LOCALAPPDATA "ort.pyke.io\dfbin\x86_64-pc-windows-msvc"
if (Test-Path $OrtCache) {
    Get-ChildItem -Path $OrtCache -Recurse -Filter "*.dll" -File -ErrorAction SilentlyContinue |
        ForEach-Object {
            $srcFull  = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $_.FullName).Path)
            $destFull = [System.IO.Path]::GetFullPath((Join-Path $Deps $_.Name))
            if ($srcFull -eq $destFull) { return }
            try {
                [System.IO.File]::Copy($srcFull, $destFull, $true)
                Write-Host "  staged $($_.Name) (ort cache)"
            } catch { }
        }
}

$total = (Get-ChildItem -Path $Deps -Filter "*.dll" -File -ErrorAction SilentlyContinue).Count
Write-Host "Done. $total DLL(s) in deps/."

# Prepend target/debug to PATH so the test exes find DLLs that ort placed
# there but that weren't picked up above.  Because this script is dot-sourced
# these assignments are visible to the calling shell.
$debugFull = [System.IO.Path]::GetFullPath($TargetDebug)
$env:PATH = "$debugFull;$env:PATH"
Write-Host "Prepended $debugFull to PATH"
