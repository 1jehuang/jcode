@echo off
rem Build a disposable, byte-identical world and resume the frozen fixture in it.
rem Usage: resume_world.cmd <home-dir> <binary> [session-fixture]
setlocal
set "HOME_DIR=%~1"
set "BINARY=%~2"
set "FIXTURE=%~3"
if "%FIXTURE%"=="" set "FIXTURE=%~dp0..\fixture\master\session_mosquito_1794671874260_10867a3abffb9231.json"

if exist "%HOME_DIR%" rmdir /s /q "%HOME_DIR%"
mkdir "%HOME_DIR%\sessions" 2>nul
mkdir "%HOME_DIR%\cwd" 2>nul
copy /y "%FIXTURE%" "%HOME_DIR%\sessions\" >nul

set "JCODE_HOME=%HOME_DIR%"
set "KCODE_HOME=%HOME_DIR%"
set "JCODE_NO_UPDATE=1"
set "TERM=xterm-256color"
set "COLORTERM=truecolor"

"%BINARY%" --no-update --socket "%HOME_DIR%\iso.sock" -C "%HOME_DIR%\cwd" --resume mosquito
endlocal
