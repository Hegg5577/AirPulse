<#
.SYNOPSIS
发布构建前准备当前源码的 Hook 接收器及前端，任一步失败都停止打包。
.NOTES
保留 UTF-8 BOM，确保 Windows PowerShell 5.1 正确读取中文。
#>
$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path $PSScriptRoot -Parent
Push-Location $repositoryRoot
try {
    # 接收器复用桌面库，先准备前端资源，确保首次发布构建也有可嵌入的页面。
    & pnpm build
    if ($LASTEXITCODE -ne 0) { throw '前端构建失败，已停止打包。' }
    & cargo build --release --bin keyphore-hook --manifest-path src-tauri/Cargo.toml --locked
    if ($LASTEXITCODE -ne 0) { throw 'Hook 接收器编译失败，已停止打包。' }
    & (Join-Path $repositoryRoot 'plugin/package.ps1')
} finally {
    Pop-Location
}
