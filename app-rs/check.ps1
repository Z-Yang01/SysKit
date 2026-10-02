# SysKit 重写门禁（对标 Python 侧 tools/run_full_check.py 的纪律）：
# fmt + clippy + cargo test + tsc + vitest，任一失败即非零退出。
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$failed = @()

Write-Host "==== [1/5] cargo fmt --check ====" -ForegroundColor Cyan
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
if ($LASTEXITCODE -ne 0) { $failed += "cargo fmt" }

Write-Host "==== [2/5] cargo clippy -D warnings ====" -ForegroundColor Cyan
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { $failed += "cargo clippy" }

Write-Host "==== [3/5] cargo test ====" -ForegroundColor Cyan
cargo test --manifest-path src-tauri/Cargo.toml
if ($LASTEXITCODE -ne 0) { $failed += "cargo test" }

Write-Host "==== [4/5] tsc --noEmit ====" -ForegroundColor Cyan
npx tsc --noEmit
if ($LASTEXITCODE -ne 0) { $failed += "tsc" }

Write-Host "==== [5/5] vitest run ====" -ForegroundColor Cyan
npx vitest run
if ($LASTEXITCODE -ne 0) { $failed += "vitest" }

if ($failed.Count -gt 0) {
    Write-Host "CHECK FAIL: $($failed -join ', ')" -ForegroundColor Red
    exit 1
}
Write-Host "CHECK PASS：fmt/clippy/test/tsc/vitest 全绿" -ForegroundColor Green
