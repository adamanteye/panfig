WASM_TARGET = wasm32-unknown-unknown
VERSION = $(shell sed -n 's/^version = "\([^"]*\)"/\1/p' typst.toml)

.PHONY: all wasm tex check package
all: wasm tex

wasm:
	cargo build --locked --release -p panfig --target $(WASM_TARGET)
	cp target/$(WASM_TARGET)/release/panfig.wasm runtime/panfig.wasm

tex:
	python3 tex/build.py

check:
	cargo test --locked --workspace
	typst compile --root . tests/stream.typ target/stream.pdf
	typst compile --root . tests/figures.typ target/figures.pdf
	python3 tests/diagnostics.py

package: all
	tar -czf target/panfig-$(VERSION).tar.gz \
		--transform 's,^,packages/preview/panfig/$(VERSION)/,' \
		typst.toml lib.typ README.md LICENSE \
		runtime/panfig.wasm runtime/latex.wasm runtime/format.gz \
		runtime/texmf.tar.gz runtime/NOTICE runtime/rust-tikz.LICENSE \
		$$(git ls-files examples)
