# rrserver tunnel <-> backend <-> local llm service integration dry-run orchestrator.
# Order: mock llm -> cloud server -> home tunnel client -> run dry-run client.
# Always stops all child processes at the end.
#
# NOTE: uses httpx (a backend dependency) for the readiness probe instead of
# Invoke-WebRequest, which crashes in non-interactive PowerShell ("speech/
# prompt unavailable"). Port 8080 is often occupied on dev machines, so we
# default to 18080/19091.
param(
    [int]$CloudPort = 18080,
    [int]$MockPort  = 19091
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # avoid WebRequest progress UI crash
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$bin  = Join-Path $root "target/release/rrserver.exe"
$mock = Join-Path $PSScriptRoot "mock_llm_server.py"
$conf = Join-Path $PSScriptRoot "tunnel_server.toml"
$cli  = Join-Path $PSScriptRoot "backend_via_tunnel.py"

if (-not (Test-Path $bin)) { throw "missing $bin, run cargo build --release first" }

$procs = @()
function Start-Background ($Name, $Exe, $CliArgs, $LogFile) {
    $p = Start-Process -FilePath $Exe -ArgumentList $CliArgs `
        -RedirectStandardOutput $LogFile -RedirectStandardError ($LogFile + ".err") `
        -PassThru -WindowStyle Hidden
    $procs += $p
    Write-Host ("[{0}] pid={1} -> {2}" -f $Name, $p.Id, $LogFile)
}

# readiness probes via httpx (robust in non-interactive shells)
function Wait-Get ($Url, $TimeoutSec) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $code = python -c "import httpx,sys; print(httpx.get(sys.argv[1], timeout=2).status_code)" $Url 2>$null
            if ($code -eq 200) { return $true }
        } catch {}
        Start-Sleep -Milliseconds 200
        Write-Host "." -NoNewline
    }
    return $false
}

function Wait-Tunnel ($Url, $TimeoutSec) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $code = python -c "import httpx,sys; print(httpx.post(sys.argv[1], json={'model':'text-default','messages':[{'role':'user','content':'ping'}]}, timeout=2).status_code)" $Url 2>$null
            if ($code -eq 200) { return $true }
        } catch {}
        Start-Sleep -Milliseconds 200
        Write-Host "." -NoNewline
    }
    return $false
}

try {
    # 1) mock llm service (stand-in for real llm_server)
    Start-Background "mock" "python" "$mock $MockPort" (Join-Path $PSScriptRoot "mock.log")
    $m = Wait-Get "http://127.0.0.1:$MockPort/health" 10
    if (-not $m) { throw "mock llm service not ready in 10s" }
    Write-Host ("[ok] mock llm service ready on :{0}" -f $MockPort)

    # 2) cloud relay server
    Start-Background "server" $bin "server --listen 127.0.0.1:$CloudPort --config $conf" (Join-Path $PSScriptRoot "server.log")
    $s = Wait-Get "http://127.0.0.1:$CloudPort/healthz" 10
    if (-not $s) { throw "rrserver server not ready in 10s" }
    Write-Host ("[ok] rrserver server ready on :{0}" -f $CloudPort)

    # 3) home tunnel client (--local -> mock)
    $local = "http://127.0.0.1:$MockPort"
    Start-Background "client" $bin "client --server http://127.0.0.1:$CloudPort --name home --token secret --local $local" (Join-Path $PSScriptRoot "client.log")

    $t = Wait-Tunnel "http://127.0.0.1:$CloudPort/t/home/v1/chat/completions" 10
    if (-not $t) { throw "tunnel not connected within 10s" }
    Write-Host ("[ok] tunnel connected (home -> {0})" -f $local)

    # 4) run the integration dry-run client (real backend provider + true streaming SSE)
    Write-Host "`n--- running backend_via_tunnel.py ---"
    $env:CLOUD = "http://127.0.0.1:$CloudPort"
    python $cli
    if ($LASTEXITCODE -ne 0) { throw ("dry-run client exited non-zero ({0})" -f $LASTEXITCODE) }
    Write-Host "`n>>> DRYRUN PASSED <<<"
}
catch {
    Write-Host ("`n!!! DRYRUN FAILED: {0}" -f $_) -ForegroundColor Red
    exit 1
}
finally {
    foreach ($p in $procs) {
        if (-not $p.HasExited) {
            Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
            Write-Host ("[stop] pid={0}" -f $p.Id)
        }
    }
}
