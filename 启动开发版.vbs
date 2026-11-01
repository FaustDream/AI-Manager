' AI Manager dev launcher: release exe + project data dir. ASCII only (WSH/ANSI rule).
' Double-click to start the app against the REAL project data (same as dev data),
' because the release build by default uses a data/ folder next to the exe.
Option Explicit

Dim fso, shell, root, exe
Set fso = CreateObject("Scripting.FileSystemObject")
Set shell = CreateObject("WScript.Shell")
root = fso.GetParentFolderName(WScript.ScriptFullName)
exe = root & "\src-tauri\target\release\ai-manager.exe"

If Not fso.FileExists(exe) Then
  MsgBox "release exe not found. Build first:" & vbCrLf & "  cargo build --release", 16, "AI Manager"
  WScript.Quit 1
End If

shell.Environment("PROCESS").Item("AIMANAGER_DATA_DIR") = root & "\data"
shell.CurrentDirectory = root
shell.Run """" & exe & """", 1, False
