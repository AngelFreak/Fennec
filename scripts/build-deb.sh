#!/bin/bash
# Builds an Ubuntu/Debian package: target/debian/fennec_<version>_<arch>.deb
# Install it with:  sudo apt install ./target/debian/fennec_*.deb
# GTK 4 and libadwaita come from the system (Ubuntu 24.04 or newer); the
# package depends on them, with versions worked out by dpkg-shlibdeps.
# Set FEATURES=vulkan or FEATURES=cuda for a GPU build.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ -f $HOME/.cargo/env ]] && source "$HOME/.cargo/env"

features=${FEATURES:-}
cargo build --release --locked ${features:+--features "$features"}
version=$(cargo pkgid | sed 's/.*[#@]//')
arch=$(dpkg --print-architecture)
app_id=io.github.fennec.Fennec
out=$PWD/target/debian
root=$out/root
rm -rf "$root"
mkdir -p "$root/DEBIAN"

install -Dm755 target/release/fennec "$root/usr/bin/fennec"
install -Dm755 target/release/fennec-bench "$root/usr/bin/fennec-bench"
install -Dm644 "data/$app_id.desktop" "$root/usr/share/applications/$app_id.desktop"
install -Dm644 "data/icons/hicolor/scalable/apps/$app_id.svg" \
    "$root/usr/share/icons/hicolor/scalable/apps/$app_id.svg"
# The document serif (Source Serif 4, OFL); Ubuntu does not package it.
install -Dm644 -t "$root/usr/share/fonts/truetype/fennec" data/fonts/*.ttf
install -Dm644 data/fonts/LICENSE-SourceSerif4.md "$root/usr/share/doc/fennec/LICENSE.source-serif-4"
# The interface icons are Lucide's (ISC), built into the binary.
install -Dm644 data/icons/ui/LICENSE "$root/usr/share/doc/fennec/LICENSE.lucide-icons"

# The built-in AI: llama.cpp's server (MIT), a release build with Vulkan and
# CPU backends for every x86-64 level. Fennec starts it only for AI jobs.
llama_build=b11982
llama_sha256=76a959124127f90555469285621c8d9099514724715d652a997f92841755efbb
llama_cache=${LLAMA_CACHE:-$HOME/.cache/fennec/llama}
llama_tgz=$llama_cache/llama-$llama_build-bin-ubuntu-vulkan-x64.tar.gz
if [[ $arch == amd64 ]]; then
    if ! echo "$llama_sha256  $llama_tgz" | sha256sum -c --status 2>/dev/null; then
        mkdir -p "$llama_cache"
        curl -sSfL -o "$llama_tgz" \
            "https://github.com/ggml-org/llama.cpp/releases/download/$llama_build/$(basename "$llama_tgz")"
        echo "$llama_sha256  $llama_tgz" | sha256sum -c --status ||
            { echo "llama.cpp download does not match its checksum" >&2; exit 1; }
    fi
    llama_src=$(mktemp -d)
    tar xzf "$llama_tgz" -C "$llama_src"
    llama_dir=$llama_src/llama-$llama_build
    install -Dm755 "$llama_dir/llama-server" "$root/usr/lib/fennec/llama/llama-server"
    # The server and the libraries it loads; symlinks kept as they are.
    (cd "$llama_dir" && cp -a libllama-server-impl.so libllama.so* libllama-common.so* libmtmd.so* \
        libggml.so* libggml-base.so* libggml-cpu-*.so libggml-vulkan.so "$root/usr/lib/fennec/llama/")
    install -Dm644 "$llama_dir/LICENSE" "$root/usr/share/doc/fennec/LICENSE.llama-cpp"
    rm -rf "$llama_src"
fi

# dpkg-shlibdeps wants a debian/control to exist; give it a throwaway one.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/debian"
printf 'Source: fennec\n\nPackage: fennec\nArchitecture: any\n' >"$work/debian/control"
# llama.cpp's own libraries come in the package; only the system's count.
llama_bins=()
if [[ -d $root/usr/lib/fennec/llama ]]; then
    llama_bins=(-l"$root/usr/lib/fennec/llama" --ignore-missing-info)
    for f in "$root"/usr/lib/fennec/llama/*; do
        [[ -L $f ]] || llama_bins+=(-e"$f")
    done
fi
depends=$(cd "$work" && dpkg-shlibdeps -O -e"$root/usr/bin/fennec" -e"$root/usr/bin/fennec-bench" "${llama_bins[@]}" |
    sed -n 's/^shlibs:Depends=//p')
[[ -n $depends ]] || { echo "could not work out the dependencies" >&2; exit 1; }

# Maintainer: $MAINTAINER, else your git identity, else the commit author.
maintainer=${MAINTAINER:-}
if [[ -z $maintainer ]]; then
    name=$(git config user.name || true)
    email=$(git config user.email || true)
    if [[ -n $name && -n $email ]]; then
        maintainer="$name <$email>"
    else
        maintainer=$(git log -1 --format='%an <%ae>')
    fi
fi

cat >"$root/DEBIAN/control" <<CONTROL
Package: fennec
Version: $version
Architecture: $arch
Maintainer: $maintainer
Installed-Size: $(du -sk --exclude=DEBIAN "$root" | cut -f1)
Depends: $depends
Recommends: ffmpeg, gstreamer1.0-plugins-good, gnome-keyring, fonts-ibm-plex
Suggests: python3-venv
Section: sound
Priority: optional
Description: Danish dictation and transcription on your own computer
 Fennec turns Danish and English speech into text locally with Whisper
 models (Edda): live dictation into an editor and transcription of audio
 and video files, with report templates and export to TXT, DOCX and PDF.
 Optional AI meeting notes, summaries and proofreading, through a built-in
 model on this computer, Claude, ChatGPT or a local server, are off until
 turned on.
CONTROL

find "$root" -type d -exec chmod 755 {} +
deb=$out/fennec_${version}_${arch}.deb
dpkg-deb --build --root-owner-group "$root" "$deb" >/dev/null
rm -rf "$root"
echo "$deb"
