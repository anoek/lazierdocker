# lazierdocker — build, run, install.
#
#   make                 build an optimised binary (target/release/lazierdocker)
#   make run ARGS="-H tcp://127.0.0.1:23750"
#   make install         install to $(PREFIX)/bin (default ~/.local/bin)
#   make install PREFIX=/usr/local   (may need sudo)
#   make uninstall
#
# Building needs Rust and Zig 0.15.2 (the libghostty-vt crate builds Ghostty's VT library).

PREFIX  ?= $(HOME)/.local
BINDIR  ?= $(PREFIX)/bin
DESTDIR ?=
CARGO   ?= cargo
ARGS    ?=

BIN    := lazierdocker
TARGET := target/release/$(BIN)

# rustup and our Zig live in the user's home and may not be on PATH.
export PATH := $(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

.PHONY: all build run install uninstall test clean screenshot

all: build

build:
	$(CARGO) build --release

run: build
	./$(TARGET) $(ARGS)

install: build
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 0755 "$(TARGET)" "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "installed $(DESTDIR)$(BINDIR)/$(BIN)"

uninstall:
	rm -f "$(DESTDIR)$(BINDIR)/$(BIN)"
	@echo "removed $(DESTDIR)$(BINDIR)/$(BIN)"

test:
	$(CARGO) test

clean:
	$(CARGO) clean

# README screenshot from made-up state (needs chromium and ImageMagick).
screenshot:
	$(CARGO) run --example screenshot -- target/screenshot.html
	chromium --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
		--window-size=1500,900 --screenshot=target/screenshot-raw.png "file://$(CURDIR)/target/screenshot.html"
	magick target/screenshot-raw.png -fuzz 1% -trim +repage -bordercolor '#1e1e2e' -border 28 docs/screenshot.png
