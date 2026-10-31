#!/usr/bin/env bash
# Builds the plugin in the official .NET SDK container; output lands in plugin/BotLab/out/.
# Copy out/ to <server>/game/csgo/addons/counterstrikesharp/plugins/BotLab/.
set -euo pipefail
cd "$(dirname "$0")"

SDK_IMAGE="${SDK_IMAGE:-mcr.microsoft.com/dotnet/sdk:10.0}"
ENGINE="${CONTAINER_ENGINE:-$(command -v podman || command -v docker)}"

rm -rf out
"$ENGINE" run --rm \
  -v "$PWD":/src:Z -w /src \
  -e DOTNET_CLI_TELEMETRY_OPTOUT=1 -e DOTNET_NOLOGO=1 \
  "$SDK_IMAGE" \
  sh -c 'dotnet publish BotLab.csproj -c Release -o /src/out && rm -rf /src/bin /src/obj'

echo "built: $(pwd)/out"
ls -1 out
