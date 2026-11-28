WASM_TARGET = wasm32-unknown-unknown
VERSION = $(shell sed -n 's/^version = "\([^"]*\)"/\1/p' typst.toml)

.PHONY: all wasm tex check package
all: wasm tex

wasm:
	cargo build --locked --release -p panfig --target $(WASM_TARGET)
	wasm-opt -Oz --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
		-o panfig.wasm target/$(WASM_TARGET)/release/panfig.wasm

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
		panfig.wasm latex.wasm rust-tikz.LICENSE \
		$$(git ls-files examples)
