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

.PHONY: all build run install uninstall test clean

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
