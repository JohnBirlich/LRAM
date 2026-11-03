
#if Ver < EncodeVer(6,6,0)
  #error Inno Setup 6.6 or newer is needed (WizardStyle dynamic windows11)
#endif
#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z, or build with scripts\build-installer.ps1
#endif
#ifndef AppNumVersion
  #define AppNumVersion AppVersion
#endif
#ifndef AppExe
  #define AppExe "..\src-tauri\target\release\lram.exe"
#endif

[Setup]
AppId={{E647C13A-F0B1-42FF-9C22-211876297675}
AppName=LRAM
AppVersion={#AppVersion}
AppPublisher=JohnBirlich
AppPublisherURL=https://github.com/JohnBirlich/RAM
VersionInfoVersion={#AppNumVersion}
DefaultDirName={autopf}\LRAM
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
WizardStyle=modern dynamic windows11
SetupIconFile=..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\LRAM.exe
UninstallDisplayName=LRAM
CloseApplications=force
OutputDir=..\dist-installer
OutputBaseFilename=LRAM-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; DestName: "LRAM.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\LRAM"; Filename: "{app}\LRAM.exe"
Name: "{autodesktop}\LRAM"; Filename: "{app}\LRAM.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\LRAM.exe"; Description: "{cm:LaunchProgram,LRAM}"; Flags: nowait postinstall skipifsilent; Check: WebView2Installed
Filename: "{app}\LRAM.exe"; Flags: nowait; Check: Relaunch

[Code]
const
  WebView2Key = 'Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  { Microsoft's Evergreen Bootstrapper: Setup only opens the link, it downloads nothing itself. }
  WebView2Download = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';
  { tauri-plugin-single-instance holds <identifier>-sim while LRAM runs. }
  RunningMutex = 'com.lram.manager-sim';

function HasWebView2(Root: Integer): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(Root, WebView2Key, 'pv', Version) and (Version <> '') and (Version <> '0.0.0.0');
end;

{ The machine-wide runtime (HKLM32 is WOW6432Node) or a per-user one. }
function WebView2Installed: Boolean;
begin
  Result := HasWebView2(HKLM32) or HasWebView2(HKCU);
end;

function InitializeSetup: Boolean;
var
  Code: Integer;
begin
  if not WebView2Installed and (SuppressibleMsgBox(
      'LRAM draws its window with the Microsoft Edge WebView2 Runtime, which is not installed.' #13#10#13#10 +
      'Setup will continue, but LRAM will not open until the runtime is installed.' #13#10#13#10 +
      'Download Microsoft''s WebView2 installer in your browser now?',
      mbConfirmation, MB_YESNO, IDNO) = IDYES) then
    ShellExecAsOriginalUser('open', WebView2Download, '', '', SW_SHOWNORMAL, ewNoWait, Code);
  Result := True;
end;

{ Setup was started by LRAM's self-updater (/RELAUNCH), silently. }
function Relaunch: Boolean;
var
  I: Integer;
begin
  Result := False;
  if WizardSilent and WebView2Installed then
    for I := 1 to ParamCount do
      if CompareText(ParamStr(I), '/RELAUNCH') = 0 then
        Result := True;
end;

{ A running LRAM holds its exe and database open: uninstalling then would leave both behind. }
function InitializeUninstall: Boolean;
begin
  Result := True;
  while Result and CheckForMutexes(RunningMutex) do
    Result := SuppressibleMsgBox('LRAM is running. Close it, then click OK.', mbError, MB_OKCANCEL, IDCANCEL) = IDOK;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  Data := ExpandConstant('{localappdata}\LRAM');
  if (CurUninstallStep <> usPostUninstall) or UninstallSilent or not DirExists(Data) then
    exit;
  { Keep is the first button, so the default: TaskDialogMsgBox has no default-button flag. }
  if TaskDialogMsgBox('Also delete your LRAM accounts and settings?',
      'They are in ' + Data + '. Your Roblox accounts themselves are not affected.',
      mbConfirmation, MB_YESNO, ['&Keep them' #13#10 'They are there again if you reinstall LRAM.',
      '&Delete them' #13#10 'Saved accounts, cookies, groups and settings. This cannot be undone.'], 0) = IDNO then begin
    DelTree(Data, True, True, True);
    { The window position tauri-plugin-window-state keeps. }
    DelTree(ExpandConstant('{userappdata}\com.lram.manager'), True, True, True);
  end;
end;
