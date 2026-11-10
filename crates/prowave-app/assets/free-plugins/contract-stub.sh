#!/bin/sh
# STUB of the free plug-in installer. The real script replaces this file; Pro Wave only relies
# on the contract below (see crates/prowave-app/src/free_plugins.rs).
#
#   --list                       one line per plug-in, TAB-separated, no header:
#                                id kind name maker mode tag page_url description
#   --gui --only id1,id2,...     install those ids, printing "@@ ..." progress lines
#   --dry-run                    with the above: go through the steps, install nothing
#
# This stub fetches nothing and installs nothing. With --dry-run it walks through the chosen
# ids and reports them as done; without it every chosen plug-in fails with a reason, because
# there is no installer here yet.
set -u

catalog() {
  # id kind name maker mode tag page_url description
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    surge-xt synth 'Surge XT' 'Surge Synth Team' auto '' 'https://surge-synthesizer.github.io' 'Hybrid synthesizer with wavetable, FM and classic oscillators' \
    dexed synth 'Dexed' 'Digital Suburban' auto '' 'https://asb2m10.github.io/dexed/' 'Six-operator FM synth that reads DX7 patches' \
    vital synth 'Vital' 'Matt Tytel' manual 'Needs account' 'https://vital.audio' 'Spectral warping wavetable synth' \
    odin2 synth 'Odin 2' 'TheWaveWarden' auto '' 'https://thewavewarden.com/odin2' '24-voice semi-modular synth' \
    ob-xd synth 'OB-Xd' 'discoDSP' auto 'Free edition' 'https://www.discodsp.com/obxd/' 'Analog polysynth modelled on the OB-X & OB-Xa' \
    valhalla-supermassive effect 'Valhalla Supermassive' 'Valhalla DSP' manual 'Needs account' 'https://valhalladsp.com/shop/reverb/valhalla-supermassive/' 'Huge reverbs and delays' \
    dragonfly-reverb effect 'Dragonfly Reverb' 'Michael Willis' auto '' 'https://michaelwillis.github.io/dragonfly-reverb/' 'Hall, room, plate and early-reflection reverbs' \
    airwindows effect 'Airwindows Consolidated' 'Airwindows' auto '' 'https://www.airwindows.com' 'Several hundred effects in one plug-in' \
    chowtape effect 'CHOW Tape Model' 'Chowdhury DSP' auto '' 'https://chowdsp.com/products.html' 'Physical model of a reel-to-reel tape machine' \
    stub-broken effect 'Stub: fails "always" <é>' 'Nobody & Co.' auto '' 'https://example.invalid/a?b=c&d=e' 'Fails in every run; here to test the failure path'
}

mode=""
only=""
dry=0
while [ $# -gt 0 ]; do
  case "$1" in
    --list) mode=list ;;
    --gui) ;;
    --dry-run) dry=1 ;;
    --only)
      shift
      [ $# -gt 0 ] || { echo "--only needs a list of ids" >&2; exit 2; }
      only=$1
      mode=install
      ;;
    --only=*) only=${1#--only=}; mode=install ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

if [ "$mode" = list ]; then
  catalog
  exit 0
fi
if [ "$mode" != install ] || [ -z "$only" ]; then
  echo "usage: $0 --list | [--gui] [--dry-run] --only id1,id2,..." >&2
  exit 2
fi

tab=$(printf '\t')
ok=0
fail=0
manual=0
old_ifs=$IFS
IFS=,
set -- $only
IFS=$old_ifs
for id in "$@"; do
  [ -n "$id" ] || continue
  row=$(catalog | awk -F "$tab" -v id="$id" '$1 == id')
  echo "@@ start $id"
  if [ -z "$row" ]; then
    echo "@@ fail $id unknown plug-in"
    fail=$((fail + 1))
    continue
  fi
  row_mode=$(printf '%s\n' "$row" | cut -f 5)
  url=$(printf '%s\n' "$row" | cut -f 7)
  if [ "$row_mode" = manual ]; then
    echo "@@ manual $id $url"
    manual=$((manual + 1))
  elif [ "$id" = stub-broken ]; then
    echo "stub: $id has nothing to download"
    echo "@@ fail $id download failed (stub)"
    fail=$((fail + 1))
  elif [ "$dry" = 1 ]; then
    echo "stub: dry run, nothing fetched or installed for $id"
    echo "@@ ok $id"
    ok=$((ok + 1))
  else
    echo "@@ fail $id the installer is not included in this build yet"
    fail=$((fail + 1))
  fi
done
echo "@@ done ok=$ok fail=$fail manual=$manual"
exit 0
