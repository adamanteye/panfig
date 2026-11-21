WASM_TARGET = wasm32-unknown-unknown

.PHONY: all wasm tex check
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
