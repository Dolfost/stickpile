# stickpile AI models as fixed-output derivations, in the layout of
# docs/ai.md (point `[ai] model_dir` / `services.stickpile.settings.ai.model_dir`
# at one of these):
#
#   english       CLIP ViT-B/32 image + English text tower   (~608 MB)
#   multilingual  CLIP ViT-B/32 image + multilingual text    (~490 MB)
#   all           both text encoders                          (~745 MB)
#
# Usage: `models = import ./nix/models.nix { inherit pkgs; };` then `models.all`.
# Keep in sync with scripts/fetch-models.sh and `MODEL_FILES` in
# crates/stickpile-ai/src/lib.rs (a unit test checks the hashes).
{ pkgs }:
let
  xenova = "https://huggingface.co/Xenova/clip-vit-base-patch32/resolve/6b7c523703ae26e39be4b03774966f8571c7f523";
  mclip = "https://huggingface.co/sentence-transformers/clip-ViT-B-32-multilingual-v1/resolve/58edf8cada9e398793dca955574a48cbb7f18be2";

  file = path: url: hash: {
    name = path;
    path = pkgs.fetchurl {
      # Store name: the layout path with "/" replaced.
      name = "stickpile-" + builtins.replaceStrings [ "/" ] [ "-" ] path;
      inherit url hash;
    };
  };

  # sha256 fd6e1402a588279d1723c7534d4bcba5bc0b14b47dfab0e46f8c47b8270d7d40 (fp32, 351685709 bytes)
  vision = file "clip-vit-b32/vision.onnx" "${xenova}/onnx/vision_model.onnx"
    "sha256-/W4UAqWIJ50XI8dTTUvLpbwLFLR9+rDkb4xHuCcNfUA=";
  # sha256 3f6571f5bad13a97c469c1622e1cfc4d9aef78b79fdbfcff804ca357bfada8cc (fp32, 254058553 bytes)
  clipText = file "clip-vit-b32/text.onnx" "${xenova}/onnx/text_model.onnx"
    "sha256-P2Vx9brROpfEacFiLhz8TZrveLef2/z/gEyjV7+tqMw=";
  # sha256 f7f3b7af117d467b58374797691a6438d3e6b9e9cef800dfd5dced7f697a90cd
  clipTokenizer = file "clip-vit-b32/tokenizer.json" "${xenova}/tokenizer.json"
    "sha256-9/O3rxF9RntYN0eXaRpkONPmuenO+ADf1dztf2l6kM0=";
  # sha256 3bed77e83926519660b5c0833cb3321cfde952331edec4f7d501c7d384b8a8a7 (qint8, 135336307 bytes;
  # byte-identical to model_qint8_arm64.onnx, runs on any CPU)
  mclipModel = file "multilingual/model.onnx" "${mclip}/onnx/model_qint8_avx512.onnx"
    "sha256-O+136DkmUZZgtcCDPLMyHP3pUjMe3sT31QHH04S4qKc=";
  # sha256 5b4e1a8171c81dfd666ae40265b9530c6e0b3d53923fe8ac493dcc84229adf81
  mclipTokenizer = file "multilingual/tokenizer.json" "${mclip}/tokenizer.json"
    "sha256-W04agXHIHf1mauQCZblTDG4LPVOSP+isST3MhCKa34E=";
  # sha256 d12568dc7300970a4d3dbb49068ad16cd89b99840b74b026f8e48071e9414f74
  mclipDense = file "multilingual/dense.safetensors" "${mclip}/2_Dense/model.safetensors"
    "sha256-0SVo3HMAlwpNPbtJBorRbNibmYQLdLAm+OSAcelBT3Q=";

  farm = name: files: pkgs.linkFarm name files;
in
{
  english = farm "stickpile-models-english" [ vision clipText clipTokenizer ];
  multilingual = farm "stickpile-models-multilingual" [ vision mclipModel mclipTokenizer mclipDense ];
  all = farm "stickpile-models" [ vision clipText clipTokenizer mclipModel mclipTokenizer mclipDense ];
}
