# SysKit 门禁：fmt + clippy + cargo test + tsc + vitest，任一失败即非零退出。
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
$failed = @()

Write-Host "==== [1/5] cargo fmt --check ====" -ForegroundColor Cyan
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
if ($LASTEXITCODE -ne 0) { $failed += "cargo fmt" }

Write-Host "==== [2/5] cargo clippy -D warnings ====" -ForegroundColor Cyan
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { $failed += "cargo clippy" }

Write-Host "==== [3/5] cargo test（单线程） ====" -ForegroundColor Cyan
# 2026-10-08：三连并行 stress 全绿（831 例，v1.3.1 后）；历史 0xc0000005
# 未再复现，暂留单线程为稳（夜间/无人值守宁慢勿偶发崩）——观察若干天后
# 可尝试去掉 --test-threads=1（并行约 20s → ~8s）。
cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1
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
