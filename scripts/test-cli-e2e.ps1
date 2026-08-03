param(
    [Parameter(Mandatory = $true)]
    [string]$Binary
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
$isWindowsHost = $env:OS -eq "Windows_NT"
$isMacHost = $false
if ($PSVersionTable.PSVersion.Major -ge 6) {
    $isMacHost = [bool]$IsMacOS
}

function Invoke-BinaryJson {
    param([string[]]$Arguments)

    $output = & $binaryPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "dcc-mcp-cua exited with code ${LASTEXITCODE}: $($Arguments -join ' ')"
    }
    try {
        return ($output | Out-String | ConvertFrom-Json)
    }
    catch {
        throw "dcc-mcp-cua returned invalid JSON for '$($Arguments[0])': $_"
    }
}

$help = & $binaryPath --help | Out-String
if ($LASTEXITCODE -ne 0 -or $help -notmatch "host-batch") {
    throw "release CLI help smoke test failed"
}

$manifest = Invoke-BinaryJson -Arguments @("manifest")
$expectedOs = if ($isWindowsHost) { "windows" } elseif ($isMacHost) { "macos" } else { "linux" }
if ($manifest.name -ne "dcc-mcp-cua" -or $manifest.target.os -ne $expectedOs) {
    throw "manifest does not describe the current release binary"
}
if ($manifest.version -notmatch '^\d+\.\d+\.\d+$' -or $manifest.host.protocol_version -ne 1) {
    throw "manifest version or Host protocol is invalid"
}
if ($manifest.host.snapshot_transports -notcontains "shared_memory" -or
    $manifest.host.capabilities -notcontains "two_axis_scroll" -or
    $manifest.host.capabilities -notcontains "host_ping" -or
    $manifest.host.capabilities -notcontains "host_diagnostics") {
    throw "manifest omitted required Host capabilities"
}

$batchRequest = @(
    [ordered]@{ request_id = "e2e-ping"; method = "ping"; params = @{} },
    [ordered]@{ request_id = "e2e-apps"; method = "list_apps"; params = @{} },
    [ordered]@{ request_id = "e2e-tools"; method = "list_tools"; params = @{} }
) | ConvertTo-Json -Depth 4 -Compress
$batchFile = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-mcp-cua-e2e-$([guid]::NewGuid().ToString('N')).json"
[System.IO.File]::WriteAllText($batchFile, $batchRequest, [System.Text.UTF8Encoding]::new($false))
try {
    $responses = @(Invoke-BinaryJson -Arguments @(
        "host-batch",
        "--spawn", $binaryPath,
        "--snapshot-transport", "shared_memory",
        "--json-file", $batchFile
    ))
}
finally {
    if (Test-Path -LiteralPath $batchFile) {
        Remove-Item -LiteralPath $batchFile -Force
    }
}

if ($responses.Count -ne 3 -or
    $responses[0].request_id -ne "e2e-ping" -or
    $responses[1].request_id -ne "e2e-apps" -or
    $responses[2].request_id -ne "e2e-tools") {
    throw "Host batch IPC did not preserve response order and correlation IDs"
}
if ($responses[0].type -ne "pong" -or $responses[0].protocol_version -ne 1) {
    throw "Host ping failed"
}
if ($responses[1].type -ne "apps" -or $null -eq $responses[1].apps) {
    throw "embedded CUA application inventory failed"
}
if ($responses[2].type -ne "tools" -or $null -eq $responses[2].tools) {
    throw "embedded CUA tool inventory failed"
}

$toolNames = @($responses[2].tools.tools | ForEach-Object { $_.name })
foreach ($requiredTool in @(
    "list_apps",
    "list_windows",
    "click",
    "scroll",
    "get_desktop_state",
    "start_session"
)) {
    if ($toolNames -notcontains $requiredTool) {
        throw "embedded CUA tool inventory omitted $requiredTool"
    }
}

$streamArguments = @(
    "host-jsonl",
    "--spawn", $binaryPath,
    "--parallel-discovery",
    "--snapshot-transport", "shared_memory"
)
$streamRequests = @(
    '{"request_id":"stream-ping-1","method":"ping","params":{}}',
    '{"request_id":"stream-doctor","method":"doctor","params":{}}',
    '{"request_id":"stream-desktop-open","method":"open_desktop_session","params":{"session_id":"cli-e2e-lifecycle","grant":{"task_grant_id":"cli-e2e","dcc_type":"desktop","allow_raw_input":false}}}',
    '{"request_id":"stream-desktop-stop","method":"stop_desktop_session","params":{"session_id":"cli-e2e-lifecycle"}}',
    '{"request_id":"stream-error","method":"unknown_method","params":{}}',
    '{"request_id":"stream-ping-2","method":"ping","params":{}}'
)
$streamInput = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-mcp-cua-e2e-$([guid]::NewGuid().ToString('N')).jsonl"
$streamOutput = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-mcp-cua-e2e-$([guid]::NewGuid().ToString('N')).out"
$streamError = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-mcp-cua-e2e-$([guid]::NewGuid().ToString('N')).err"
[System.IO.File]::WriteAllLines($streamInput, $streamRequests, [System.Text.UTF8Encoding]::new($true))
try {
    $stream = Start-Process -FilePath $binaryPath -ArgumentList $streamArguments -RedirectStandardInput $streamInput -RedirectStandardOutput $streamOutput -RedirectStandardError $streamError -PassThru -Wait
    if ($stream.ExitCode -ne 0) {
        throw "host-jsonl failed: $([System.IO.File]::ReadAllText($streamError))"
    }
    $streamResponses = @(Get-Content -LiteralPath $streamOutput | ForEach-Object { $_ | ConvertFrom-Json })
}
finally {
    foreach ($path in @($streamInput, $streamOutput, $streamError)) {
        if (Test-Path -LiteralPath $path) {
            Remove-Item -LiteralPath $path -Force
        }
    }
}

$streamJson = $streamResponses | ConvertTo-Json -Depth 12 -Compress
if ($streamResponses[0].request_id -ne "stream-ping-1" -or
    $streamResponses[0].type -ne "pong" -or
    $streamResponses[1].request_id -ne "stream-doctor" -or
    $streamResponses[1].type -ne "diagnostics" -or
    $streamResponses[1].schema_version -ne 1 -or
    $null -eq $streamResponses[1].checks.driver.success -or
    $null -eq $streamResponses[1].checks.health.success -or
    $null -eq $streamResponses[1].checks.interactive_desktop.success -or
    $streamResponses[4].request_id -ne "stream-error" -or
    $streamResponses[4].type -ne "error" -or
    $streamResponses[5].request_id -ne "stream-ping-2" -or
    $streamResponses[5].type -ne "pong") {
    throw "host-jsonl did not preserve diagnostics, correlation, or error recovery: $streamJson"
}

if ($streamResponses[2].type -eq "desktop_session_opened") {
    if ($streamResponses[2].request_id -ne "stream-desktop-open" -or
        $streamResponses[2].started.active -ne $true -or
        $streamResponses[3].request_id -ne "stream-desktop-stop" -or
        $streamResponses[3].type -ne "desktop_session_stopped" -or
        $streamResponses[3].result.active -ne $false) {
        throw "Host desktop session lifecycle failed: $streamJson"
    }
}
elseif (-not $isMacHost -or
    $streamResponses[1].ready -ne $false -or
    $streamResponses[2].request_id -ne "stream-desktop-open" -or
    $streamResponses[2].type -ne "error" -or
    [string]::IsNullOrWhiteSpace($streamResponses[2].code) -or
    $streamResponses[3].request_id -ne "stream-desktop-stop" -or
    $streamResponses[3].type -ne "error" -or
    [string]::IsNullOrWhiteSpace($streamResponses[3].code)) {
    throw "Host session lifecycle was not available without a structured macOS readiness refusal: $streamJson"
}

$endpointHost = $null
$endpoint = if ($isWindowsHost) {
    "\\.\pipe\dcc-mcp-cua-e2e-$([guid]::NewGuid().ToString('N'))"
} else {
    Join-Path ([System.IO.Path]::GetTempPath()) "dcc-mcp-cua-e2e-$([guid]::NewGuid().ToString('N')).sock"
}
$endpointBatchJson = @(
    [ordered]@{ request_id = "endpoint-apps"; method = "list_apps"; params = @{} },
    [ordered]@{ request_id = "endpoint-tools"; method = "list_tools"; params = @{} }
) | ConvertTo-Json -Depth 4 -Compress
$endpointBatchFile = Join-Path ([System.IO.Path]::GetTempPath()) "dcc-mcp-cua-e2e-$([guid]::NewGuid().ToString('N')).json"
[System.IO.File]::WriteAllText($endpointBatchFile, $endpointBatchJson, [System.Text.UTF8Encoding]::new($false))
try {
    $endpointStart = [System.Diagnostics.ProcessStartInfo]::new()
    $endpointStart.FileName = $binaryPath
    $endpointStart.UseShellExecute = $false
    $endpointStart.CreateNoWindow = $true
    $endpointArguments = @("host", "--endpoint", $endpoint)
    $argumentListProperty = $endpointStart.PSObject.Properties["ArgumentList"]
    if ($null -ne $argumentListProperty -and $null -ne $argumentListProperty.Value) {
        foreach ($argument in $endpointArguments) {
            [void]$argumentListProperty.Value.Add($argument)
        }
    }
    else {
        $endpointStart.Arguments = ($endpointArguments | ForEach-Object {
            if ($_ -match '[\s"]') { '"' + $_.Replace('"', '\"') + '"' } else { $_ }
        }) -join " "
    }
    $endpointHost = [System.Diagnostics.Process]::new()
    $endpointHost.StartInfo = $endpointStart
    if (-not $endpointHost.Start()) {
        throw "failed to start endpoint Host process"
    }

    $endpointPing = $null
    for ($attempt = 0; $attempt -lt 40 -and $null -eq $endpointPing; $attempt++) {
        if ($endpointHost.HasExited) {
            throw "endpoint Host exited before accepting connections with code $($endpointHost.ExitCode)"
        }
        try {
            $pingOutput = & $binaryPath host-call --endpoint $endpoint --method ping 2>$null
            if ($LASTEXITCODE -eq 0) {
                $endpointPing = ($pingOutput | Out-String | ConvertFrom-Json)
            }
        }
        catch {
            $endpointPing = $null
        }
        if ($null -eq $endpointPing) {
            Start-Sleep -Milliseconds 100
        }
    }
    if ($null -eq $endpointPing -or $endpointPing.type -ne "pong") {
        throw "cross-platform Host endpoint did not answer ping"
    }

    $endpointBatch = & $binaryPath host-batch --endpoint $endpoint --json-file $endpointBatchFile |
        Out-String | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or
        $endpointBatch.Count -ne 2 -or
        $endpointBatch[0].request_id -ne "endpoint-apps" -or
        $endpointBatch[0].type -ne "apps" -or
        $endpointBatch[1].request_id -ne "endpoint-tools" -or
        $endpointBatch[1].type -ne "tools") {
        throw "cross-platform Host endpoint batch IPC failed"
    }
}
finally {
    if ($null -ne $endpointHost) {
        if (-not $endpointHost.HasExited) {
            $endpointHost.Kill()
            [void]$endpointHost.WaitForExit(5000)
        }
        $endpointHost.Dispose()
    }
    if (-not $isWindowsHost -and (Test-Path -LiteralPath $endpoint)) {
        Remove-Item -LiteralPath $endpoint -Force
    }
    if (Test-Path -LiteralPath $endpointBatchFile) {
        Remove-Item -LiteralPath $endpointBatchFile -Force
    }
}

Write-Host "CLI E2E passed for ${expectedOs}: manifest, diagnostics, session lifecycle, batch/stream Host IPC, error recovery, apps, and $($toolNames.Count) CUA tools."
