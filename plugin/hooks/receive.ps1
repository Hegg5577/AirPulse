<#
.SYNOPSIS
使用原始 stdin 调用插件内接收器，避免 PowerShell 管道重编码 JSON。
#>
$ErrorActionPreference = 'Stop'
$receiverPath = Join-Path (Split-Path $PSScriptRoot -Parent) 'bin/keyphore-hook.exe'
$hookProcess = $null
$hookStarted = $false
try {
    if (-not (Test-Path -LiteralPath $receiverPath -PathType Leaf)) {
        throw '插件缺少已编译的 keyphore-hook.exe'
    }
    $processInfo = New-Object System.Diagnostics.ProcessStartInfo
    $processInfo.FileName = $receiverPath
    $processInfo.UseShellExecute = $false
    $processInfo.CreateNoWindow = $true
    $processInfo.RedirectStandardInput = $true
    $processInfo.RedirectStandardOutput = $true
    $processInfo.RedirectStandardError = $true
    $processInfo.StandardOutputEncoding = [Text.Encoding]::UTF8
    $processInfo.StandardErrorEncoding = [Text.Encoding]::UTF8
    $hookProcess = New-Object System.Diagnostics.Process
    $hookProcess.StartInfo = $processInfo
    [void]$hookProcess.Start()
    $hookStarted = $true
    # 同时读取两个输出管道，防止等待进程退出时互相阻塞。
    $stdoutTask = $hookProcess.StandardOutput.ReadToEndAsync()
    $stderrTask = $hookProcess.StandardError.ReadToEndAsync()
    [Console]::OpenStandardInput().CopyTo($hookProcess.StandardInput.BaseStream)
    $hookProcess.StandardInput.Close()
    $hookProcess.WaitForExit()
    [Console]::Out.Write($stdoutTask.GetAwaiter().GetResult())
    [Console]::Error.Write($stderrTask.GetAwaiter().GetResult())
    exit $hookProcess.ExitCode
} catch {
    # 不输出 stdin 或异常中的任意输入内容，也不返回会阻止 Codex 的退出码 2。
    [Console]::Error.WriteLine('Keyphore Hook 接收失败，请检查插件二进制和本地状态目录。')
    exit 1
} finally {
    if ($null -ne $hookProcess) {
        if ($hookStarted -and -not $hookProcess.HasExited) { $hookProcess.Kill() }
        $hookProcess.Dispose()
    }
}
