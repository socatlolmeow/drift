PREFIX  ?= /usr/local
BINDIR  := $(PREFIX)/bin
BINARY  := drift

.PHONY: all build install uninstall clean

all: build

build:
	cargo build --release

install: build
	install -Dm755 target/release/$(BINARY) $(DESTDIR)$(BINDIR)/$(BINARY)

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/$(BINARY)

clean:
	cargo clean
