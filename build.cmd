@echo off
rem Gera o executavel e o instalador do CapturaX e copia os dois para a pasta "release".
cd /d "%~dp0"
title Capturax (build)

if not exist node_modules call npm install

call npx tauri build
if errorlevel 1 (
    echo.
    echo A build falhou. Veja o erro acima.
    pause
    exit /b 1
)

if not exist release mkdir release
copy /y "src-tauri\target\release\capturax-react.exe" "release\CapturaX.exe" >nul
copy /y "src-tauri\target\release\bundle\nsis\*-setup.exe" "release\" >nul

echo.
echo Pronto. Na pasta "release":
echo   - CapturaX_..._x64-setup.exe  (instalador: e o que voce manda para os outros)
echo   - CapturaX.exe                (o app sozinho, sem instalar)
start "" "%~dp0release"
pause
