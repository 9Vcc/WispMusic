; WispMusic 每用户安装包脚本（NSIS 3.x），参考 SanBox 的 xtask + NSIS 链路。
; 生成：target/installer/WispMusic-<版本>-setup.exe
;
; 设计要点：
; - RequestExecutionLevel user + 装到 $LOCALAPPDATA\Programs：不需要管理员，也不弹 UAC。
; - exe 已静态链接 CRT（见 .cargo/config.toml），安装包里不需要 VC++ 运行时。
; - 应用数据（设置、插件、缓存、下载）在 $INSTDIR\wisp；卸载时保留，不随程序删除。
; - 应用窗口标题会随播放歌曲变化，不能用 FindWindow 判断；改为独占打开 exe 判断是否在运行。

Unicode true

!include "MUI2.nsh"
!include "FileFunc.nsh"

!define APP_NAME       "WispMusic"
!define APP_EXE        "wisp.exe"
!define APP_PUBLISHER  "9Vcc"
!define UNINST_KEY     "Software\Microsoft\Windows\CurrentVersion\Uninstall\WispMusic"

; 上一代 Tauri 版的默认每用户安装目录 / 可执行文件 / 卸载登记。
; Tauri 的主程序名来自 Cargo package（wisp-music-react-tauri），不是产品名；
; 安装位置可能写在 identifier 键，也可能写在产品键 WispMusic 里。
!define OLD_INSTALL_DIR "$LOCALAPPDATA\WispMusic"
!define OLD_EXE         "wisp-music-react-tauri.exe"
!define OLD_UNINST_KEY  "Software\Microsoft\Windows\CurrentVersion\Uninstall\com.isen.wisp-music"
!define OLD_APP_DATA    "$LOCALAPPDATA\com.isen.wisp-music"

; 兜底版本号：正常走 `cargo build-installer` 时由 xtask 用 -DAPP_VERSION=<Cargo.toml 的 version>
; 覆盖；这里只在直接手跑 makensis 时起作用。
!ifndef APP_VERSION
    !define APP_VERSION "0.2.4"
!endif
!ifndef APP_VERSION4
    !define APP_VERSION4 "${APP_VERSION}.0"
!endif
!ifndef ICON_FILE
    !define ICON_FILE "..\src\assets\windows\wisp.ico"
!endif

!ifndef STAGE_DIR
    !error "STAGE_DIR is not defined"
!endif
!ifndef OUT_FILE
    !define OUT_FILE "..\target\installer\${APP_NAME}-${APP_VERSION}-setup.exe"
!endif

Name "${APP_NAME} ${APP_VERSION}"
OutFile "${OUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
InstallDirRegKey HKCU "${UNINST_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 64
VIProductVersion "${APP_VERSION4}"
VIAddVersionKey /LANG=2052 "ProductName"     "${APP_NAME}"
VIAddVersionKey /LANG=2052 "FileDescription" "${APP_NAME} 安装程序"
VIAddVersionKey /LANG=2052 "FileVersion"     "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "ProductVersion"  "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "LegalCopyright"  "© ${APP_PUBLISHER}"
VIAddVersionKey /LANG=2052 "CompanyName"     "${APP_PUBLISHER}"

!define MUI_ICON   "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "启动 ${APP_NAME}"
!define MUI_FINISHPAGE_LINK "GitHub 项目主页"
!define MUI_FINISHPAGE_LINK_LOCATION "https://github.com/9Vcc/WispMusic"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"

; 正在运行则拒绝安装：exe 被占用会装一半失败。独占打开失败即可判定（不需要按窗口标题）。
; 应用内更新器用 /S 发起安装后自己就退出，这里最多等 30 秒让 exe 释放再动手；
; 等不到就放弃本次安装——下次启动会重新检查更新，不会留下半装状态。
Function .onInit
    SetShellVarContext current
    IfSilent silent_wait
    IfFileExists "$INSTDIR\${APP_EXE}" 0 done
    ClearErrors
    FileOpen $0 "$INSTDIR\${APP_EXE}" a
    IfErrors running
    FileClose $0
    Goto done
running:
    MessageBox MB_ICONEXCLAMATION|MB_OK "检测到 ${APP_NAME} 正在运行（或安装目录不可写），请退出后再安装。"
    Abort
silent_wait:
    StrCpy $1 0
silent_loop:
    IfFileExists "$INSTDIR\${APP_EXE}" 0 done
    ClearErrors
    FileOpen $0 "$INSTDIR\${APP_EXE}" a
    IfErrors silent_busy
    FileClose $0
    Goto done
silent_busy:
    Sleep 500
    IntOp $1 $1 + 1
    IntCmp $1 60 silent_abort silent_loop silent_loop
silent_abort:
    Abort
done:
FunctionEnd

; 静默安装由应用内更新器发起，装完把新版本拉起来；手动安装保持普通安装向导。
Function .onInstSuccess
    IfSilent silent_start
    Goto done
silent_start:
    Exec '"$INSTDIR\${APP_EXE}"'
done:
FunctionEnd

Function un.onInit
    SetShellVarContext current
    IfFileExists "$INSTDIR\${APP_EXE}" 0 done
    ClearErrors
    FileOpen $0 "$INSTDIR\${APP_EXE}" a
    IfErrors running
    FileClose $0
    Goto done
running:
    MessageBox MB_ICONEXCLAMATION|MB_OK "检测到 ${APP_NAME} 正在运行（或安装目录不可写），请退出后再卸载。"
    Abort
done:
FunctionEnd

; 从 Tauri 版升级时接管旧版程序目录：等旧进程退出（静默安装最多等 30 秒），把旧日志
; 搬进新版数据目录留档，再删旧程序、快捷方式与卸载登记。
; 旧版用户数据（设置、曲库、下载记录、封面、登录与 WebView2 数据）不迁移也不删除：
; 格式与 GPUI 不兼容，原样备份比误删安全。
; 任何一步失败都不阻断新版本安装，最坏只是留下旧目录。
Function RemoveOldTauri
    ; 旧版可能注册在 identifier 键，也可能注册在产品键 WispMusic 下；后者在本次安装写新值前仍指向旧目录。
    ReadRegStr $0 HKCU "${OLD_UNINST_KEY}" "InstallLocation"
    StrCmp $0 "" old_try_product
    IfFileExists "$0\${OLD_EXE}" 0 old_try_product
    StrCpy $2 $0
    Goto old_located
old_try_product:
    ReadRegStr $0 HKCU "${UNINST_KEY}" "InstallLocation"
    StrCmp $0 "" old_try_default
    IfFileExists "$0\${OLD_EXE}" 0 old_try_default
    StrCpy $2 $0
    Goto old_located
old_try_default:
    IfFileExists "${OLD_INSTALL_DIR}\${OLD_EXE}" 0 old_try_current
    StrCpy $2 "${OLD_INSTALL_DIR}"
    Goto old_located
old_try_current:
    ; 自定义安装目录没有留下旧卸载键时，旧文件可能就在本次选择的目录里。
    IfFileExists "$INSTDIR\${OLD_EXE}" 0 old_registry
    StrCpy $2 "$INSTDIR"
old_located:
    IfFileExists "$2\${OLD_EXE}" 0 old_registry
    IfSilent old_silent_wait old_prompt
old_prompt:
    ClearErrors
    FileOpen $0 "$2\${OLD_EXE}" a
    IfErrors old_running
    FileClose $0
    Goto old_program_files
old_running:
    MessageBox MB_RETRYCANCEL|MB_ICONQUESTION "检测到旧版 ${APP_NAME} 正在运行。$\r$\n请先退出旧版，点「重试」清理；点「取消」跳过旧版清理（新版仍会继续安装）。" IDRETRY old_prompt IDCANCEL old_done
old_silent_wait:
    StrCpy $3 0
old_silent_loop:
    ClearErrors
    FileOpen $0 "$2\${OLD_EXE}" a
    IfErrors old_silent_busy
    FileClose $0
    Goto old_program_files
old_silent_busy:
    Sleep 500
    IntOp $3 $3 + 1
    IntCmp $3 60 old_done old_silent_loop old_silent_loop
old_program_files:
    DetailPrint "移除旧版 Tauri 程序目录 $2"
    ; 旧日志从程序目录和旧应用数据各留一份；随后清掉可再生的旧缓存。
    ; 复制而不是移动，任何一步失败都不至于既丢日志又删目录。
    CreateDirectory "$INSTDIR\wisp\logs-tauri\install"
    CopyFiles /SILENT "$2\logs\*.*" "$INSTDIR\wisp\logs-tauri\install"
    CreateDirectory "$INSTDIR\wisp\logs-tauri\appdata"
    CopyFiles /SILENT "${OLD_APP_DATA}\logs\*.*" "$INSTDIR\wisp\logs-tauri\appdata"
    RMDir /r "${OLD_APP_DATA}\logs"
    RMDir /r "${OLD_APP_DATA}\audio_cache"
    RMDir /r "${OLD_APP_DATA}\native_audio"
    RMDir /r "${OLD_APP_DATA}\unm-runtime"
    ; 旧版和新版可能共用目录；只要看到 GPUI 的 exe 或 wisp 数据，就只能删 Tauri 文件，
    ; 绝不能整目录删除，否则会带走新版程序或用户数据。
    StrCmp "$2" "$INSTDIR" old_shared_dir
    IfFileExists "$2\${APP_EXE}" old_shared_dir
    IfFileExists "$2\wisp\settings.json" old_shared_dir
    IfFileExists "$2\wisp\state.sqlite" old_shared_dir
    Goto old_remove_dir
old_shared_dir:
    Delete "$2\${OLD_EXE}"
    RMDir /r "$2\icons"
    RMDir /r "$2\logs"
    RMDir /r "$2\缓存"
    RMDir /r "$2\cache"
    Goto old_registry
old_remove_dir:
    RMDir /r "$2"
    ; 旧版若装到了 Program Files，受限权限删不掉；保留给用户，不阻断安装。
    IfFileExists "$PROGRAMFILES\WispMusic\${OLD_EXE}" 0 old_registry
    RMDir /r "$PROGRAMFILES\WispMusic"
old_registry:
    DeleteRegKey HKCU "${OLD_UNINST_KEY}"
    ; Tauri 的卸载快捷方式名与 GPUI 不同，删旧名避免开始菜单留下死链接。
    Delete "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk"
    Delete "$SMPROGRAMS\${APP_NAME}\Uninstall.lnk"
old_done:
FunctionEnd

Section "安装 ${APP_NAME}"
    Call RemoveOldTauri
    SetOutPath "$INSTDIR"
    SetOverwrite on
    ; 安装包不再携带 README/LICENSE/THIRD-PARTY；升级时清掉旧版本留下的副本。
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\THIRD-PARTY.md"
    Delete "$INSTDIR\README.md"
    File "${STAGE_DIR}\${APP_EXE}"

    WriteUninstaller "$INSTDIR\Uninstall.exe"

    CreateDirectory "$SMPROGRAMS\${APP_NAME}"
    CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
    CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
    CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"

    WriteRegStr   HKCU "${UNINST_KEY}" "DisplayName"     "${APP_NAME}"
    WriteRegStr   HKCU "${UNINST_KEY}" "DisplayVersion"  "${APP_VERSION}"
    WriteRegStr   HKCU "${UNINST_KEY}" "Publisher"       "${APP_PUBLISHER}"
    WriteRegStr   HKCU "${UNINST_KEY}" "DisplayIcon"     "$INSTDIR\${APP_EXE}"
    WriteRegStr   HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr   HKCU "${UNINST_KEY}" "URLInfoAbout"    "https://github.com/9Vcc/WispMusic"
    WriteRegStr   HKCU "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\Uninstall.exe$\""
    WriteRegStr   HKCU "${UNINST_KEY}" "QuietUninstallString" "$\"$INSTDIR\Uninstall.exe$\" /S"
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    IntFmt $0 "0x%08X" $0
    WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Uninstall"
    Delete "$INSTDIR\${APP_EXE}"
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\THIRD-PARTY.md"
    Delete "$INSTDIR\README.md"
    Delete "$INSTDIR\Uninstall.exe"

    Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
    Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
    RMDir  "$SMPROGRAMS\${APP_NAME}"
    Delete "$DESKTOP\${APP_NAME}.lnk"

    DeleteRegKey HKCU "${UNINST_KEY}"

    ; $INSTDIR\wisp（设置、插件、缓存、下载）是用户数据，保留；只删空目录。
    RMDir "$INSTDIR"

    MessageBox MB_ICONINFORMATION|MB_OK "设置、插件、缓存和下载保留在：$\r$\n$INSTDIR\wisp$\r$\n$\r$\n需要彻底删除时请手动删除该文件夹。"
SectionEnd