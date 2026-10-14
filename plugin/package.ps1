<#
.SYNOPSIS
仅复制已构建的 release 接收器，不自动编译、不安装插件或修改 Codex 配置。
.NOTES
保留 UTF-8 BOM，确保 Windows PowerShell 5.1 正确读取中文。
#>
$ErrorActionPreference = 'Stop'
$repositoryRoot = Split-Path $PSScriptRoot -Parent
$receiverPath = Join-Path $repositoryRoot 'src-tauri/target/release/keyphore-hook.exe'
if (-not (Test-Path -LiteralPath $receiverPath -PathType Leaf)) {
    throw '未找到 release 接收器，请先编译当前源码的 keyphore-hook 二进制。'
}
$pluginBin = Join-Path $PSScriptRoot 'bin'
[void](New-Item -ItemType Directory -Path $pluginBin -Force)
Copy-Item -LiteralPath $receiverPath -Destination (Join-Path $pluginBin 'keyphore-hook.exe') -Force
Write-Output '插件文件已准备到 plugin/bin；尚未安装，也未改变 Codex Hook 信任状态。'
