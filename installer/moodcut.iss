; MoodCut for Windows: installer (Inno Setup 6.1 or later).
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

#define AppName "MoodCut"
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#define AppPublisher "MoodCut"
#define AppURL "https://github.com/hamzarasheed309/MoodCut"
#define AppExe "moodcut.exe"
#ifndef SourceExe
  #define SourceExe "..\target\release\moodcut.exe"
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
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
WizardImageFile=wizard.bmp,wizard-2x.bmp
WizardSmallImageFile=wizard-small.bmp,wizard-small-2x.bmp
Compression=lzma2/ultra64
SolidCompression=yes
ChangesEnvironment=yes
ChangesAssociations=yes
CloseApplications=yes
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
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
; started minimized so the console window that hosts the command line never flashes up
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "The AI-native video editor"; Flags: runminimized
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "The AI-native video editor"; Flags: runminimized; Tasks: desktopicon

[Registry]
; Win+R "moodcut" works even without PATH
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\App Paths\{#AppExe}"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.mcut"; ValueType: string; ValueName: ""; ValueData: "MoodCut.Project"; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\.mcut\OpenWithProgids"; ValueType: string; ValueName: "MoodCut.Project"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\MoodCut.Project"; ValueType: string; ValueName: ""; ValueData: "MoodCut Project"; Flags: uninsdeletekey; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\MoodCut.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: assoc
Root: HKA; Subkey: "Software\Classes\MoodCut.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" gui ""%1"""; Tasks: assoc

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open MoodCut now"; Flags: nowait postinstall skipifsilent runminimized

[UninstallRun]
; take MoodCut out of the AI apps' settings so they don't point at a removed program
Filename: "{app}\{#AppExe}"; Parameters: "uninstall-ai"; Flags: runhidden waituntilterminated; RunOnceId: "UninstallAI"

[UninstallDelete]
Type: filesandordirs; Name: "{app}\ffmpeg"

[Code]
const
  FFmpegUrl1 = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip';
  FFmpegUrl2 = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip';
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

function RunMoodCut(Params, Status: String): Integer;
var
  Code: Integer;
begin
  WizardForm.StatusLabel.Caption := Status;
  if not Exec(ExpandConstant('{app}\{#AppExe}'), Params, ExpandConstant('{app}'), SW_HIDE, ewWaitUntilTerminated, Code) then
    Code := -1;
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
    Ok := RunMoodCut('setup --from "' + Zip + '" --dest "' + ExpandConstant('{app}\ffmpeg\bin') + '"', 'Unpacking FFmpeg...') = 0;
  if not Ok then
    SuppressibleMsgBox('FFmpeg could not be downloaded right now.' + #13#10#13#10 + 'MoodCut will offer a one-click install the first time it opens.', mbInformation, MB_OK, IDOK);
end;

{ ---------- steps ---------- }

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
  begin
    if WizardIsTaskSelected('addtopath') then AddToPath(ExpandConstant('{app}'));
    if WizardIsTaskSelected('ffmpeg') then InstallFFmpeg;
    if WizardIsTaskSelected('connectai') then RunMoodCut('install-ai --quiet', 'Connecting your AI apps...');
  end;
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
      if MsgBox('Also remove MoodCut''s settings and caches (' + Data + ')?' + #13#10#13#10 + 'Your projects and videos are not touched.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(Data, True, True, True);
  end;
end;
