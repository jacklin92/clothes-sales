@echo off
chcp 65001 >nul
cd /d %~dp0
rem 開發用：不經 exe 直接啟動
.venv\Scripts\python.exe launcher.py
pause
