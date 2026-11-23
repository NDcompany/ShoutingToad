# ShoutingToad Windows Installer Script
# Installs ShoutingToad, creates Start Menu shortcuts and registers clean uninstaller
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ShoutingToad"
)

Write-Host "Installing ShoutingToad by N&D Co. (ndcompany.in)..." -ForegroundColor Cyan

# 1. Create directory
if (!(Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# 2. Copy binary
$SourceExe = "$PSScriptRoot\..\target\debug\shouting-toad.exe"
if (!(Test-Path $SourceExe)) {
    $SourceExe = "$PSScriptRoot\..\shouting-toad.exe"
}

Copy-Item $SourceExe "$InstallDir\shouting-toad.exe" -Force

# 3. Create Start Menu Shortcut
$WshShell = New-Object -ComObject WScript.Shell
$StartMenuDir = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\ShoutingToad"
if (!(Test-Path $StartMenuDir)) {
    New-Item -ItemType Directory -Path $StartMenuDir -Force | Out-Null
}
$Shortcut = $WshShell.CreateShortcut("$StartMenuDir\ShoutingToad.lnk")
$Shortcut.TargetPath = "$InstallDir\shouting-toad.exe"
$Shortcut.Description = "ShoutingToad SEO Spider - Open Source Crawler by N&D Co."
$Shortcut.WorkingDirectory = $InstallDir
$Shortcut.Save()

# 4. Create Desktop Shortcut
$DesktopShortcut = $WshShell.CreateShortcut("$env:USERPROFILE\Desktop\ShoutingToad.lnk")
$DesktopShortcut.TargetPath = "$InstallDir\shouting-toad.exe"
$DesktopShortcut.Description = "ShoutingToad SEO Spider"
$DesktopShortcut.WorkingDirectory = $InstallDir
$DesktopShortcut.Save()

# 5. Register in Windows Registry for Add/Remove Programs
$UninstallRegKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\ShoutingToad"
New-Item -Path $UninstallRegKey -Force | Out-Null
Set-ItemProperty -Path $UninstallRegKey -Name "DisplayName" -Value "ShoutingToad SEO Spider (by N&D Co.)"
Set-ItemProperty -Path $UninstallRegKey -Name "DisplayVersion" -Value "1.0.0"
Set-ItemProperty -Path $UninstallRegKey -Name "Publisher" -Value "N&D Co. (ndcompany.in)"
Set-ItemProperty -Path $UninstallRegKey -Name "InstallLocation" -Value $InstallDir
Set-ItemProperty -Path $UninstallRegKey -Name "UninstallString" -Value "powershell.exe -ExecutionPolicy Bypass -File `"$PSScriptRoot\uninstall_windows.ps1`""
Set-ItemProperty -Path $UninstallRegKey -Name "URLInfoAbout" -Value "https://ndcompany.in"

Write-Host "✓ ShoutingToad successfully installed!" -ForegroundColor Green
Write-Host "Shortcuts added to Start Menu and Desktop." -ForegroundColor Green
