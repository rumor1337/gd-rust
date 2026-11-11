#!/bin/sh
# Songs for the 5 demons. NG pages block non-browsers; original CDN URLs
# recovered via Wayback CDX (newgrounds.com/audio/listen/<id>) and the
# live audio.ngfiles.com CDN still serves them. Level->NG id from gdbrowser.
set -e
mkdir -p audio
dl() { # level_id ng_id cdn_path
  curl -s -o "audio/$1.mp3" "https://audio.ngfiles.com/$3"
  file "audio/$1.mp3" | grep -q MPEG || { echo "FAIL $1"; exit 1; }
}
dl 149992717 482872   482000/482872_KzX---Stalemate.mp3
dl 148598469 462046   462000/462046_NK_Headrush.mp3
dl 127323087 1569886  1569000/1569886_Pathetic---Society-Remix.mp3
dl 119544028 489111   489000/489111_Nuke-Powder.mp3
dl 119550490 233860   233000/233860_Dawn.mp3
echo done
