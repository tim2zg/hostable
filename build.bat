@echo off
echo =========================================
echo   Hostable Linux Binary Builder
echo =========================================

echo Building Hostable Linux binary using Docker...
docker build -f backend/Dockerfile -t hostable-builder .
if %errorlevel% neq 0 exit /b %errorlevel%

echo Extracting binary...
docker create --name hostable-extract hostable-builder
docker cp hostable-extract:/usr/local/bin/hostable hostable-linux-amd64
docker rm hostable-extract

echo.
echo Build complete!
echo You can now transfer 'hostable-linux-amd64' and 'install.sh' to your Proxmox server and run 'bash install.sh'
