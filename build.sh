#!/usr/bin/env bash
set -euo pipefail

PLUGIN_ID="io.github.michelejohn.fps.sdPlugin"
DIST_DIR="dist/${PLUGIN_ID}"

cargo build --release

rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}/pi"

cp assets/manifest.json assets/icon.png assets/key.png "${DIST_DIR}/"
cp assets/pi/index.html "${DIST_DIR}/pi/"
cp target/release/sd_fps_plugin "${DIST_DIR}/"
cp target/release/libvk_layer_sd_fps.so "${DIST_DIR}/"

# Se --install è passato, aggiorna direttamente il plugin locale di OpenDeck
if [[ "${1:-}" == "--install" ]]; then
  TARGET="${HOME}/.config/opendeck/plugins/${PLUGIN_ID}"
  mkdir -p "${TARGET}/pi"
  rm -f "${TARGET}/sd_fps_plugin" "${TARGET}/libvk_layer_sd_fps.so"
  cp -r "${DIST_DIR}/"* "${TARGET}/"
  echo "Plugin aggiornato in ${TARGET}. Riavvia OpenDeck per applicare."
fi
