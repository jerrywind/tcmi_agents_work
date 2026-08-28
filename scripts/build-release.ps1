<#
.SYNOPSIS
  Build Linux release binaries for the server/ workspace (for Docker images).
  Compatible with Windows PowerShell 5.1 and PowerShell 7+.

.DESCRIPTION
  Why this script exists:
    Inside a Docker build container the network corrupts crates.io downloads
    (manifest parse failure, exit 101), so `cargo build` cannot run there.
    Both images (harness / rrserver) therefore ship a PRE-BUILT Linux binary,
    and this script produces those binaries.

  Why it builds inside the WSL native filesystem by default:
    The repo lives on the Windows filesystem; WSL accesses /mnt/d over 9p, which
    is extremely slow for a full compile. So the sources are copied to a native
    ext4 dir (~/tcm-build/server), compiled there, and the two binaries are
    copied back to server/target/release/ where `docker build` can COPY them.

  Output paths (Dockerfiles depend on these - do not change):
    server/target/release/harness
    server/target/release/rrserver
  (harness and rrserver share one workspace; sub-crates have no own target/,
   so the docker build context MUST be the workspace root: server/.)

.PARAMETER InPlace
  Compile directly under /mnt/d instead (slower, but no copy needed).

.PARAMETER Clean
  Run `cargo clean` first (full rebuild).

.EXAMPLE
  powershell -NoProfile -File scripts\build-release.ps1

  Then deploy:
    cd frontend && npm run build:h5
    docker compose -f deploy/docker-compose.yml up -d --build
#>
param(
  [switch]$InPlace,
  [switch]$Clean
)

$ErrorActionPreference = 'Stop'

# ---- 1. Paths -------------------------------------------------
$RepoRoot  = Split-Path $PSScriptRoot -Parent          # tcm_work
$ServerDir = Join-Path $RepoRoot 'server'

# Windows path -> WSL path (d:\labs\... -> /mnt/d/labs/...)
$WslServer = ($ServerDir -replace '\\', '/')
if ($WslServer -match '^([A-Za-z]):') {
  $WslServer = '/mnt/' + $Matches[1].ToLower() + $WslServer.Substring(2)
}

Write-Host "[build] repo root : $RepoRoot" -ForegroundColor Cyan
Write-Host "[build] wsl path  : $WslServer" -ForegroundColor Cyan

# ---- 2. Environment checks ------------------------------------
if (-not (Get-Command wsl -ErrorAction SilentlyContinue)) {
  throw "wsl not found. Install WSL2 (Ubuntu 24.04 recommended: glibc 2.39 matches the runtime image)."
}
$cargoVer = (wsl -e bash -c '. "$HOME/.cargo/env" 2>/dev/null; cargo --version' | Out-String).Trim()
if ($cargoVer -notmatch '^cargo ') {
  throw "cargo not found in WSL (got: $cargoVer). Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
}
Write-Host "[build] $cargoVer" -ForegroundColor Cyan

# ---- 3. Pick compile dir --------------------------------------
$sw = [System.Diagnostics.Stopwatch]::StartNew()
# Use ~/ (the WSL default user's home) - /root is not writable for a non-root user.
# '~' is expanded by bash, so it must stay as-is in the command string.
$WslBuildDir = '~/tcm-build/server'

if ($InPlace) {
  Write-Host '[build] mode: in-place under /mnt/d (slower)' -ForegroundColor Yellow
  $compileDir = $WslServer
} else {
  Write-Host '[build] mode: WSL native dir (fast), then copy back' -ForegroundColor Yellow
  $compileDir = $WslBuildDir

  Write-Host "[build] syncing sources -> $WslBuildDir ..." -ForegroundColor Yellow
  $sync = @'
set -e
mkdir -p @@BUILD@@
cd '@@SRC@@'
for d in harness rrserver; do
  rm -rf @@BUILD@@/$d
  cp -r $d @@BUILD@@/
done
cp Cargo.toml @@BUILD@@/ 2>/dev/null || true
cp Cargo.lock @@BUILD@@/ 2>/dev/null || true
echo synced
'@
  $sync = $sync -replace '@@BUILD@@', $WslBuildDir
  $sync = $sync -replace '@@SRC@@', $WslServer
  # PowerShell here-strings use CRLF; bash chokes on the trailing \r.
  $sync = $sync -replace "`r`n", "`n"
  wsl -e bash -c $sync
  if ($LASTEXITCODE -ne 0) { throw "source sync failed (exit $LASTEXITCODE)" }
}

# ---- 4. Compile -----------------------------------------------
if ($Clean) {
  Write-Host '[build] cargo clean (full rebuild)...' -ForegroundColor Yellow
  wsl -e bash -c ('cd ' + $compileDir + ' && . "$HOME/.cargo/env" && cargo clean')
  if ($LASTEXITCODE -ne 0) { throw 'cargo clean failed' }
}

Write-Host '[build] cargo build --release (first run is slow)...' -ForegroundColor Yellow
wsl -e bash -c ('cd ' + $compileDir + ' && . "$HOME/.cargo/env" && cargo build --release')
if ($LASTEXITCODE -ne 0) {
  throw "cargo build --release failed (exit $LASTEXITCODE). See output above."
}

$sw.Stop()
Write-Host ("[build] compiled in {0}s" -f [int]$sw.Elapsed.TotalSeconds) -ForegroundColor Green

# ---- 5. Copy binaries back ------------------------------------
if (-not $InPlace) {
  Write-Host '[build] copying binaries back to server/target/release/ ...' -ForegroundColor Yellow
  $copy = @'
set -e
mkdir -p '@@SRC@@/target/release'
cp -f @@BUILD@@/target/release/harness  '@@SRC@@/target/release/harness'
cp -f @@BUILD@@/target/release/rrserver '@@SRC@@/target/release/rrserver'
echo copied
'@
  $copy = $copy -replace '@@BUILD@@', $WslBuildDir
  $copy = $copy -replace '@@SRC@@', $WslServer
  $copy = $copy -replace "`r`n", "`n"
  wsl -e bash -c $copy
  if ($LASTEXITCODE -ne 0) { throw "copy back failed (exit $LASTEXITCODE)" }
}

# ---- 6. Verify ------------------------------------------------
$missing = @()
foreach ($bin in @('harness', 'rrserver')) {
  $p = Join-Path $ServerDir ('target/release/' + $bin)
  if (Test-Path $p) {
    $size = [math]::Round((Get-Item $p).Length / 1MB, 2)
    Write-Host ("[build] OK  target/release/{0} ({1} MB)" -f $bin, $size) -ForegroundColor Green
  } else {
    $missing += $bin
    Write-Host "[build] NG  missing target/release/$bin" -ForegroundColor Red
  }
}

if ($missing.Count -gt 0) {
  throw ('binaries not produced: ' + ($missing -join ', ') + '. Docker build would fail.')
}

Write-Host ''
Write-Host '[build] done. Next steps:' -ForegroundColor Cyan
Write-Host '        cd frontend && npm run build:h5            # static assets for nginx'
Write-Host '        docker compose -f deploy/docker-compose.yml up -d --build'
