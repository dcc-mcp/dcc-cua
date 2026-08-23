param(
    [Parameter(Mandatory = $true)]
    [string]$Binary
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$binaryCandidate = if ([System.IO.Path]::IsPathRooted($Binary)) { $Binary } else { Join-Path $repoRoot $Binary }
$binaryPath = (Resolve-Path $binaryCandidate).Path
$jsonlStart = [System.Diagnostics.ProcessStartInfo]::new()
$jsonlStart.FileName = $binaryPath
$jsonlStart.UseShellExecute = $false
$jsonlStart.RedirectStandardInput = $true
$jsonlStart.RedirectStandardOutput = $true
$jsonlStart.RedirectStandardError = $true
foreach ($argument in @(
    "host-jsonl",
    "--spawn", $binaryPath,
    "--snapshot-transport", "binary_frame"
)) {
    [void]$jsonlStart.ArgumentList.Add($argument)
}
$jsonl = [System.Diagnostics.Process]::new()
$jsonl.StartInfo = $jsonlStart
if (-not $jsonl.Start()) { throw "host-jsonl recovery probe did not start" }
try {
    $jsonl.StandardInput.WriteLine('{"request_id":"gui-stream-open","method":"open_desktop_session","params":{"session_id":"gui-stream-session","grant":{"task_grant_id":"gui-stream-task","application_label":"GUI E2E","allow_raw_input":false}}}')
    $jsonl.StandardInput.Flush()
    $open = $jsonl.StandardOutput.ReadLine() | ConvertFrom-Json
    if ($open.request_id -ne "gui-stream-open" -or
        $open.type -ne "desktop_session_opened" -or
        [string]::IsNullOrWhiteSpace($open.desktop_capability)) {
        throw "host-jsonl recovery probe could not open a desktop session: $($open | ConvertTo-Json -Depth 8 -Compress)"
    }

    $snapshot = @{
        request_id = "gui-stream-snapshot"
        method = "desktop_session_snapshot"
        params = @{
            session_id = "gui-stream-session"
            task_grant_id = "gui-stream-task"
            desktop_capability = $open.desktop_capability
        }
    } | ConvertTo-Json -Depth 8 -Compress
    $jsonl.StandardInput.WriteLine($snapshot)
    $jsonl.StandardInput.Flush()
    $outputError = $jsonl.StandardOutput.ReadLine() | ConvertFrom-Json
    if ($outputError.request_id -ne "gui-stream-snapshot" -or
        $outputError.type -ne "error" -or
        $outputError.code -ne "output_error") {
        throw "host-jsonl did not return a correlated recoverable image output error: $($outputError | ConvertTo-Json -Depth 8 -Compress)"
    }

    $jsonl.StandardInput.WriteLine('{"request_id":"gui-stream-stop","method":"stop_desktop_session","params":{"session_id":"gui-stream-session"}}')
    $jsonl.StandardInput.Flush()
    $stopped = $jsonl.StandardOutput.ReadLine() | ConvertFrom-Json
    if ($stopped.request_id -ne "gui-stream-stop" -or
        $stopped.type -ne "desktop_session_stopped" -or
        $stopped.result.active -ne $false) {
        throw "host-jsonl did not preserve the active session after output_error: $($stopped | ConvertTo-Json -Depth 8 -Compress)"
    }

    $jsonl.StandardInput.Close()
    if (-not $jsonl.WaitForExit(10000)) {
        throw "host-jsonl recovery probe did not stop after stdin EOF"
    }
    if ($jsonl.ExitCode -ne 0) {
        throw "host-jsonl recovery probe failed: $($jsonl.StandardError.ReadToEnd())"
    }
}
finally {
    if (-not $jsonl.HasExited) {
        $jsonl.Kill($true)
        $jsonl.WaitForExit()
    }
    $jsonl.Dispose()
}
