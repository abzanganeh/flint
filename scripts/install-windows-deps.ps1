# Tauri 2.x Windows build dependencies for Flint.
# Run once on a fresh dev or CI Windows machine.
#
# Idempotent: skips chocolatey packages that are already installed.
# Sets LIBCLANG_PATH for the current session and prints the export line
# that the workflow consumes via $GITHUB_ENV.

$ErrorActionPreference = "Stop"

function Ensure-Choco {
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
        Write-Host "Chocolatey not found — bootstrapping..."
        Set-ExecutionPolicy Bypass -Scope Process -Force
        [System.Net.ServicePointManager]::SecurityProtocol = `
            [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
        Invoke-Expression ((New-Object System.Net.WebClient).DownloadString(
            "https://community.chocolatey.org/install.ps1"))
    }
}

function Install-Package($pkg) {
    # choco v2 removed --local-only; `choco list` is local-only by default.
    $installed = choco list --exact $pkg --limit-output 2>$null
    if ($installed -match "^$pkg\|") {
        Write-Host "$pkg already installed — skipping."
    } else {
        Write-Host "Installing $pkg..."
        choco install $pkg -y --no-progress
    }
}

Ensure-Choco

# cmake powers whisper-rs's whisper.cpp build, llvm provides libclang for
# bindgen-based crates (whisper-rs, rusqlite, etc.).
Install-Package "cmake"
Install-Package "llvm"

# Locate libclang and surface it via GITHUB_ENV when running under Actions.
$llvmRoot = "C:\Program Files\LLVM"
if (-not (Test-Path "$llvmRoot\bin\libclang.dll")) {
    Write-Error "libclang.dll not found under $llvmRoot — LLVM install may be incomplete."
    exit 1
}

$env:LIBCLANG_PATH = "$llvmRoot\bin"
Write-Host "LIBCLANG_PATH=$env:LIBCLANG_PATH"

if ($env:GITHUB_ENV) {
    Add-Content -Path $env:GITHUB_ENV -Value "LIBCLANG_PATH=$env:LIBCLANG_PATH"
    Add-Content -Path $env:GITHUB_PATH -Value "$llvmRoot\bin"
}

Write-Host "Windows dependencies installed. Verify with: cd src-tauri; cargo build"
