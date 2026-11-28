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
verify_archive() {
  (cd "$scratch" && printf '%s  %s\n' "$sha" "$file" |
    if command -v sha256sum >/dev/null; then sha256sum -c -; else shasum -a 256 -c -; fi)
}
# All sources distribute the same pinned Blender archive. Hosted runners
# sometimes receive HTTP 403 from the primary download server.
downloaded=false
for source in https://download.blender.org https://mirror.clarkson.edu/blender https://mirrors.ocf.berkeley.edu/blender; do
  if curl --fail --location --connect-timeout 10 --max-time 150 \
      "$source/release/Blender5.1/$file" -o "$scratch/$file" && verify_archive; then
    downloaded=true
    break
  fi
done
if [[ "$downloaded" != true ]]; then
  echo "No verified Blender $version archive available for $RUNNER_OS" >&2
  exit 1
fi
runner=()
geometry=()
binary=target/release/dcc-cua
case "$RUNNER_OS" in
  Linux)
    tar -xJf "$scratch/$file" -C "$scratch"
    export XDG_SESSION_TYPE=x11
    unset WAYLAND_DISPLAY
    runner=(xvfb-run -a --server-args="-screen 0 1600x1100x24 -nolisten tcp -noreset" dbus-run-session -- \
      bash -ec 'openbox >/dev/null 2>&1 &
        for _ in {1..100}; do wmctrl -m >/dev/null 2>&1 && break; sleep 0.05; done
        wmctrl -m >/dev/null
        exec "$@"' --)
    blender="$scratch/${file%.tar.xz}/blender"
    ;;
  macOS)
    hdiutil attach -readonly -nobrowse -mountpoint "$scratch/mount" "$scratch/$file"
    trap 'hdiutil detach "$scratch/mount"' EXIT
    ditto "$scratch/mount/Blender.app" "$scratch/Blender.app"
    hdiutil detach "$scratch/mount"
    trap - EXIT
    blender="$scratch/Blender.app/Contents/MacOS/Blender"
    geometry=(--window-geometry 80 160 700 420)
    ;;
  Windows)
    unzip -q "$scratch/$file" -d "$scratch"
    mesa=/c/msys64/ucrt64/bin
    cp "$mesa/opengl32.dll" "$scratch/${file%.zip}/"
    export PATH="$mesa:$PATH"
    export GALLIUM_DRIVER=llvmpipe
    binary=target/release/dcc-cua.exe
    blender="$scratch/${file%.zip}/blender.exe"
    ;;
esac
command=(python -B scripts/test_blender_workflows.py --binary "$binary" --blender "$blender" "${geometry[@]}")
"${runner[@]}" "${command[@]}" --report "$RUNNER_TEMP/blender-workflows.json"
"${runner[@]}" "${command[@]}" --pointer-sequence --report "$RUNNER_TEMP/blender-pointer.json"
