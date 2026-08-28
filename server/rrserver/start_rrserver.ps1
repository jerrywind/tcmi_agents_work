$ErrorActionPreference = "Stop"
Set-Location "d:/labs/windblue_tech/tcm_work/rrserver"

$exe = "D:\labs\windblue_tech\tcm_work\rrserver\target\debug\rrserver.exe"
Start-Process -FilePath $exe -ArgumentList "server","--config","config/rrserver.toml" `
    -RedirectStandardOutput "rrserver.log" -RedirectStandardError "rrserver.err" -NoNewWindow

Start-Sleep -Seconds 3
$p = Get-Process -Name "rrserver" -ErrorAction SilentlyContinue
if ($p) {
    Write-Host "rrserver started, pid: $($p.Id)"
} else {
    Write-Host "rrserver NOT RUNNING - check rrserver.err"
}
