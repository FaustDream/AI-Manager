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
    WScript.Quit rc
End If

FixPythonw


' venv's pythonw.exe may be a console copy of python.exe (how uv builds envs),
' which pops a cmd window on double-click. Overwrite it with the GUI template
' shipped by the base interpreter. ASCII only: see the header note.
Sub FixPythonw()
    Dim cfg, stream, line, baseHome, tpl
    On Error Resume Next
    cfg = root & "\.venv\pyvenv.cfg"
    If Not fso.FileExists(cfg) Then Exit Sub

    Set stream = fso.OpenTextFile(cfg, 1)
    Do Until stream.AtEndOfStream
        line = stream.ReadLine
        If Left(line, 7) = "home = " Then baseHome = Trim(Mid(line, 8))
    Loop
    stream.Close

    If baseHome = "" Then Exit Sub
    tpl = baseHome & "\Lib\venv\scripts\nt\pythonw.exe"
    If fso.FileExists(tpl) Then
        fso.CopyFile tpl, root & "\.venv\Scripts\pythonw.exe", True
    End If
    Err.Clear
End Sub
