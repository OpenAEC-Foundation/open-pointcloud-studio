# Point count report: an example extension of Open Pointcloud Studio.
#
# It reads the status of the window through the local API and shows the
# number of points of the open scans in the status bar. With --save, as the
# tile on the Export page of the File view passes, it asks where to write a
# CSV report with a line per scan.
#
# It runs in Windows PowerShell 5.1, which comes with Windows, and in
# PowerShell 7. The window starts it with:
#   OPS_API_PORT, OPS_API_TOKEN  the local API and the token of this run
#   OPS_EXTENSION_ID             the id of the extension
#   OPS_CONTEXT                  a JSON file with what the window shows

$ErrorActionPreference = 'Stop'
$save = $args -contains '--save'
$url = "http://127.0.0.1:$($env:OPS_API_PORT)/exec"
$headers = @{ 'X-OPS-Token' = $env:OPS_API_TOKEN }
$invariant = [System.Globalization.CultureInfo]::InvariantCulture

# Send one command and return its answer; a refusal ends the script with
# the reason, which the status bar shows.
function Invoke-Ops([hashtable]$command) {
    $json = ConvertTo-Json -InputObject $command -Compress -Depth 8
    $body = [System.Text.Encoding]::UTF8.GetBytes($json)
    $answer = Invoke-RestMethod -Uri $url -Method Post -Headers $headers `
        -Body $body -ContentType 'application/json; charset=utf-8'
    if (-not $answer.ok) {
        throw "$($command.command): $($answer.error)"
    }
    return $answer
}

function Format-Count([int64]$count) {
    return $count.ToString('N0', $invariant)
}

# What the window showed when the run started.
$context = Get-Content -Raw -Encoding UTF8 -Path $env:OPS_CONTEXT | ConvertFrom-Json
Write-Output "Started from: $($context.entry); active scan: $($context.active_scan.path)"

Invoke-Ops @{ command = 'report_progress'; percent = 10; text = 'Reading the status' } | Out-Null
$status = (Invoke-Ops @{ command = 'status' }).result
$scans = @($status.clouds | Where-Object { $null -ne $_ })
$remaining = [int64]0
$selected = [int64]0
foreach ($scan in $scans) {
    $remaining += [int64]$scan.remaining
    $selected += [int64]$scan.selected
}
Invoke-Ops @{ command = 'report_progress'; percent = 60; text = 'Counting points' } | Out-Null

$noun = if ($scans.Count -eq 1) { 'scan' } else { 'scans' }
$summary = "$($scans.Count) $noun, $(Format-Count $remaining) points, $(Format-Count $selected) selected"
Write-Output $summary

if (-not $save) {
    Invoke-Ops @{ command = 'show_message'; text = $summary } | Out-Null
    exit 0
}

# Ask where to write the report, and wait for the answer of the user.
$job = (Invoke-Ops @{
    command = 'choose_path'
    mode = 'save'
    title = 'Save the point count report'
    file_name = 'point-count-report.csv'
    filters = @(@{ name = 'CSV table'; extensions = @('csv') })
}).job_id
do {
    Start-Sleep -Milliseconds 250
    $state = (Invoke-Ops @{ command = 'job'; id = $job }).job
} while ($state.state -eq 'running')
if ($state.state -ne 'complete') {
    Invoke-Ops @{ command = 'show_message'; text = 'No report was written' } | Out-Null
    exit 0
}

$lines = @('scan,points,remaining,selected')
foreach ($scan in $scans) {
    $name = ([string]$scan.path).Replace('"', '""')
    $lines += '"{0}",{1},{2},{3}' -f $name, $scan.points, $scan.remaining, $scan.selected
}
[System.IO.File]::WriteAllLines($state.path, [string[]]$lines, (New-Object System.Text.UTF8Encoding $false))
Invoke-Ops @{ command = 'show_message'; text = "$summary; report written to $($state.path)" } | Out-Null
