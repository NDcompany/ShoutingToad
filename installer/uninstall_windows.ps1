# Remove ShoutingToad installed by install_windows.ps1.
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ShoutingToad"
)

Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\ShoutingToad.lnk" -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\Desktop\ShoutingToad.lnk" -Force -ErrorAction SilentlyContinue
Remove-Item 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ShoutingToad' -Recurse -Force -ErrorAction SilentlyContinue

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath) {
    $kept = ($userPath -split ';') | Where-Object { $_ -and ($_ -ne $InstallDir) }
    [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
}

# The script may live inside the folder it deletes, so remove the folder last.
if (Test-Path $InstallDir) { Remove-Item $InstallDir -Recurse -Force -ErrorAction SilentlyContinue }
Write-Host 'ShoutingToad has been removed.' -ForegroundColor Green
