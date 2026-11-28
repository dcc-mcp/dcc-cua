# Start a private Blender build from the official versioned release archives.
set -euo pipefail
version=5.1.1
scratch=$(mktemp -d)
case "$RUNNER_OS" in
  Linux)
    file="blender-${version}-linux-x64.tar.xz"
    sha=6f9fff89fef154ef7974d1a1c4b916ab4bc1f5618bcb48d5befee1bd0a7c7f2a
    ;;
  macOS)
    file="blender-${version}-macos-arm64.dmg"
    sha=ff6219b3aaab4d9adf548b9a32b3b31764fe740b6c741d16660c5c0f4ffe9841
    ;;
  Windows)
    file="blender-${version}-windows-x64.zip"
    sha=307123caa3bad0c1d04ec373a2f22f622c403eecb34b9c646ede104906b04ced
    ;;
  *) exit 2 ;;
esac
curl --fail --location --retry 2 --max-time 240 \
  "https://download.blender.org/release/Blender5.1/$file" -o "$scratch/$file"
(cd "$scratch" && printf '%s  %s\n' "$sha" "$file" | if command -v sha256sum >/dev/null; then sha256sum -c -; else shasum -a 256 -c -; fi)
command=(python -B scripts/test_blender_workflows.py --binary target/release/dcc-cua --report "$RUNNER_TEMP/blender-workflows.json")
case "$RUNNER_OS" in
  Linux)
    tar -xJf "$scratch/$file" -C "$scratch"
    export XDG_SESSION_TYPE=x11
    unset WAYLAND_DISPLAY
    xvfb-run -a --server-args="-screen 0 1600x1100x24 -nolisten tcp" dbus-run-session -- \
      bash -ec 'openbox >/dev/null 2>&1 & exec "$@"' -- "${command[@]}" --blender "$scratch/${file%.tar.xz}/blender"
    ;;
  macOS)
    hdiutil attach -readonly -nobrowse -mountpoint "$scratch/mount" "$scratch/$file"
    trap 'hdiutil detach "$scratch/mount"' EXIT
    "${command[@]}" --blender "$scratch/mount/Blender.app/Contents/MacOS/Blender"
    ;;
  Windows)
    unzip -q "$scratch/$file" -d "$scratch"
    python -B scripts/test_blender_workflows.py --binary target/release/dcc-cua.exe \
      --report "$RUNNER_TEMP/blender-workflows.json" --blender "$scratch/${file%.zip}/blender.exe"
    ;;
esac
