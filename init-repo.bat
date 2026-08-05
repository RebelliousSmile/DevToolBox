# Git Repository Initialization Script
# Run this in the DevToolBox directory to initialize a local git repo

@echo off
echo Initializing DevToolBox repository...
git init
git add .
git commit -m "Initial commit: DevToolBox v0.1.0"

echo Creating remote repository on GitHub...
echo Please run these commands manually:
echo.
echo   cd C:\Users\fxgui\Documents\Projets\MyApps\DevToolBox
echo   git remote add origin https://github.com/RebelliousSmile/DevToolBox.git
echo   git branch -M main
echo   git push -u origin main
echo.
echo Then create the issue on GitHub with the roadmap below.
