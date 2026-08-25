# Registers the dev sweep as a Windows scheduled task: every 5 minutes, "start if not
# running" - the daemon's lockfile makes the extra starts no-ops, and a crash or reboot
# is healed within 5 minutes.
$action = New-ScheduledTaskAction -Execute "python" `
    -Argument '"F:\Solana sniper\analysis\dev_sweep.py"' `
    -WorkingDirectory "F:\Solana sniper"
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date) `
    -RepetitionInterval (New-TimeSpan -Minutes 5)
$settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit (New-TimeSpan -Days 3650) -StartWhenAvailable
Register-ScheduledTask -TaskName "pump-dev-sweep" -Action $action -Trigger $trigger `
    -Settings $settings -Description "keeps dev_history fresh for the sniper" -Force
Start-ScheduledTask -TaskName "pump-dev-sweep"
Write-Host "installed + started: pump-dev-sweep (every 5 min, start-if-not-running)"
