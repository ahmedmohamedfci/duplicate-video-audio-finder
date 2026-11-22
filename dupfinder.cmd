@echo off
setlocal
REM Launcher — pass --data and --port for separate instances
REM Example:
REM   dupfinder.cmd --data D:\dup-runs\a
REM   dupfinder.cmd --data D:\dup-runs\b --port 0

set "ROOT=%~dp0"
cd /d "%ROOT%"

where node >nul 2>&1
if errorlevel 1 (
  echo Node.js not found on PATH. Install Node 22.5+ and retry.
  exit /b 1
)

node "%ROOT%src\server.js" %*
