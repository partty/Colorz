; Inno Setup 6 script for the Colorz installer. Packs the tree that
; windows/bundle.sh staged; CI passes both defines:
;   ISCC.exe /DAppVersion=<version> /DStageDir=<stage dir> windows\colorz.iss
; Installs per user, without admin rights, into %LOCALAPPDATA%\Programs\Colorz.

#ifndef AppVersion
  #error Pass the version with /DAppVersion=<version>
#endif
#ifndef StageDir
  #error Pass the staged tree from windows/bundle.sh with /DStageDir=<dir>
#endif

[Setup]
; Identifies the app to Windows across versions: never change it.
AppId={{856CE34A-7395-413A-9655-E5DE86D21707}
AppName=Colorz
AppVersion={#AppVersion}
AppPublisher=Ogi Tsonev
AppPublisherURL=https://github.com/partty/Colorz
AppSupportURL=https://github.com/partty/Colorz/issues
PrivilegesRequired=lowest
DefaultDirName={autopf}\Colorz
DisableProgramGroupPage=yes
OutputBaseFilename=colorz-{#AppVersion}-setup-x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
SetupIconFile=colorz.ico
UninstallDisplayIcon={app}\bin\colorz.exe
UninstallDisplayName=Colorz
; [Code] ends a running Colorz itself, including one that only has a tray
; icon, which the Restart Manager would not close.
CloseApplications=no
Compression=lzma2
SolidCompression=yes

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
; Windows shows Colorz's toasts only for a shortcut with this AppUserModelID.
Name: "{autoprograms}\Colorz"; Filename: "{app}\bin\colorz.exe"; AppUserModelID: "io.github.partty.Colorz"

[Registry]
; Colorz writes its start-at-login value itself; uninstalling removes it.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Colorz"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\bin\colorz.exe"; Description: "Launch Colorz"; Flags: nowait postinstall skipifsilent

[Code]
// Ends any running colorz.exe, and the GLib session bus that Colorz started
// from AppDir\bin\gdbus.exe, so their files can be replaced or removed.
procedure EndColorz(AppDir: String);
var
  ResultCode: Integer;
  Dir: String;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM colorz.exe', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  // The bus outlives Colorz and keeps the DLLs in bin locked. Only a gdbus.exe
  // under AppDir is ended: other GLib apps may run their own. Dir goes into a
  // single-quoted PowerShell string, so its quotes are doubled (O'Brien).
  if AppDir <> '' then
  begin
    Dir := AddBackslash(AppDir);
    StringChangeEx(Dir, '''', '''''', True);
    Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' +
      'Get-Process gdbus -ErrorAction SilentlyContinue | ' +
      'Where-Object { $_.Path -and $_.Path.StartsWith(''' + Dir + ''', [StringComparison]::OrdinalIgnoreCase) } | ' +
      'Stop-Process -Force"',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  end;
  // Gives Windows time to release the ended processes' file handles.
  Sleep(500);
end;

// WizardDirValue is the chosen install dir; for an upgrade, the existing one.
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  EndColorz(WizardDirValue);
  Result := '';
end;

// After the user confirms the uninstall, before any file is removed.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    EndColorz(ExpandConstant('{app}'));
end;
