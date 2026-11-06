; MoodCut for Windows: installer (Inno Setup 6.3 or later).
;
; Build:   iscc /DAppVersion=1.0.0 installer\moodcut.iss      -> dist\MoodCut-Setup-x64.exe
; Silent:  MoodCut-Setup-x64.exe /VERYSILENT /SUPPRESSMSGBOXES /TASKS="desktopicon,addtopath,assoc,ffmpeg,connectai"
;          (add /ALLUSERS to install for every user, which asks for admin rights)
;
; Installs for the current user by default: no admin prompt, like a modern app.
; Optional steps: desktop shortcut, PATH (so AI apps and terminals can start
; moodcut), .mcut file association, a portable FFmpeg downloaded with a progress
; bar, and registering MoodCut with the AI apps found on the PC. The uninstaller
; reverses all of it.

#if VER < EncodeVer(6,3,0)
  #error The MoodCut installer needs Inno Setup 6.3 or later (it uses x64compatible and DownloadTemporaryFile)
#endif

#define AppName "MoodCut"
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#define AppPublisher "MoodCut"
#define AppURL "https://github.com/hamzarasheed309/MoodCut"
; moodcut.exe is the command line (terminals, scripts, AI apps); MoodCutApp.exe is the same
; program as a windowless Windows app: what the Start menu, the desktop and .mcut files open
#define AppExe "moodcut.exe"
#define GuiExe "MoodCutApp.exe"
#ifndef SourceExe
  #define SourceExe "..\target\release\moodcut.exe"
#endif
#ifndef SourceGuiExe
  #define SourceGuiExe "..\target\release\MoodCutApp.exe"
#endif

[Setup]
AppId={{8C3F6B2E-5B1D-4E8A-9F0C-6D2A1E7B4C93}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
AppComments=The AI-native video editor and motion graphics engine
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableWelcomePage=no
DisableReadyPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; curl and tar ship with Windows 10 1803 and later
MinVersion=10.0.17134
OutputDir=..\dist
OutputBaseFilename=MoodCut-Setup-x64
SetupIconFile=..\crates\moodcut\assets\moodcut.ico
UninstallDisplayIcon={app}\{#GuiExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
WizardImageFile=wizard.bmp,wizard-2x.bmp
WizardSmallImageFile=wizard-small.bmp,wizard-small-2x.bmp
Compression=lzma2/ultra64
SolidCompression=yes
ChangesEnvironment=yes
ChangesAssociations=yes
; a running editor, or an AI app's background moodcut (no window), must be closed to update the exe
CloseApplications=force
RestartApplications=no
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Messages]
WelcomeLabel2=This will install [name/ver]: a video editor and motion graphics engine that Claude, ChatGPT and other AI apps can drive.%n%nSetup can also download FFmpeg and connect your AI apps, so MoodCut is ready to use when it finishes.
FinishedLabel=MoodCut is ready. Open it from the Start menu, or ask your AI app to "use MoodCut".

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"
Name: "ffmpeg"; Description: "Download and install &FFmpeg (about 100 MB; needed to import, play and export video)"; GroupDescription: "Get ready to edit:"
Name: "connectai"; Description: "Connect the &AI apps on this PC (Claude Desktop, Claude Code, Cursor, VS Code, Windsurf, LM Studio...)"; GroupDescription: "Get ready to edit:"
Name: "addtopath"; Description: "Add MoodCut to &PATH (lets AI apps and terminals start it by name)"; GroupDescription: "Get ready to edit:"
Name: "assoc"; Description: "Open .&mcut project files with MoodCut"; GroupDescription: "Get ready to edit:"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceGuiExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#GuiExe}"; Comment: "The AI-native video editor"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#GuiExe}"; Comment: "The AI-native video editor"; Tasks: desktopicon

[Registry]
; Win+R "moodcut" works even without PATH
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExe}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.mcut"; ValueType: string; ValueName: ""; ValueData: "MoodCut.Project"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mcut\OpenWithProgids"; ValueType: string; ValueName: "MoodCut.Project"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\MoodCut.Project"; ValueType: string; ValueName: ""; ValueData: "MoodCut Project"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\MoodCut.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#GuiExe},0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\MoodCut.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#GuiExe}"" gui ""%1"""; Tasks: assoc

[Run]
Filename: "{app}\{#GuiExe}"; Description: "Open MoodCut now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; take MoodCut out of the AI apps' settings so they don't point at a removed program: this
; account's, and (all-users installs run elevated, maybe as another admin) every other
; profile MoodCut connected, noted in %ProgramData%\MoodCut\ai-profiles
Filename: "{app}\{#AppExe}"; Parameters: "uninstall-ai --all-profiles"; Flags: runhidden waituntilterminated; RunOnceId: "UninstallAI"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\ffmpeg"

[Code]
const
  FFmpegUrl1 = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip';
  FFmpegUrl2 = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip';
  WebView2Url = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';
  WebView2Key = 'SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  UserEnvKey = 'Environment';
  SystemEnvKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

{ ---------- PATH ---------- }

function EnvRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKEY_LOCAL_MACHINE else Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then Result := SystemEnvKey else Result := UserEnvKey;
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then Paths := '';
  if Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0 then exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then Paths := Paths + ';';
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths, S: String;
  P: Integer;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', Paths) then exit;
  S := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(S));
  if P = 0 then exit;
  Delete(S, P, Length(Dir) + 1);
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', Copy(S, 2, Length(S) - 2));
end;

{ ---------- WebView2 (the editor window) ---------- }

{ Windows 11 and up-to-date Windows 10 have it already; older PCs get Microsoft's small
  Evergreen installer. Without it MoodCut still opens, in an Edge/Chrome app window. }
function WebView2Installed: Boolean;
var
  V: String;
begin
  Result := False;
  if RegQueryStringValue(HKLM32, WebView2Key, 'pv', V) or RegQueryStringValue(HKCU32, WebView2Key, 'pv', V) then
    Result := (V <> '') and (V <> '0.0.0.0');
end;

function OnQuietProgress(const Url, FileName: String; const Progress, ProgressMax: Int64): Boolean;
begin
  Result := True;
end;

procedure InstallWebView2;
var
  Code: Integer;
begin
  if WebView2Installed then exit;
  WizardForm.StatusLabel.Caption := 'Installing the Microsoft WebView2 runtime...';
  try
    DownloadTemporaryFile(WebView2Url, 'webview2-setup.exe', '', @OnQuietProgress);
    if not Exec(ExpandConstant('{tmp}\webview2-setup.exe'), '/silent /install', '', SW_HIDE, ewWaitUntilTerminated, Code) then Code := -1;
    Log(Format('WebView2 setup -> %d', [Code]));
  except
    Log('WebView2 download failed: ' + GetExceptionMessage);
  end;
end;

{ ---------- FFmpeg ---------- }

function FFmpegInstalled: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\ffmpeg\bin\ffmpeg.exe'));
end;

function OnDownloadProgress(const Url, FileName: String; const Progress, ProgressMax: Int64): Boolean;
begin
  if ProgressMax > 0 then
  begin
    WizardForm.ProgressGauge.Max := 1000;
    WizardForm.ProgressGauge.Position := Integer(Progress * 1000 div ProgressMax);
    WizardForm.StatusLabel.Caption := Format('Downloading FFmpeg: %d of %d MB', [Integer(Progress div 1048576), Integer(ProgressMax div 1048576)]);
  end else
    WizardForm.StatusLabel.Caption := Format('Downloading FFmpeg: %d MB', [Integer(Progress div 1048576)]);
  Result := True;
end;

function RunMoodCut(Params, Status: String; AsUser: Boolean): Integer;
var
  Code: Integer;
  Ok: Boolean;
begin
  WizardForm.StatusLabel.Caption := Status;
  { AI app settings live in the signed-in user's profile: in an all-users install
    Setup runs elevated (maybe as another admin account), so run those steps as the
    user who started Setup }
  if AsUser and IsAdminInstallMode then
    Ok := ExecAsOriginalUser(ExpandConstant('{app}\{#AppExe}'), Params, ExpandConstant('{app}'), SW_HIDE, ewWaitUntilTerminated, Code)
  else
    Ok := Exec(ExpandConstant('{app}\{#AppExe}'), Params, ExpandConstant('{app}'), SW_HIDE, ewWaitUntilTerminated, Code);
  if not Ok then Code := -1;
  Log(Format('moodcut %s -> %d', [Params, Code]));
  Result := Code;
end;

procedure InstallFFmpeg;
var
  Zip: String;
  Ok: Boolean;
begin
  if FFmpegInstalled then exit;
  Zip := ExpandConstant('{tmp}\ffmpeg.zip');
  Ok := False;
  try
    DownloadTemporaryFile(FFmpegUrl1, 'ffmpeg.zip', '', @OnDownloadProgress);
    Ok := True;
  except
    Log('FFmpeg download failed: ' + GetExceptionMessage);
    try
      DownloadTemporaryFile(FFmpegUrl2, 'ffmpeg.zip', '', @OnDownloadProgress);
      Ok := True;
    except
      Log('FFmpeg mirror failed: ' + GetExceptionMessage);
    end;
  end;
  WizardForm.ProgressGauge.Position := WizardForm.ProgressGauge.Max;
  if Ok then
    Ok := RunMoodCut('setup --from "' + Zip + '" --dest "' + ExpandConstant('{app}\ffmpeg\bin') + '"', 'Unpacking FFmpeg...', False) = 0;
  if not Ok then
    SuppressibleMsgBox('FFmpeg could not be downloaded right now.' + #13#10#13#10 + 'MoodCut will offer a one-click install the first time it opens.', mbInformation, MB_OK, IDOK);
end;

{ ---------- steps ---------- }

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    InstallWebView2;
    if WizardIsTaskSelected('addtopath') then AddToPath(ExpandConstant('{app}'));
    if WizardIsTaskSelected('ffmpeg') then InstallFFmpeg;
    if WizardIsTaskSelected('connectai') then RunMoodCut('install-ai --quiet', 'Connecting your AI apps...', True);
  end;
end;

{ Settings and re-creatable files only. Kept: autosave\, graphics\, sfx\, downloads\,
  fonts\ and models\, which can hold the user's own work or files they added. }
procedure RemoveSettingsAndCaches(Data: String);
begin
  DelTree(Data + '\cache', True, True, True);
  DelTree(Data + '\ffmpeg', True, True, True);
  DelTree(Data + '\ffmpeg-download', True, True, True);
  DelTree(Data + '\bin', True, True, True);
  DelTree(Data + '\webview2', True, True, True);
  DeleteFile(Data + '\window.json');
  DeleteFile(Data + '\settings.json');
  DeleteFile(Data + '\instance.json');
  DeleteFile(Data + '\remote-token');
  RemoveDir(Data); { only if nothing else is left }
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Data: String;
begin
  if CurUninstallStep = usUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
  if CurUninstallStep = usPostUninstall then
  begin
    Data := ExpandConstant('{userappdata}\MoodCut');
    if DirExists(Data) and not UninstallSilent then
      if MsgBox('Also remove MoodCut''s settings, caches and downloaded tools (' + Data + ')?' + #13#10#13#10 + 'Autosaves, generated graphics and sound effects are kept, and your projects and videos are not touched.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        RemoveSettingsAndCaches(Data);
  end;
end;
