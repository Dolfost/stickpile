#!/usr/bin/env bash
# Download the stickpile AI models into the layout of docs/ai.md and verify
# their sha256. Files that are already present with the right hash are kept.
#
#   scripts/fetch-models.sh [--english | --multilingual | --all] <model_dir>
#
#   --english       CLIP ViT-B/32 image + English text tower   (~608 MB, default)
#   --multilingual  CLIP ViT-B/32 image + multilingual text    (~490 MB)
#   --all           both text encoders                          (~745 MB)
#
# Point `[ai] model_dir` at <model_dir>. The Nix equivalent is nix/models.nix.
# Keep the table below in sync with nix/models.nix and
# crates/stickpile-ai/src/lib.rs (`MODEL_FILES`); a unit test checks it.
set -euo pipefail

XENOVA="https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/6b7c523703ae26e39be4b03774966f8571c7f523"
MCLIP="https://huggingface.co/sentence-transformers/clip-ViT-B-32-multilingual-v1/resolve/58edf8cada9e398793dca955574a48cbb7f18be2"

# set  path                              url                                        sha256                                                            bytes
FILES="
clip  clip-vit-b32/vision.onnx         $XENOVA/onnx/vision_model.onnx             fd6e1402a588279d1723c7534d4bcba5bc0b14b47dfab0e46f8c47b8270d7d40  351685709
en    clip-vit-b32/text.onnx           $XENOVA/onnx/text_model.onnx               3f6571f5bad13a97c469c1622e1cfc4d9aef78b79fdbfcff804ca357bfada8cc  254058553
en    clip-vit-b32/tokenizer.json      $XENOVA/tokenizer.json                     f7f3b7af117d467b58374797691a6438d3e6b9e9cef800dfd5dced7f697a90cd  2224119
ml    multilingual/model.onnx          $MCLIP/onnx/model_qint8_avx512.onnx        3bed77e83926519660b5c0833cb3321cfde952331edec4f7d501c7d384b8a8a7  135336307
ml    multilingual/tokenizer.json      $MCLIP/tokenizer.json                      5b4e1a8171c81dfd666ae40265b9530c6e0b3d53923fe8ac493dcc84229adf81  1961847
ml    multilingual/dense.safetensors   $MCLIP/2_Dense/model.safetensors           d12568dc7300970a4d3dbb49068ad16cd89b99840b74b026f8e48071e9414f74  1572984
"

usage() {
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

sets="clip en"
dir=""
for arg in "$@"; do
  case "$arg" in
    --english) sets="clip en" ;;
    --multilingual) sets="clip ml" ;;
    --all) sets="clip en ml" ;;
    -h | --help) usage 0 ;;
    -*) echo "unknown option: $arg" >&2; usage 2 ;;
    *) [ -z "$dir" ] || usage 2; dir="$arg" ;;
  esac
done
[ -n "$dir" ] || usage 2

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
else
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

mkdir -p "$dir"
echo "$FILES" | while read -r set path url hash bytes; do
  [ -n "$set" ] || continue
  case " $sets " in *" $set "*) ;; *) continue ;; esac
  dest="$dir/$path"
  if [ -f "$dest" ] && [ "$(sha256 "$dest")" = "$hash" ]; then
    echo "ok       $path"
    continue
  fi
  mkdir -p "$(dirname "$dest")"
  tmp="$dest.part"
  echo "fetching $path ($((bytes / 1000000)) MB)"
  curl --fail --location --retry 3 --silent --show-error --output "$tmp" "$url"
  got="$(sha256 "$tmp")"
  if [ "$got" != "$hash" ]; then
    rm -f "$tmp"
    echo "sha256 mismatch for $path: expected $hash, got $got" >&2
    exit 1
  fi
  mv -f "$tmp" "$dest"
  echo "ok       $path"
done
echo "models ready in $dir"
