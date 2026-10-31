#!/bin/bash
# One-shot: installs or updates CS2 into the shared base directory, then exits.
# SteamCMD keeps partial downloads under steamapps/, so an interrupted run resumes.
# Stop the workers before updating: they overlay this directory while running.
set -uo pipefail
MARKER="$STEAMAPPDIR/.botlab-install-ok"

mkdir -p "$STEAMAPPDIR"
rm -f "$MARKER"
for attempt in 1 2 3; do
    VALIDATE=""
    # A failed run may have left a half-applied update; validate repairs it with a delta.
    [[ $attempt -gt 1 || "${STEAMAPPVALIDATE:-0}" == 1 ]] && VALIDATE="validate"
    bash "$STEAMCMDDIR/steamcmd.sh" +force_install_dir "$STEAMAPPDIR" \
        +@bClientTryRequestManifestWithoutCode 1 +login anonymous \
        +app_update "$STEAMAPPID" -beta "$STEAMAPPVERSION" $VALIDATE +quit
    rc=$?
    [[ $rc == 0 ]] && break
    echo "install.sh: SteamCMD failed (rc $rc), attempt $attempt" >&2
    df -h "$STEAMAPPDIR" >&2
done
[[ $rc == 0 ]] || exit $rc
test -x "$STEAMAPPDIR/game/cs2.sh" || { echo "install.sh: game/cs2.sh missing after update" >&2; exit 1; }

BUILD=$(grep -m1 '"buildid"' "$STEAMAPPDIR/steamapps/appmanifest_${STEAMAPPID}.acf" | tr -dc '0-9')
echo "${BUILD:-unknown}" > "$MARKER"
echo "install.sh: CS2 build ${BUILD:-unknown} ready in $STEAMAPPDIR"
