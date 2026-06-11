# Remove stale whisper-rs-sys CMake build dirs from a restored cargo cache.
#
# whisper-rs-sys runs cmake with the host Visual Studio generator. When GHA
# Windows runners upgrade VS (e.g. 2022 -> 2026), a restored target/ cache
# still contains CMakeCache.txt pinned to the old generator and the build
# fails with "generator does not match the generator used previously".

$ErrorActionPreference = "Stop"

$targetRoot = Join-Path $PSScriptRoot ".." "src-tauri" "target"
if (-not (Test-Path $targetRoot)) {
    Write-Host "No target dir at $targetRoot — nothing to clean."
    exit 0
}

$profiles = @("debug", "release")
$removed = 0

foreach ($profile in $profiles) {
    $buildDir = Join-Path $targetRoot $profile "build"
    if (-not (Test-Path $buildDir)) { continue }

    Get-ChildItem -Path $buildDir -Filter "whisper-rs-sys-*" -Directory |
        ForEach-Object {
            Write-Host "Removing stale whisper-rs-sys cmake cache: $($_.FullName)"
            Remove-Item -LiteralPath $_.FullName -Recurse -Force
            $removed++
        }
}

if ($removed -eq 0) {
    Write-Host "No whisper-rs-sys build dirs found — cache is clean."
} else {
    Write-Host "Removed $removed whisper-rs-sys build dir(s)."
}
