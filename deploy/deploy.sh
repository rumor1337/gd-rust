#!/usr/bin/env bash
# Sync the build context + GD assets to the Dockhand stack dir on a host.
# Usage: deploy/deploy.sh [host]   (default: staging). Does not start the stack.
set -euo pipefail
host=${1:-staging}
root=$(cd "$(dirname "$0")/.." && pwd)
res=${GD_RESOURCES:-$HOME/Documents/gd-clone/Resources}
stack=/opt/stacks/gdrust
# out/ is written by the container user: leave its files' ownership alone
ssh "$host" "sudo mkdir -p $stack/src $stack/assets/icons $stack/out && sudo chown 1001:1001 $stack && sudo chmod 777 $stack/out"
rsync -az --delete --rsync-path="sudo rsync" \
  --exclude target --exclude .git --exclude 'bridge/node_modules' --exclude analysis --exclude assets \
  "$root/" "$host:$stack/src/"
# only what the renderer/effects open (~31 MB of 300): sheets, backgrounds,
# ground tiles, font, particle + robot/spider animation plists, death/complete sfx
(cd "$res" && rsync -az --rsync-path="sudo rsync" --relative \
  GJ_GameSheet*-uhd.p* GJ_GameSheetIcons-hd.* PixelSheet_01-uhd.p* GJ_ParticleSheet-uhd.p* FireSheet_01-uhd.p* game_bg_*-uhd.png groundSquare_*_001-uhd.png \
  bigFont-uhd.* gjFont*-uhd.* *Effect*.plist firework.plist {Robot,Spider}_AnimDesc.plist explode_11.ogg endStart_02.ogg \
  icons/{bird,dart,player,player_ball,robot,ship,spider,swing}_01-uhd.* "$host:$stack/assets/")
rsync -az --rsync-path="sudo rsync" "$root/deploy/docker-compose.yml" "$host:$stack/docker-compose.yml"
ssh "$host" "sudo test -f $stack/.env || sudo install -m 600 -o 1001 -g 1001 $stack/src/deploy/env.example $stack/.env; sudo chown -R 1001:1001 $stack/src $stack/assets"
echo "synced to $host:$stack"
