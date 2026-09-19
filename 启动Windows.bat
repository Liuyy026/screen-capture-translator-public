@echo off
setlocal EnableExtensions DisableDelayedExpansion
chcp 65001 >nul
set "PROJECT=%~dp0"
if "%PROJECT:~-1%"=="\" set "PROJECT=%PROJECT:~0,-1%"
set "LOGDIR=%PROJECT%\.runtime"
if not exist "%LOGDIR%" mkdir "%LOGDIR%" >nul 2>&1
set "LOG=%LOGDIR%\windows-launch.log"
>"%LOG%" echo [%date% %time%] launcher started: "%PROJECT%"
set "PYTHONHOME="
set "PYTHONPATH="
set "PYTHONUTF8=1"
set "PYTHONIOENCODING=utf-8"
set "CHECK_ONLY="
if /i "%1"=="--check" set "CHECK_ONLY=1"
set "PYTHON="
call :check_python "%PROJECT%\.venv-cuda\Scripts\python.exe"
if not defined PYTHON call :check_python "%PROJECT%\.venv\Scripts\python.exe"
if not defined PYTHON goto no_python
>>"%LOG%" echo selected Python: "%PYTHON%"
if defined CHECK_ONLY exit /b 0
set "OLLAMA=%LOCALAPPDATA%\Programs\Ollama\ollama.exe"
if not exist "%OLLAMA%" (
  set "OLLAMA="
  where ollama >nul 2>&1
  if not errorlevel 1 for /f "delims=" %%I in ('where ollama') do if not defined OLLAMA set "OLLAMA=%%I"
)
set "OLLAMA_HOST=127.0.0.1:11439"
set "OLLAMA_NO_CLOUD=1"
set "OLLAMA_MAX_LOADED_MODELS=1"
set "OLLAMA_NUM_PARALLEL=1"
rem KOMA_OLLAMA_MODELS can point to a shared Ollama store. The project path
rem remains the default so an existing junction works without extra settings.
set "MODEL_ROOT=%KOMA_OLLAMA_MODELS%"
if not defined MODEL_ROOT set "MODEL_ROOT=%OLLAMA_MODELS%"
if not defined MODEL_ROOT set "MODEL_ROOT=%PROJECT%\.models\ollama"
if exist "%MODEL_ROOT%\manifests\" (
  set "OLLAMA_MODELS=%MODEL_ROOT%"
  >>"%LOG%" echo Ollama model store: "%OLLAMA_MODELS%"
) else (
  >>"%LOG%" echo Ollama model store is missing manifests: "%MODEL_ROOT%"
)
set "LISTENING="
for /f "usebackq delims=" %%I in (`powershell.exe -NoProfile -Command "$x=Get-NetTCPConnection -LocalPort 11439 -State Listen -ErrorAction SilentlyContinue; if($x){'yes'}"`) do set "LISTENING=%%I"
if not defined LISTENING if defined OLLAMA (
  >>"%LOG%" echo starting Ollama: "%OLLAMA%"
  powershell.exe -NoLogo -NoProfile -Command "Start-Process -FilePath $env:OLLAMA -ArgumentList 'serve' -WindowStyle Hidden" >>"%LOG%" 2>&1
)
>>"%LOG%" echo launching windows_app.py
cd /d "%PROJECT%"
"%PYTHON%" -X utf8 "%PROJECT%\windows_app.py" >>"%LOG%" 2>&1
set "EXITCODE=%ERRORLEVEL%"
>>"%LOG%" echo application exited with code %EXITCODE%
if not "%EXITCODE%"=="0" (
  type "%LOG%"
  echo Koma Manga Translator failed to start. See "%LOG%"
  pause
)
exit /b %EXITCODE%
:no_python
>>"%LOG%" echo No working project Python environment found.
type "%LOG%"
echo The checks above distinguish Python runtime failures from library import failures.
echo Details: "%LOG%"
if not defined CHECK_ONLY pause
exit /b 1

:check_python
>>"%LOG%" echo Checking "%~1"
if not exist "%~1" (
  >>"%LOG%" echo Interpreter file does not exist; trying next environment.
  exit /b 0
)
"%~1" -I -X utf8 -c "import sys; print('Python:', sys.executable); print('Base Python:', sys._base_executable)" >>"%LOG%" 2>&1
if errorlevel 1 (
  >>"%LOG%" echo Python runtime failed to start; trying next environment.
  exit /b 0
)
"%~1" -I -X utf8 -c "from PySide6.QtWidgets import QApplication; import PySide6; print('PySide6:', PySide6.__version__)" >>"%LOG%" 2>&1
if errorlevel 1 (
  >>"%LOG%" echo Python started, but PySide6 could not load; trying next environment.
  exit /b 0
)
set "PYTHON=%~1"
exit /b 0
