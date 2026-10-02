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
# The interface icons are Lucide's (ISC), built into the binary.
install -Dm644 data/icons/ui/LICENSE "$root/usr/share/doc/fennec/LICENSE.lucide-icons"

# dpkg-shlibdeps wants a debian/control to exist; give it a throwaway one.
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/debian"
printf 'Source: fennec\n\nPackage: fennec\nArchitecture: any\n' >"$work/debian/control"
depends=$(cd "$work" && dpkg-shlibdeps -O -e"$root/usr/bin/fennec" -e"$root/usr/bin/fennec-bench" |
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
 Fennec turns Danish speech into text locally with Whisper models (Edda,
 Hviske, Røst): live dictation into an editor and transcription of audio
 and video files, with report templates and export to TXT, DOCX and PDF.
 Optional AI summaries and clean-up through Claude, ChatGPT or a local
 model are off until turned on.
CONTROL

find "$root" -type d -exec chmod 755 {} +
deb=$out/fennec_${version}_${arch}.deb
dpkg-deb --build --root-owner-group "$root" "$deb" >/dev/null
rm -rf "$root"
echo "$deb"
