#!/bin/bash
# Worker start: no SteamCMD. The shared CS2 install is mounted as a throwaway overlay
# (podman `:O`), so everything written here is per-worker and discarded on stop.
set -euo pipefail
GAME="$STEAMAPPDIR/game/csgo"
MARKER="$STEAMAPPDIR/.botlab-install-ok"

until [[ -f "$MARKER" ]]; do
    echo "worker-entry: waiting for the shared CS2 install (run the cs2-install service)"
    sleep 30
done
echo "worker-entry: CS2 build $(cat "$MARKER")"

mkdir -p ~/.steam/sdk64
ln -sfT "$STEAMCMDDIR/linux64/steamclient.so" ~/.steam/sdk64/steamclient.so

mkdir -p "$GAME/addons" "$GAME/cfg" "$GAME/botlab_demos"
cp -a /botlab/addons/. "$GAME/addons/"
cp /botlab/cfg/server.cfg /botlab/cfg/botlab.cfg "$GAME/cfg/"
# Loose file next to the VPKs; whether CS2 prefers it over the packed one is a spike item.
cp /botlab/botprofile.db "$GAME/botprofile.db"

# Metamod must be the first search path in gameinfo.gi.
GI="$GAME/gameinfo.gi"
if ! grep -q "csgo/addons/metamod" "$GI"; then
    sed -i '/Game_LowViolence[[:space:]]*csgo_lv/a\\t\t\tGame\tcsgo/addons/metamod' "$GI"
fi
grep -q "csgo/addons/metamod" "$GI" || { echo "worker-entry: failed to patch gameinfo.gi" >&2; exit 1; }

cd "$STEAMAPPDIR/game"
exec ./cs2.sh -dedicated -ip 0.0.0.0 -port 27015 -console -usercon \
    -maxplayers "${CS2_MAXPLAYERS:-12}" \
    +game_alias competitive +mapgroup mg_active +map "${CS2_STARTMAP:-de_dust2}" \
    +rcon_password "$CS2_RCONPW" +sv_lan 1 +tv_port 27020
