# Colorz build helpers. `install` puts the app under $(PREFIX) (default ~/.local).

CARGO ?= $(HOME)/.cargo/bin/cargo
PREFIX ?= $(HOME)/.local
APP_ID := io.github.partty.Colorz

BINDIR := $(PREFIX)/bin
APPDIR := $(PREFIX)/share/applications
ICONDIR := $(PREFIX)/share/icons/hicolor/scalable/apps

.PHONY: build test lint install uninstall

build:
	$(CARGO) build --release

test:
	$(CARGO) test --workspace

lint:
	$(CARGO) clippy --workspace --all-targets -- -D warnings
	$(CARGO) fmt --all --check

install: build
	install -Dm755 target/release/colorz $(BINDIR)/colorz
	install -Dm644 data/$(APP_ID).desktop $(APPDIR)/$(APP_ID).desktop
	install -Dm644 data/$(APP_ID).svg $(ICONDIR)/$(APP_ID).svg
	-update-desktop-database -q $(APPDIR)

uninstall:
	rm -f $(BINDIR)/colorz $(APPDIR)/$(APP_ID).desktop $(ICONDIR)/$(APP_ID).svg
	-update-desktop-database -q $(APPDIR)
