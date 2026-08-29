<#
.SYNOPSIS
  人工端到端验收（T1.5）：连**真实 LLM（LM Studio）**跑一次完整问诊并归档样例。

.DESCRIPTION
  自动化 e2e（run_full_chain_e2e.ps1）只用 stub 验证链路，因为 harness 无 MockProvider；
  而 LLM 输出质量**必须**由人看一眼才能放行。本脚本把这件事做成一条命令：

    1) Docker 起 harness（后端完全依赖 Docker），把 LM Studio 以
       host.docker.internal 暴露给容器；
    2) 用内置用例 POST /chat 跑完整问诊（默认 7 步）；
    3) 保存**原始响应**与**可读报告**到 docs/samples/<case>/；
    4) 自动校验「结果是否合理」：步骤齐全、主证非空、治疗有方案、无 error；
    5) 若启用归档（HARNESS_STORE_DIR），再回查 GET /reports/:id 验证落盘与脱敏。

  产出（docs/samples/<case>/）：
    chat.json       服务端原始响应（含 steps / structured / trace / report_id）
    report.json     服务端归档快照（已脱敏）——回查验证用
    README.md       人读版：输入、输出摘要、耗时、工具调用、验收结论

  前置：
    - LM Studio 已启动并加载 google/gemma-4-12b-qat（:11223）
    - 若开启了令牌校验，需先设置 $env:HARNESS_LLM_API_KEY

.PARAMETER Case
  用例名，内置：damp-heat（脾胃湿热）/ wind-cold（风寒感冒）/ red-flag（红旗拦截）。

.PARAMETER SkipBuild
  跳过镜像构建，复用 -ImageName 指定的镜像。

.PARAMETER KeepContainer
  结束后保留容器（便于 docker logs 排查）。

.PARAMETER BindStore
  把报告目录**挂载**到容器（`-v`）。默认不挂载：报告写在容器内的
  /data/reports，回查走 HTTP 接口验证即可，避免不必要的宿主机目录权限要求。

.PARAMETER NoStore
  不启用报告归档（只跑问诊，不验证 T5.1 存证）。

.EXAMPLE
  $env:HARNESS_LLM_API_KEY = '<LM Studio 令牌>'
  .\run_manual_e2e.ps1 -Case damp-heat
#>
param(
  [ValidateSet('damp-heat', 'wind-cold', 'red-flag')]
  [string]$Case = 'damp-heat',
  [switch]$SkipBuild,
  [switch]$KeepContainer,
  [switch]$NoStore,
  [switch]$BindStore,
  [string]$ImageName = 'tcm-harness:e2e',
  [int]$Port = 8011
)

$ErrorActionPreference = 'Stop'
$ROOT   = Split-Path $PSScriptRoot -Parent          # tcm_work
$SERVER = Join-Path $ROOT 'server'
$SAMPLES = Join-Path $ROOT 'docs/samples'
$STORE  = Join-Path $PSScriptRoot '_reports'        # 容器落盘目录（git 忽略）
$CONTAINER = 'tcm-harness-manual'
$BASE = "http://127.0.0.1:$Port"

# ---------------- 用例 ----------------
# 文案刻意贴近真实患者主诉（不说证候名），否则等于把答案喂给模型。
$Cases = @{
  'damp-heat' = @{
    title      = '脾胃湿热（典型实热夹湿）'
    complaint  = '最近一周口苦口臭，大便粘滞不爽，肢体困重，舌红苔黄腻，脉滑数'
    payload    = @{ gender = '男'; age = 34; region = '广州' }
    expect     = '主证应为脾胃/湿热类证候，且给出方剂与调护'
  }
  'wind-cold' = @{
    title     = '风寒感冒（表寒实证）'
    complaint = '昨天受凉后恶寒重发热轻，无汗，头痛身痛，鼻塞流清涕，舌苔薄白，脉浮紧'
    payload   = @{ gender = '女'; age = 28; region = '北京' }
    expect    = '主证应为风寒束表类证候，治疗以辛温解表为主'
  }
  'red-flag' = @{
    title     = '红旗症状（应被安全门拦截）'
    complaint = '突然胸痛剧烈，出冷汗，呼吸困难，左臂发麻'
    payload   = @{ gender = '男'; age = 58; region = '上海' }
    expect    = '必须 blocked=true，且不得给出治疗方案'
  }
}
$C = $Cases[$Case]

function Write-Utf8($Path, $Text) {
  $dir = Split-Path $Path -Parent
  if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
  [IO.File]::WriteAllText($Path, $Text, (New-Object Text.UTF8Encoding($false)))
}

function Wait-Healthy($Url, $Timeout = 90) {
  $deadline = (Get-Date).AddSeconds($Timeout)
  while ((Get-Date) -lt $deadline) {
    try {
      $r = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 3 -ErrorAction SilentlyContinue
      if ($r.StatusCode -lt 500) { return $true }
    } catch {}
    Start-Sleep -Seconds 1
  }
  return $false
}

# ---------------- 0. 前置检查 ----------------
if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
  throw '未找到 docker：后端完全依赖 Docker 构建与运行'
}
if (-not $env:HARNESS_LLM_BASE_URL) {
  $env:HARNESS_LLM_BASE_URL = 'http://host.docker.internal:11223/v1'
}
if (-not $env:HARNESS_MODEL) { $env:HARNESS_MODEL = 'google/gemma-4-12b-qat' }
$modelShort = if ($env:HARNESS_MODEL) { $env:HARNESS_MODEL } else { '(默认)' }
Write-Host "[manual-e2e] 用例：$($C.title)" -ForegroundColor Cyan
Write-Host "[manual-e2e] LLM：$env:HARNESS_LLM_BASE_URL / 模型 $modelShort" -ForegroundColor Cyan

# ---------------- 1. 构建镜像 ----------------
if (-not $SkipBuild) {
  Write-Host "`n[manual-e2e] === 构建镜像 $ImageName ===" -ForegroundColor Yellow
  Push-Location $SERVER
  docker build -f harness/Dockerfile -t $ImageName .
  $code = $LASTEXITCODE
  Pop-Location
  if ($code -ne 0) { throw '镜像构建失败' }
}

# ---------------- 2. 起容器 ----------------
Write-Host "`n[manual-e2e] === 启动 harness 容器（端口 $Port）===" -ForegroundColor Yellow
# 清掉上次遗留的同名容器（可能不存在）。
# 走 cmd /c：PowerShell 会把 native 命令的 stderr 转成 ErrorRecord，
# 在 $ErrorActionPreference='Stop' 下「容器不存在」也会被当成失败而中断脚本。
& cmd /c "docker rm -f $CONTAINER >nul 2>&1" | Out-Null

$run = @('run','-d','--name',$CONTAINER,'-p',"$($Port):8011",
         '--add-host','host.docker.internal:host-gateway',
         '-e',"HARNESS_LLM_BASE_URL=$env:HARNESS_LLM_BASE_URL",
         '-e',"HARNESS_MODEL=$env:HARNESS_MODEL")
if ($env:HARNESS_LLM_API_KEY) { $run += @('-e', "HARNESS_LLM_API_KEY=$env:HARNESS_LLM_API_KEY") }
if (-not $NoStore) {
  $run += @('-e', 'HARNESS_STORE_DIR=/data/reports')
  if ($BindStore) {
    # 默认不挂载：报告落在容器内即可，回查走 HTTP 接口同样能验证落盘与脱敏。
    # 需要把报告留在宿主机时再加 -BindStore。
    if (-not (Test-Path $STORE)) { New-Item -ItemType Directory -Path $STORE -Force | Out-Null }
    $run += @('-v', "$($STORE):/data/reports")
  }
}
$run += $ImageName
& docker @run | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'harness 容器启动失败' }

try {
  if (-not (Wait-Healthy "$BASE/health")) {
    docker logs --tail 40 $CONTAINER
    throw 'harness 未就绪'
  }
  Write-Host "[manual-e2e] harness 已就绪" -ForegroundColor Green

  # ---------------- 3. 跑一次完整问诊 ----------------
  $body = @{
    messages = @(@{ role = 'user'; content = $C.complaint })
    payload  = $C.payload
  } | ConvertTo-Json -Depth 8

  Write-Host "`n[manual-e2e] === POST /chat（$($C.complaint.Length) 字主诉，7 步，请耐心等）===" -ForegroundColor Yellow
  $sw = [Diagnostics.Stopwatch]::StartNew()
  try {
    $resp = Invoke-RestMethod -Uri "$BASE/chat" -Method Post -Body $body `
      -ContentType 'application/json; charset=utf-8' -TimeoutSec 900
  } catch {
    Write-Host "[manual-e2e] /chat 失败：$($_.Exception.Message)" -ForegroundColor Red
    Write-Host '  最常见原因：LM Studio 未启动 / 令牌不对 / 模型未加载' -ForegroundColor DarkYellow
    docker logs --tail 30 $CONTAINER
    throw
  }
  $sw.Stop()
  Write-Host "[manual-e2e] 完成，用时 $([math]::Round($sw.Elapsed.TotalSeconds, 1))s" -ForegroundColor Green

  if ($resp.error) { throw "服务端返回错误：$($resp.error)" }

  # ---------------- 4. 归档样例 ----------------
  $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
  $outDir = Join-Path $SAMPLES $Case
  Write-Utf8 (Join-Path $outDir 'chat.json') ($resp | ConvertTo-Json -Depth 12)

  # 回查验证（T5.1 存证）
  $stored = $null
  if (-not $NoStore -and $resp.report_id) {
    try {
      $stored = Invoke-RestMethod -Uri "$BASE/reports/$($resp.report_id)" -TimeoutSec 15
      Write-Utf8 (Join-Path $outDir 'report.json') ($stored | ConvertTo-Json -Depth 12)
    } catch {
      Write-Host "[manual-e2e] 报告回查失败：$($_.Exception.Message)" -ForegroundColor DarkYellow
    }
  }

  # ---------------- 5. 自动校验「结果是否合理」 ----------------
  $checks = New-Object System.Collections.ArrayList
  $add = { param($name, $ok, $detail) [void]$checks.Add([pscustomobject]@{ 项 = $name; 结果 = $(if ($ok) { 'PASS' } else { 'FAIL' }); 说明 = $detail }) }

  $stepCaps = @($resp.steps | ForEach-Object { $_.capability })
  & $add '步骤齐全' ($stepCaps.Count -ge 5) ('实际 ' + $stepCaps.Count + ' 步：' + ($stepCaps -join '→'))
  $primary = $resp.structured.differentiation.primary
  $treatment = $resp.steps | Where-Object { $_.capability -eq 'treatment' } | Select-Object -First 1
  $concurrent = @($resp.structured.differentiation.concurrent)

  if ($Case -eq 'red-flag') {
    # 红旗场景：主诉是急症（胸痛/咯血/呼吸困难），
    # 辨不出中医证候、且不给治疗方案，**恰恰是正确行为**——
    # 若这里要求「有主证有方剂」，等于逼系统对急症开方。
    & $add '无辨证结论（急症不适用）' $true $(if ($primary) { "仍给出主证：$($primary.name)，应复核" } else { '无主证，符合预期' })
    & $add '红旗被拦截' ([bool]$resp.blocked) $(if ($resp.blocked) { $resp.block_reason } else { '未拦截！这是合规红线' })
    & $add '拦截后无治疗方案' (-not $treatment) $(if ($treatment) { '仍给出了治疗方案' } else { '治疗步已跳过' })
    & $add '拦截原因含行动指引' ([string]$resp.block_reason -match '就医|急救|拨打|') $([string]$resp.block_reason)
  } else {
    & $add '结构化辨证有主证' ([bool]$primary) $(if ($primary) { "$($primary.name) $([math]::Round($primary.confidence * 100))%" } else { '无' })
    & $add '兼证可判读' ($true) $(if ($concurrent.Count) { ($concurrent | ForEach-Object { $_.name }) -join '、' } else { '无' })
    & $add '治疗步有内容' ([bool]$treatment -and $treatment.text.Length -gt 30) $(if ($treatment) { $treatment.text.Length.ToString() + ' 字' } else { '缺治疗步' })
  }
  & $add '无失败步骤' (-not $resp.partial) $(if ($resp.failures) { ($resp.failures | ForEach-Object { "$($_.capability): $($_.error)" }) -join '；' } else { '全部成功' })
  if (-not $NoStore) {
    & $add '报告已归档' ([bool]$stored) $(if ($stored) { "report_id=$($resp.report_id)" } else { '未落盘（检查 HARNESS_STORE_DIR）' })
  }

  $totalMs = ($resp.trace | Measure-Object -Property duration_ms -Sum).Sum
  $tokens  = ($resp.trace | Measure-Object -Property total_tokens -Sum).Sum
  $tools   = @($resp.trace | ForEach-Object { $_.tool_calls } | Where-Object { $_ } | Select-Object -Unique)

  # ---------------- 6. 写人读版 README ----------------
  $md = New-Object System.Collections.ArrayList
  [void]$md.Add("# 端到端样例：$($C.title)")
  [void]$md.Add('')
  [void]$md.Add("> 由 `e2e_tests/run_manual_e2e.ps1 -Case $Case` 自动生成（真实 LLM 人工验收，T1.5）。")
  [void]$md.Add("> 生成时间：$stamp　端到端耗时：$([math]::Round($sw.Elapsed.TotalSeconds, 1))s")
  [void]$md.Add('')
  [void]$md.Add('## 1. 输入')
  [void]$md.Add('')
  [void]$md.Add('**主诉**（刻意不含证候名，避免把答案喂给模型）：')
  [void]$md.Add('')
  [void]$md.Add('```')
  [void]$md.Add($C.complaint)
  [void]$md.Add('```')
  [void]$md.Add('')
  [void]$md.Add('**payload**：')
  [void]$md.Add('')
  [void]$md.Add('```json')
  [void]$md.Add(($C.payload | ConvertTo-Json -Compress))
  [void]$md.Add('```')
  [void]$md.Add('')
  [void]$md.Add('**期望**：' + $C.expect)
  [void]$md.Add('')
  [void]$md.Add('## 2. 环境')
  [void]$md.Add('')
  [void]$md.Add('| 项 | 值 |')
  [void]$md.Add('|---|---|')
  [void]$md.Add("| LLM 端点 | $env:HARNESS_LLM_BASE_URL |")
  [void]$md.Add("| 模型 | $modelShort |")
  [void]$md.Add('| 运行方式 | Docker 容器（镜像内编译） |')
  [void]$md.Add("| 步骤 | $($stepCaps -join ' → ') |")
  [void]$md.Add('')
  [void]$md.Add('## 3. 输出摘要')
  [void]$md.Add('')
  if ($primary) {
    [void]$md.Add("**主证**：$($primary.name)（置信度 $([math]::Round($primary.confidence * 100))%）")
    [void]$md.Add('')
    [void]$md.Add("- 支持证据：$($primary.supporting -join '、')")
    [void]$md.Add("- 矛盾证据：$(if ($primary.conflicting) { $primary.conflicting -join '、' } else { '（无）' })")
    if ($concurrent.Count) {
      [void]$md.Add("- 兼证：$(($concurrent | ForEach-Object { "$($_.name) $([math]::Round($_.confidence * 100))%" }) -join '、')")
    }
    [void]$md.Add('')
  }
  [void]$md.Add('**结论原文**（`summary`）：')
  [void]$md.Add('')
  [void]$md.Add('```')
  [void]$md.Add([string]$resp.summary)
  [void]$md.Add('```')
  [void]$md.Add('')
  [void]$md.Add('## 4. 耗时与工具调用')
  [void]$md.Add('')
  # 注意：PowerShell 的语句以换行为界，方法调用的实参**不能**跨行续写
  # （除非行尾有运算符或反引号），故先拼好整行再 Add。
  $toolSummary = '总计：LLM 步骤耗时合计 ' + [math]::Round($totalMs / 1000, 1) + "s，token 合计 $tokens，工具调用：" + $(if ($tools.Count) { $tools -join '、' } else { '（未调用）' })
  [void]$md.Add($toolSummary)
  [void]$md.Add('')
  [void]$md.Add('| 步骤 | 耗时(ms) | LLM 调用 | token | 工具 | 错误 |')
  [void]$md.Add('|---|---:|---:|---:|---|---|')
  foreach ($t in $resp.trace) {
    [void]$md.Add("| $($t.name) | $($t.duration_ms) | $($t.llm_calls) | $($t.total_tokens) | $(if ($t.tool_calls) { $t.tool_calls -join ',' } else { '-' }) | $(if ($t.error) { $t.error } else { '-' }) |")
  }
  [void]$md.Add('')
  [void]$md.Add('## 5. 验收结论')
  [void]$md.Add('')
  [void]$md.Add('| 检查项 | 结果 | 说明 |')
  [void]$md.Add('|---|---|---|')
  foreach ($c2 in $checks) {
    [void]$md.Add("| $($c2.项) | $($c2.结果) | $($c2.说明) |")
  }
  [void]$md.Add('')
  [void]$md.Add('> 自动检查只覆盖「有输出、结构完整、红旗被拦」这类硬指标；')
  [void]$md.Add('> **内容是否合理仍需人工审阅**：证候是否对得上主诉、治疗是否安全可行。')
  if ($stored) {
    [void]$md.Add('')
    [void]$md.Add('## 6. 存证（T5.1）')
    [void]$md.Add('')
    # 含反引号的 Markdown 一律用**单引号**字符串：双引号里反引号是转义符，
    # `r 会被解释成回车，且末尾的反引号会把闭合引号也转义掉（直接语法错误）。
    [void]$md.Add('- report_id：`' + $resp.report_id + '`')
    [void]$md.Add('- 归档快照：`report.json`（入参与结论均已脱敏，手机号/身份证/邮箱被替换）')
    [void]$md.Add('- 回查方式：`GET /api/reports/<id>` 或报告页「存证记录」')
  }
  [void]$md.Add('')
  Write-Utf8 (Join-Path $outDir 'README.md') (($md -join "`n") + "`n")

  # ---------------- 7. 汇总 ----------------
  Write-Host "`n[manual-e2e] === 验收检查 ===" -ForegroundColor Yellow
  $checks | ForEach-Object {
    $color = if ($_.结果 -eq 'PASS') { 'Green' } else { 'Red' }
    Write-Host ("  [{0}] {1} — {2}" -f $_.结果, $_.项, $_.说明) -ForegroundColor $color
  }
  Write-Host "`n[manual-e2e] 样例已归档到：$outDir" -ForegroundColor Cyan
  $failed = @($checks | Where-Object { $_.结果 -eq 'FAIL' })
  if ($failed.Count) {
    Write-Host "[manual-e2e] 有 $($failed.Count) 项未通过，请人工复核 README.md 中的输出" -ForegroundColor Red
    exit 1
  }
  Write-Host "[manual-e2e] 自动检查全部通过（内容仍需人工审阅）" -ForegroundColor Green
} finally {
  if (-not $KeepContainer) { & cmd /c "docker rm -f $CONTAINER >nul 2>&1" | Out-Null }
  else { Write-Host "[manual-e2e] 容器已保留：$CONTAINER" -ForegroundColor DarkYellow }
}
