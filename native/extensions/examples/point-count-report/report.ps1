# Point count report: an example extension of Open Pointcloud Studio.
#
# It reads the status of the window through the local API and shows the
# number of points of the open scans in the status bar. With --save, as the
# tile on the Export page of the File view passes, it asks where to write a
# CSV report with a line per scan. It speaks the language of the window,
# English or Dutch, and groups digits with a point as the window does.
#
# It runs in Windows PowerShell 5.1, which comes with Windows, and in
# PowerShell 7. The window starts it with:
#   OPS_API_PORT, OPS_API_TOKEN  the local API and the token of this run
#   OPS_EXTENSION_ID             the id of the extension
#   OPS_CONTEXT                  a JSON file with what the window shows

$ErrorActionPreference = 'Stop'

# An error ends the script with only its reason on standard error, which the
# status bar shows; where it happened goes to the log.
trap {
    Write-Output $_.InvocationInfo.PositionMessage
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}

$save = $args -contains '--save'
$url = "http://127.0.0.1:$($env:OPS_API_PORT)/exec"
$headers = @{ 'X-OPS-Token' = $env:OPS_API_TOKEN }
$invariant = [System.Globalization.CultureInfo]::InvariantCulture

# Send one command and return its answer. A refusal throws with the reason:
# the window refuses with ok false, the server itself (an undeclared or
# unknown command) with HTTP 400, 403 or 504 and only an error.
function Invoke-Ops([hashtable]$command) {
    $json = ConvertTo-Json -InputObject $command -Compress -Depth 8
    $body = [System.Text.Encoding]::UTF8.GetBytes($json)
    try {
        $answer = Invoke-RestMethod -Uri $url -Method Post -Headers $headers `
            -Body $body -ContentType 'application/json; charset=utf-8'
    } catch {
        $failure = $_
        $answer = $null
        try { $answer = ConvertFrom-Json $failure.ErrorDetails.Message } catch { }
        if (-not $answer.error) { throw $failure }
    }
    if (-not $answer.ok) {
        throw "$($command.command): $($answer.error)"
    }
    return $answer
}

# What the window showed when the run started, with its language.
$context = Get-Content -Raw -Encoding UTF8 -Path $env:OPS_CONTEXT | ConvertFrom-Json
Write-Output "Started from: $($context.entry); active scan: $($context.active_scan.path)"

$texts = @{
    en = @{
        reading = 'Reading the status'
        counting = 'Counting points'
        scan = 'scan'
        scans = 'scans'
        summary = '{0} {1}, {2} points, {3} selected'
        title = 'Save the point count report'
        table = 'CSV table'
        nothing = 'No report was written'
        written = '{0}; report written to {1}'
    }
    nl = @{
        reading = 'Status lezen'
        counting = 'Punten tellen'
        scan = 'scan'
        scans = 'scans'
        summary = '{0} {1}, {2} punten, {3} geselecteerd'
        title = 'Puntentellingrapport opslaan'
        table = 'CSV-tabel'
        nothing = 'Er is geen rapport geschreven'
        written = '{0}; rapport opgeslagen in {1}'
    }
}
$language = [string]$context.application.language
$text = if ($texts.ContainsKey($language)) { $texts[$language] } else { $texts['en'] }

# A count with its digits grouped by a point, as the window shows counts.
function Format-Count([int64]$count) {
    return $count.ToString('N0', $invariant).Replace(',', '.')
}

Invoke-Ops @{ command = 'report_progress'; percent = 10; text = $text.reading } | Out-Null
$status = (Invoke-Ops @{ command = 'status' }).result
$scans = @($status.clouds | Where-Object { $null -ne $_ })
$remaining = [int64]0
$selected = [int64]0
foreach ($scan in $scans) {
    $remaining += [int64]$scan.remaining
    $selected += [int64]$scan.selected
}
Invoke-Ops @{ command = 'report_progress'; percent = 60; text = $text.counting } | Out-Null

$noun = if ($scans.Count -eq 1) { $text.scan } else { $text.scans }
$summary = $text.summary -f $scans.Count, $noun, (Format-Count $remaining), (Format-Count $selected)
Write-Output $summary

if (-not $save) {
    Invoke-Ops @{ command = 'show_message'; text = $summary } | Out-Null
    exit 0
}

# Ask where to write the report, and wait for the answer of the user.
$job = (Invoke-Ops @{
    command = 'choose_path'
    mode = 'save'
    title = $text.title
    file_name = 'point-count-report.csv'
    filters = @(@{ name = $text.table; extensions = @('csv') })
}).job_id
do {
    Start-Sleep -Milliseconds 250
    $state = (Invoke-Ops @{ command = 'job'; id = $job }).job
} while ($state.state -eq 'running')
if ($state.state -ne 'complete') {
    Invoke-Ops @{ command = 'show_message'; text = $text.nothing } | Out-Null
    exit 0
}

$lines = @('scan,points,remaining,selected')
foreach ($scan in $scans) {
    $name = ([string]$scan.path).Replace('"', '""')
    $lines += '"{0}",{1},{2},{3}' -f $name, $scan.points, $scan.remaining, $scan.selected
}
[System.IO.File]::WriteAllLines($state.path, [string[]]$lines, (New-Object System.Text.UTF8Encoding $false))
Invoke-Ops @{ command = 'show_message'; text = ($text.written -f $summary, $state.path) } | Out-Null
