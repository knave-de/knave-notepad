PREFIX ?= /usr/local

.PHONY: build install uninstall
build:
	cargo build --release --locked
install: build
	install -Dm755 target/release/knave-notepad $(DESTDIR)$(PREFIX)/bin/knave-notepad
	install -Dm644 data/knave-notepad.desktop $(DESTDIR)$(PREFIX)/share/applications/knave-notepad.desktop
uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/knave-notepad
	rm -f $(DESTDIR)$(PREFIX)/share/applications/knave-notepad.desktop
