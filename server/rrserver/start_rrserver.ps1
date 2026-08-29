$ErrorActionPreference = "Stop"

# 一键启动 rrserver（云端中继 server）。
#
# 路径说明（此前写死的是迁移前的 tcm_work/rrserver，已失效）：
#   - 源码与配置现位于 server/rrserver
#   - 由于 rrserver 与 harness 同属 server/ Cargo workspace，
#     编译产物统一在 server/target/{debug,release}/ 下，而非 server/rrserver/target/

$Root = Split-Path $PSScriptRoot -Parent          # server/
Set-Location $PSScriptRoot                          # server/rrserver（相对路径读取 config/）

$exe = Join-Path $Root "target/debug/rrserver.exe"
if (-not (Test-Path $exe)) {
    $exe = Join-Path $Root "target/release/rrserver.exe"
}
if (-not (Test-Path $exe)) {
    Write-Host "未找到 rrserver 二进制，请先构建（在 server/ 目录）：cargo build -p rrserver" -ForegroundColor Red
    Write-Host "注意：按项目约定，后端一律通过 Docker 构建与验证，不要依赖本地产物。" -ForegroundColor Yellow
    exit 1
}

Start-Process -FilePath $exe -ArgumentList "server", "--config", "config/rrserver.toml" `
    -RedirectStandardOutput "rrserver.log" -RedirectStandardError "rrserver.err" -NoNewWindow

Start-Sleep -Seconds 3
$p = Get-Process -Name "rrserver" -ErrorAction SilentlyContinue
if ($p) {
    Write-Host "rrserver started, pid: $($p.Id)" -ForegroundColor Green
} else {
    Write-Host "rrserver NOT RUNNING - check rrserver.err" -ForegroundColor Red
    exit 1
}
