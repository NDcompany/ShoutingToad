# ShoutingToad Windows Uninstaller Script
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ShoutingToad"
)

Write-Host "Uninstalling ShoutingToad..." -ForegroundColor Yellow

# 1. Remove Shortcuts
Remove-Item "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\ShoutingToad" -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item "$env:USERPROFILE\Desktop\ShoutingToad.lnk" -Force -ErrorAction SilentlyContinue

# 2. Remove Registry Entry
Remove-Item "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ShoutingToad" -Recurse -Force -ErrorAction SilentlyContinue

# 3. Remove Installed Files
if (Test-Path $InstallDir) {
    Remove-Item $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "✓ ShoutingToad has been completely removed." -ForegroundColor Green
