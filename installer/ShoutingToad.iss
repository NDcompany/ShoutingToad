; Windows installer (Inno Setup 6). Built by the release workflow:
;   iscc /DAppVersion=1.0.0 /DBinary=path\to\shouting-toad.exe installer\ShoutingToad.iss
; Installs for the current user without administrator rights (the setup offers "all users"
; too), adds Start-menu and optional desktop shortcuts, opens .shoutingtoad crawl files, can
; put shouting-toad on PATH for the CLI, and registers an uninstaller in Apps & features.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Binary
  #define Binary "..\target\release\shouting-toad.exe"
#endif

[Setup]
AppId={{6F3B2C1E-8D4A-4E5B-9C7F-2A1D0E3B4C5D}
AppName=ShoutingToad SEO Spider
AppVersion={#AppVersion}
AppVerName=ShoutingToad SEO Spider {#AppVersion}
AppPublisher=N&D Co.
AppPublisherURL=https://github.com/NDcompany/ShoutingToad
AppSupportURL=https://github.com/NDcompany/ShoutingToad/issues
AppUpdatesURL=https://github.com/NDcompany/ShoutingToad/releases
DefaultDirName={autopf}\ShoutingToad
DefaultGroupName=ShoutingToad
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
ChangesAssociations=yes
ChangesEnvironment=yes
CloseApplications=yes
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\shouting-toad.exe
UninstallDisplayName=ShoutingToad SEO Spider
OutputDir=..\dist
OutputBaseFilename=ShoutingToad-Setup-{#AppVersion}-x64
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: desktopicon; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: addtopath; Description: "Add shouting-toad to &PATH (use the command line and CI mode from any terminal)"; GroupDescription: "Command line:"; Flags: unchecked
Name: assoc; Description: "Open .shoutingtoad crawl files with ShoutingToad"; GroupDescription: "Files:"

[Files]
Source: "{#Binary}"; DestDir: "{app}"; DestName: "shouting-toad.exe"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\ShoutingToad"; Filename: "{app}\shouting-toad.exe"; Comment: "ShoutingToad SEO Spider"
Name: "{autodesktop}\ShoutingToad"; Filename: "{app}\shouting-toad.exe"; Comment: "ShoutingToad SEO Spider"; Tasks: desktopicon

[Registry]
Root: HKA; Subkey: "Software\Classes\.shoutingtoad"; ValueType: string; ValueName: ""; ValueData: "ShoutingToad.Crawl"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\ShoutingToad.Crawl"; ValueType: string; ValueName: ""; ValueData: "ShoutingToad crawl"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\ShoutingToad.Crawl\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\shouting-toad.exe,0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\ShoutingToad.Crawl\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\shouting-toad.exe"" ""%1"""; Tasks: assoc

[Run]
Filename: "{app}\shouting-toad.exe"; Description: "Launch ShoutingToad"; Flags: nowait postinstall skipifsilent

[Code]
// PATH entry for the CLI: per-user PATH for a per-user install, system PATH for all users.
function EnvKey(var Root: Integer): String;
begin
  if IsAdminInstallMode then begin
    Root := HKEY_LOCAL_MACHINE;
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  end else begin
    Root := HKEY_CURRENT_USER;
    Result := 'Environment';
  end;
end;

function PathHas(Path, Dir: String): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') > 0;
end;

procedure AddToPath(Dir: String);
var Root: Integer; Key, Path: String;
begin
  Key := EnvKey(Root);
  if not RegQueryStringValue(Root, Key, 'Path', Path) then Path := '';
  if PathHas(Path, Dir) then exit;
  if (Path <> '') and (Copy(Path, Length(Path), 1) <> ';') then Path := Path + ';';
  RegWriteExpandStringValue(Root, Key, 'Path', Path + Dir);
end;

procedure RemoveFromPath(Dir: String);
var Root, P: Integer; Key, Path: String;
begin
  Key := EnvKey(Root);
  if not RegQueryStringValue(Root, Key, 'Path', Path) then exit;
  Path := ';' + Path + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path));
  if P = 0 then exit;
  Delete(Path, P, Length(Dir) + 1);
  Path := Copy(Path, 2, Length(Path) - 2);
  RegWriteExpandStringValue(Root, Key, 'Path', Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
