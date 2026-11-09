@echo off
REM MAGI // release terminal — interactive build, CI payload fetch and adb deploy.
REM Requires: cargo (host). Optional: gh (CI payloads), adb (device), docker (local device build).
setlocal
title MAGI // release terminal
cd /d "%~dp0"

set "PAYLOAD=payload-latest"
set "DEV_BIN=target\aarch64-unknown-linux-gnu\release\magi"
set "SD=/mnt/sdcard"
set "ADB=adb"
if exist "%~dp0adb\adb.exe" set "ADB=%~dp0adb\adb.exe"

:menu
cls
echo.
echo   MAGI // RELEASE TERMINAL
echo   =======================
echo.
echo   1  Host build: fmt + tests + selftest
echo   2  Run desktop harness
echo   3  Fetch latest CI payload              (needs gh; or unzip into payload-latest\)
echo   4  Build device binary in docker         (needs docker)
echo   5  Deploy binary to console over adb     (atomic swap + reboot)
echo   6  Push system payload over adb           (keeps card data)
echo   7  Push a game folder to Games\
echo   8  Pull logs from console
echo   9  Pull screenshots from console
echo   V  Prepare release: bump version, changelog, commit, tag
echo   G  Publish release: push main + tag (CI publishes the release)
echo   P  Device probe (frontend --probe)
echo   S  adb shell
echo   R  Reboot console
echo   0  Exit
echo.
choice /c 123456789VGPSR0 /n /m "choose: "
if errorlevel 15 exit /b 0
if errorlevel 14 goto reboot
if errorlevel 13 goto shell
if errorlevel 12 goto probe
if errorlevel 11 goto publish_release
if errorlevel 10 goto prepare_release
if errorlevel 9 goto pull_shots
if errorlevel 8 goto pull_logs
if errorlevel 7 goto push_game
if errorlevel 6 goto push_payload
if errorlevel 5 goto deploy_bin
if errorlevel 4 goto build_docker
if errorlevel 3 goto fetch_ci
if errorlevel 2 goto run_harness
if errorlevel 1 goto host_build
goto menu

REM ---------------------------------------------------------------- checks

:need_adb
"%ADB%" version >nul 2>nul
if errorlevel 1 (
    echo.
    echo   adb not found. Either drop Android platform-tools into the adb\
    echo   folder beside this script or install them with
    echo     winget install Google.PlatformTools
    echo.
    pause
    exit /b 1
)
exit /b 0

:need_gh
where gh >nul 2>nul
if errorlevel 1 (
    echo.
    echo   gh not found. Install the GitHub CLI with
    echo     winget install GitHub.cli
    echo   then run: gh auth login
    echo.
    pause
    exit /b 1
)
exit /b 0

:need_docker
where docker >nul 2>nul
if errorlevel 1 (
    echo.
    echo   docker not found. Install Docker Desktop, or fetch the CI payload
    echo   instead (menu 3).
    echo.
    pause
    exit /b 1
)
exit /b 0

REM ---------------------------------------------------------------- build

:host_build
cargo fmt
if errorlevel 1 goto failed
cargo test --quiet
if errorlevel 1 goto failed
cargo run --quiet -- --selftest
if errorlevel 1 goto failed
echo.
echo   host build ok — frames in target\selftest
pause
goto menu

:run_harness
cargo run
pause
goto menu

:fetch_ci
call :need_gh
if errorlevel 1 goto menu
echo.
echo   payload panel:
echo     1  640x480
echo     2  720x480
choice /c 12 /n /m "choose: "
if errorlevel 2 set "PANEL=720"
if errorlevel 1 if not errorlevel 2 set "PANEL=640"
if not exist "%PAYLOAD%" mkdir "%PAYLOAD%"
set "RUN_ID="
set "RUN_STATE="
for /f "usebackq delims=" %%a in (`gh run list --workflow build --status success --limit 1 --jq .[0].databaseId`) do set "RUN_ID=%%a"
for /f "usebackq delims=" %%a in (`gh run list --workflow build --status success --limit 1 --jq .[0].conclusion`) do set "RUN_STATE=%%a"
if "%RUN_ID%"=="" (
    echo   no CI runs found.
    pause
    goto menu
)
echo   latest run %RUN_ID% (conclusion: %RUN_STATE%), %PANEL%
rd /s /q "%PAYLOAD%" 2>nul
mkdir "%PAYLOAD%"
gh run download %RUN_ID% --name magi-payload-%PANEL% --dir "%PAYLOAD%"
if errorlevel 1 (
    echo.
    echo   download failed. The run may have failed or this repo is private
    echo   and gh is not logged in. Run: gh auth login
    echo   You can also download the artifact in a browser and extract its
    echo   contents into %PAYLOAD%\.
    pause
    goto menu
)
echo.
echo   payload in %PAYLOAD%\ (System, Games, Config, BIOS, Overlays, Saves, Screenshots, Logs)
pause
goto menu

:build_docker
call :need_docker
if errorlevel 1 goto menu
echo   building the aarch64 binary in a bullseye container (first run downloads a lot)...
docker run --rm -v "%CD%:/w" -w /w debian:bullseye sh -c "printf '%s\n' 'deb http://archive.debian.org/debian bullseye main' 'deb http://archive.debian.org/debian-security bullseye-security main' > /etc/apt/sources.list && apt-get -o Acquire::Check-Valid-Until=false update && apt-get install -y --no-install-recommends curl build-essential cmake gcc-aarch64-linux-gnu g++-aarch64-linux-gnu binutils-aarch64-linux-gnu && curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable && . $HOME/.cargo/env && rustup target add aarch64-unknown-linux-gnu && cargo build --release --target aarch64-unknown-linux-gnu"
if errorlevel 1 goto failed
if not exist "%DEV_BIN%" goto failed
echo.
echo   device binary at %DEV_BIN%
pause
goto menu

REM ---------------------------------------------------------------- deploy

:resolve_payload
REM Sets PAYROOT to a folder whose System\frontend exists — accepting the
REM zip's own top-level folder when the archive was extracted with one.
set "PAYROOT="
if exist "%PAYLOAD%\System\frontend" set "PAYROOT=%PAYLOAD%"
if not defined PAYROOT (
    for /d %%d in ("%PAYLOAD%\*") do (
        if exist "%%~d\System\frontend" set "PAYROOT=%%~d"
    )
)
exit /b 0

:push_bootlogo
if exist "assets\boot\bootlogo-640x480.bmp" (
    "%ADB%" push "assets\boot\bootlogo-640x480.bmp" "%SD%/System/bootlogo-640x480.bmp.new"
    if errorlevel 1 exit /b 1
)
if exist "assets\boot\bootlogo-720x480.bmp" (
    "%ADB%" push "assets\boot\bootlogo-720x480.bmp" "%SD%/System/bootlogo-720x480.bmp.new"
    if errorlevel 1 exit /b 1
)
if exist "assets\boot\gehirn.png" (
    "%ADB%" push "assets\boot\gehirn.png" "%SD%/System/magi/assets/boot/gehirn.png.new"
    if errorlevel 1 exit /b 1
)
if exist "assets\boot\naoko.wav" (
    "%ADB%" push "assets\boot\naoko.wav" "%SD%/System/magi/assets/boot/naoko.wav.new"
    if errorlevel 1 exit /b 1
)
"%ADB%" shell "mv -f %SD%/System/bootlogo-640x480.bmp.new %SD%/System/bootlogo-640x480.bmp; mv -f %SD%/System/bootlogo-720x480.bmp.new %SD%/System/bootlogo-720x480.bmp; mv -f %SD%/System/magi/assets/boot/gehirn.png.new %SD%/System/magi/assets/boot/gehirn.png; mv -f %SD%/System/magi/assets/boot/naoko.wav.new %SD%/System/magi/assets/boot/naoko.wav; sync"
if errorlevel 1 exit /b 1
exit /b 0

:deploy_bin
call :need_adb
if errorlevel 1 goto menu
set "BIN="
if exist "%DEV_BIN%" set "BIN=%DEV_BIN%"
if not defined BIN call :resolve_payload
if not defined BIN if defined PAYROOT set "BIN=%PAYROOT%\System\frontend"
if not defined BIN (
    echo.
    echo   no device binary found. Build one with menu 4 or download the
    echo   magi-payload-PANEL artifact and extract its contents into %PAYLOAD%\
    echo   so that %PAYLOAD%\System\frontend exists.
    pause
    goto menu
)
echo   deploying %BIN%
"%ADB%" wait-for-device
if defined PAYROOT "%ADB%" push "%PAYROOT%\System\magi" %SD%/System/
if errorlevel 1 goto failed
call :push_bootlogo
if errorlevel 1 goto failed
"%ADB%" push "%BIN%" %SD%/System/frontend.new
if errorlevel 1 goto failed
"%ADB%" shell "mv -f %SD%/System/frontend.new %SD%/System/frontend && sync"
if errorlevel 1 goto failed
"%ADB%" reboot
echo.
echo   pushed and rebooting — the new build starts with the console.
pause
goto menu

:push_payload
call :need_adb
if errorlevel 1 goto menu
call :resolve_payload
if not defined PAYROOT (
    echo.
    echo   no payload found. Download a magi-payload-PANEL artifact from the CI run
    echo   and extract its contents into %PAYLOAD%\ so that
    echo   %PAYLOAD%\System\frontend exists. Then try again.
    pause
    goto menu
)
echo   refreshing system files from %PAYROOT%\ (games, saves, settings and BIOS are kept)
"%ADB%" wait-for-device
REM Config, BIOS, Saves and Screenshots are user data and are never pushed:
REM the local payload copy may hold defaults written by the desktop harness.
"%ADB%" push "%PAYROOT%\System\magi" %SD%/System/
if errorlevel 1 goto failed
call :push_bootlogo
if errorlevel 1 goto failed
if exist "%PAYROOT%\Overlays" "%ADB%" push "%PAYROOT%\Overlays" %SD%/
if errorlevel 1 goto failed
if exist "%PAYROOT%\Themes\_template.ini" "%ADB%" push "%PAYROOT%\Themes\_template.ini" %SD%/Themes/
if errorlevel 1 goto failed
"%ADB%" push "%PAYROOT%\System\frontend" %SD%/System/frontend.new
if errorlevel 1 goto failed
"%ADB%" shell "mv -f %SD%/System/frontend.new %SD%/System/frontend && sync"
"%ADB%" reboot
echo.
echo   payload pushed and rebooting.
pause
goto menu

:push_game
call :need_adb
if errorlevel 1 goto menu
set "GAMEDIR="
set "CONSOLE="
set /p "GAMEDIR=local folder with games: "
if not defined GAMEDIR goto menu
if not exist "%GAMEDIR%" (
    echo   %GAMEDIR% not found.
    pause
    goto menu
)
set /p "CONSOLE=console (GB GBC GBA NES SNES GENESIS PS1 DC NGPC PCE POKEMINI N64) [GBA]: "
if not defined CONSOLE set "CONSOLE=GBA"
"%ADB%" wait-for-device
"%ADB%" push "%GAMEDIR%" %SD%/Games/%CONSOLE%/
if errorlevel 1 goto failed
echo.
echo   copied into Games/%CONSOLE%/ — rescan happens on next boot.
pause
goto menu

REM ---------------------------------------------------------------- inspect

:pull_logs
call :need_adb
if errorlevel 1 goto menu
if not exist logs mkdir logs
"%ADB%" wait-for-device
for %%f in (magi.log magi-run.log magi-n64.log magi-n64-supervisor.log magi-n64-aplay.log) do "%ADB%" pull %SD%/Logs/%%f logs\ 2>nul
"%ADB%" pull %SD%/baseos-boot.log logs\ 2>nul
echo.
echo   logs in logs\ (missing files simply did not exist on the card)
pause
goto menu

:pull_shots
call :need_adb
if errorlevel 1 goto menu
"%ADB%" wait-for-device
"%ADB%" pull %SD%/Screenshots .\Screenshots\ 2>nul
if errorlevel 1 (
    echo   no screenshots on the card yet — press SELECT+MENU+R1 to take one.
    pause
    goto menu
)
echo.
echo   screenshots in .\Screenshots\
pause
goto menu

:probe
call :need_adb
if errorlevel 1 goto menu
"%ADB%" wait-for-device
echo   running the device probe (from /tmp so the live log is left alone)...
echo.
"%ADB%" shell "cd /tmp; /lib/ld-linux-aarch64.so.1 %SD%/System/frontend --probe"
echo.
pause
goto menu

:shell
call :need_adb
if errorlevel 1 goto menu
"%ADB%" wait-for-device
"%ADB%" shell
goto menu

:reboot
call :need_adb
if errorlevel 1 goto menu
"%ADB%" wait-for-device
"%ADB%" reboot
echo   rebooting.
pause
goto menu

REM ---------------------------------------------------------------- release

:prepare_release
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\gehirn\release.ps1" prepare
if errorlevel 1 goto failed
pause
goto menu

:publish_release
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0tools\gehirn\release.ps1" publish
if errorlevel 1 goto failed
pause
goto menu

:failed
echo.
echo   command failed (see output above).
pause
goto menu
