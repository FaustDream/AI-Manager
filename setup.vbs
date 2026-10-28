' AI Manager - rebuild the runtime environment silently.
' Double-click: no console window. A GUI message box appears only on failure.
' Keep this file ASCII-only: Windows Script Host reads .vbs with the system
' ANSI code page, and UTF-8 Chinese comments break the parser.
Option Explicit

Dim fso, sh, root, uv, q, cmd, rc

Set fso = CreateObject("Scripting.FileSystemObject")
Set sh = CreateObject("WScript.Shell")

root = fso.GetParentFolderName(WScript.ScriptFullName)
sh.CurrentDirectory = root

uv = sh.ExpandEnvironmentStrings("%USERPROFILE%") & "\.local\bin\uv.exe"
If Not fso.FileExists(uv) Then uv = "uv"

q = Chr(34)
cmd = q & uv & q & " sync --python 3.12"

rc = sh.Run(cmd, 0, True)

If rc <> 0 Then
    MsgBox "Dependency sync failed (exit code " & rc & ")." & vbCrLf & vbCrLf & _
           "Run setup.bat in a terminal to see details.", _
           vbCritical, "AI Manager"
End If
