# Installs Fennec for the current user (no root needed).
#   make install                   CPU build
#   make install FEATURES=vulkan   AMD/Intel GPU (needs glslc and Vulkan headers)
#   make install FEATURES=cuda     NVIDIA GPU (needs the CUDA toolkit)
PREFIX ?= $(HOME)/.local
APP_ID = io.github.fennec.Fennec
FEATURES ?=
CARGO_FLAGS = --release --locked $(if $(FEATURES),--features $(FEATURES))
ICON = share/icons/hicolor/scalable/apps/$(APP_ID).svg

.PHONY: install uninstall deb

install:
	cargo build $(CARGO_FLAGS)
	install -Dm755 target/release/fennec $(PREFIX)/bin/fennec
	install -Dm755 target/release/fennec-bench $(PREFIX)/bin/fennec-bench
	install -Dm644 data/$(APP_ID).desktop $(PREFIX)/share/applications/$(APP_ID).desktop
	install -Dm644 data/icons/hicolor/scalable/apps/$(APP_ID).svg $(PREFIX)/$(ICON)
	-update-desktop-database $(PREFIX)/share/applications 2>/dev/null
	-gtk-update-icon-cache -f -t $(PREFIX)/share/icons/hicolor 2>/dev/null

uninstall:
	rm -f $(PREFIX)/bin/fennec $(PREFIX)/bin/fennec-bench
	rm -f $(PREFIX)/share/applications/$(APP_ID).desktop $(PREFIX)/$(ICON)
	-update-desktop-database $(PREFIX)/share/applications 2>/dev/null

# An installable package: sudo apt install ./target/debian/fennec_*.deb
deb:
	FEATURES="$(FEATURES)" scripts/build-deb.sh
