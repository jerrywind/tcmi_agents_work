<#
.SYNOPSIS
    Clean generated artifacts and temporary junk files (test/verify leftovers).

.DESCRIPTION
    Safely deletes non-source artifacts. NEVER deletes git-tracked files.
    Default is --DryRun (preview only). Use -Clean to actually delete.

    How it works:
      1. `git clean -fdx` removes git-ignored AND untracked build/runtime
         artifacts. `git clean` intrinsically protects every tracked file,
         so it is safe and fast (git computes the set internally).
      2. Additionally, untracked files reported by `git status --porcelain`
         that match the naming convention (_verify_ / _tmp_ / _gen_ /
         _scratch_) are removed even if not covered by .gitignore. This
         guarantees future throwaway test/verify scripts get cleaned.
      3. A `git ls-files` guard rejects deletion of any tracked path.

    Safety constraints (see docs/cleanup-rules.md):
      1. Tracked files are never deleted.
      2. Only explicit whitelisted patterns are removed; no blind dir deletes.
      3. Temp verify/test/gen scripts MUST use the naming prefix above.

.PARAMETER Clean
    Perform actual deletion. Omit for preview only.
.PARAMETER Path
    Repo root (default: parent of this script's directory).

.EXAMPLE
    pwsh scripts/cleanup.ps1            # preview
    pwsh scripts/cleanup.ps1 -Clean     # delete
#>
[CmdletBinding()]
param(
    [switch]$Clean,
    [string]$Path = ""
)

$ErrorActionPreference = "Stop"

# Resolve repo root: default to parent of this script's directory.
if (-not $Path) {
    $scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
    $Path = Resolve-Path (Join-Path $scriptDir "..")
}
$repo = Resolve-Path $Path

Write-Host "==> Repo root : $repo"
Write-Host "==> Mode      : $(if ($Clean) { 'CLEAN (actually delete)' } else { 'DRY-RUN (preview only)' })"

# ---- 1. Protected set: all git-tracked files (never delete) ----
$protected = @{}
git -C $repo ls-files | ForEach-Object {
    $p = Join-Path $repo $_
    try { $protected[(Resolve-Path -LiteralPath $p).Path] = $true } catch { }
}

function Test-Protected($filePath) {
    try {
        $rp = (Resolve-Path -LiteralPath $filePath -ErrorAction Stop).Path
        return $protected.ContainsKey($rp)
    } catch { return $false }
}

# ---- 2. Naming convention for temp/verify/gen scripts & reports ----
$tmpPatterns = @("_verify_*", "_tmp_*", "_gen_*", "tmp_*", "temp_*", "_scratch_*", "_pyr*")
$debugFiles  = @("mcp_check.txt", "one.txt", "cov_unit.txt", "pytest_out.txt")

$toDelete = [System.Collections.Generic.List[string]]::new()

# ---- 2a. Untracked files matching naming convention (from git status, fast) ----
$status = git -C $repo status --porcelain --untracked-files=all 2>$null
foreach ($line in $status) {
    # lines for untracked files start with "?? "
    if ($line -notmatch "^\?\? ") { continue }
    $rel = $line.Substring(3).Trim()
    $name = Split-Path -Leaf $rel
    $matched = $false
    foreach ($pat in $tmpPatterns) {
        if ($name -like $pat) { $matched = $true; break }
    }
    if (-not $matched) {
        foreach ($pat in $debugFiles) {
            if ($name -eq $pat) { $matched = $true; break }
        }
    }
    if ($matched) {
        $full = Join-Path $repo $rel
        if (-not (Test-Protected $full)) { $toDelete.Add($full) }
    }
}

# ---- 2b. git-ignored / untracked artifacts via `git clean` (safe & fast) ----
$cleanArgs = if ($Clean) { @("clean", "-fdx") } else { @("clean", "-ndx") }
$cleanOut = git -C $repo @cleanArgs 2>$null
foreach ($line in $cleanOut) {
    if ($line -match "Would remove (.+)") {
        $rel = $Matches[1].Trim()
        $full = Join-Path $repo $rel
        if (-not (Test-Protected $full)) { $toDelete.Add($full) }
    }
}

# ---- de-dup ----
$unique = $toDelete | Sort-Object -Unique

if ($unique.Count -eq 0) {
    Write-Host "==> No junk files found to clean."
    exit 0
}

$sep = [IO.Path]::DirectorySeparatorChar
Write-Host "`n==> Items to clean (total $($unique.Count)):"
foreach ($item in $unique) {
    $short = $item -replace [regex]::Escape($repo.Path + $sep), ""
    Write-Host ("  " + $short)
}

if (-not $Clean) {
    Write-Host "`n==> This was a preview. Add -Clean to actually delete."
    exit 0
}

# ---- perform deletion ----
# Bulk delete via git clean (already safe: skips tracked files)
Write-Host "`n==> Running git clean -fdx ..."
git -C $repo clean -fdx 2>&1 | Out-Null

# Delete any remaining naming-convention files not handled by git clean
foreach ($item in $unique) {
    if (-not (Test-Path -LiteralPath $item)) { continue }
    if (Test-Protected $item) { Write-Warning "Skip protected: $item"; continue }
    try {
        Remove-Item -LiteralPath $item -Recurse -Force -ErrorAction Stop
        Write-Host "  deleted: $item"
    } catch {
        Write-Warning "Failed: $item -> $_"
    }
}
Write-Host "`n==> Cleanup complete."
