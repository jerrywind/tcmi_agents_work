# Local one-shot E2E runner: start backend -> run backend E2E -> run frontend contract -> stop backend.
# Usage: pwsh -ExecutionPolicy Bypass -File scripts/run_e2e.ps1
$ErrorActionPreference = 'Continue'
$ROOT = Resolve-Path (Join-Path $PSScriptRoot '..')
$BACKEND = Join-Path $ROOT 'backend'
$FRONTEND = Join-Path $ROOT 'frontend'
$PORT = 8000
$HEALTH = "http://127.0.0.1:$PORT/api/health"

function Test-BackendUp {
  try {
    $r = Invoke-WebRequest -Uri $HEALTH -UseBasicParsing -TimeoutSec 1
    return ($r.StatusCode -eq 200)
  } catch { return $false }
}

$startedHere = $false
$p = $null
if (Test-BackendUp) {
  Write-Host "[run_e2e] $HEALTH already up, reusing existing backend."
} else {
  Write-Host "[run_e2e] starting backend uvicorn (port $PORT)..."
  $log = Join-Path $BACKEND 'uvicorn_e2e.log'
  $err = Join-Path $BACKEND 'uvicorn_e2e_err.log'
  $p = Start-Process -NoNewWindow -FilePath python -ArgumentList @("-m","uvicorn","app.main:app","--host","127.0.0.1","--port",$PORT) -WorkingDirectory $BACKEND -PassThru -RedirectStandardOutput $log -RedirectStandardError $err
  $startedHere = $true
  $ready = $false
  for ($i = 0; $i -lt 30; $i++) {
    if (Test-BackendUp) { $ready = $true; break }
    Start-Sleep -Milliseconds 500
  }
  if (-not $ready) {
    Write-Host "[run_e2e] backend not ready in time, see uvicorn_e2e_err.log" -ForegroundColor Red
    if ($p) { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }
    exit 1
  }
}

$failed = $false
try {
  Write-Host "`n[run_e2e] === backend E2E (pytest -m e2e) ===" -ForegroundColor Cyan
  Set-Location $BACKEND
  python -m pytest -q -m e2e --cov=app --cov-report=term-missing
  if ($LASTEXITCODE -ne 0) { $failed = $true }

  Write-Host "`n[run_e2e] === frontend<->backend contract (vitest) ===" -ForegroundColor Cyan
  Set-Location $FRONTEND
  npx vitest run --no-cache src/services/api.contract.test.ts
  if ($LASTEXITCODE -ne 0) { $failed = $true }
} finally {
  if ($startedHere -and $p) {
    Write-Host "[run_e2e] stopping backend process..." -ForegroundColor Cyan
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  }
}

if ($failed) {
  Write-Host "`n[run_e2e] some tests failed." -ForegroundColor Red
  exit 1
}
Write-Host "`n[run_e2e] all passed." -ForegroundColor Green
