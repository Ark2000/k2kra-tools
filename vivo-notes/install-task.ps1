#Requires -Version 5.1
<#
.SYNOPSIS
    Register (or remove) the Windows scheduled task that syncs vivo notes.

.PARAMETER IntervalMinutes
    Minutes between runs. Default 15. The task also runs once at sign-in.

.PARAMETER Remove
    Remove the scheduled task instead of registering it.

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\install-task.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\install-task.ps1 -IntervalMinutes 60

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File .\install-task.ps1 -Remove
#>
[CmdletBinding()]
param(
    [int]$IntervalMinutes = 30,
    [switch]$Remove
)

$ErrorActionPreference = 'Stop'
$TaskName = 'vivo-notes-sync'
$ScriptPath = Join-Path $PSScriptRoot 'sync-notes.ps1'
$LauncherPath = Join-Path $PSScriptRoot 'sync-hidden.vbs'

if ($Remove) {
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($task) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Host "Removed scheduled task: $TaskName"
    } else {
        Write-Host "Scheduled task not found: $TaskName"
    }
    exit 0
}

if (-not (Test-Path -LiteralPath $ScriptPath)) {
    Write-Host "Cannot find sync-notes.ps1 next to this script."
    exit 1
}
if (-not (Test-Path -LiteralPath $LauncherPath)) {
    Write-Host "Cannot find sync-hidden.vbs next to this script."
    exit 1
}
if ($IntervalMinutes -lt 1) {
    Write-Host "IntervalMinutes must be at least 1."
    exit 1
}

# Launch through wscript (a GUI-subsystem host), so no console window is ever created.
# powershell.exe -WindowStyle Hidden still flashes a console for a moment.
$action = New-ScheduledTaskAction -Execute 'wscript.exe' `
    -Argument ('//B //Nologo "{0}"' -f $LauncherPath)

$triggerLogon = New-ScheduledTaskTrigger -AtLogOn
$triggerRepeat = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) `
    -RepetitionInterval (New-TimeSpan -Minutes $IntervalMinutes) `
    -RepetitionDuration (New-TimeSpan -Days 3650)

$settings = New-ScheduledTaskSettingsSet `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit (New-TimeSpan -Minutes 30)

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger @($triggerLogon, $triggerRepeat) `
    -Settings $settings `
    -Description 'Sync vivo Office Suite notes to local Markdown' `
    -Force | Out-Null

Write-Host "Registered scheduled task: $TaskName (every $IntervalMinutes minutes, and at sign-in)"
