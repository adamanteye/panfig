# panfig

Import PGF, TikZ, Matplotlib PGF, and SVG figures as native Typst content.

Where possible, paths and labels are turned into Typst `curve` and `text`
elements so figures pick up your document font and work with normal `set` and
`show` rules. Anything that cannot be lowered directly stays in SVG layers, and
PGF/TikZ code that needs TeX runs in a bundled WebAssembly e-TeX engine.

Requires Typst 0.15+.

## Usage

```typst
#import "lib.typ": panfig

#figure(
  panfig("examples/hydrogen.pgf"),
  caption: [Hydrogen atom wavefunction probability densities],
)
```

Because lifted labels are ordinary Typst text, you can restyle or replace them
with a scoped `show` rule:

```typst
#{
  show "peak": [$E_"max"$]
  panfig(```svg
    <svg xmlns="http://www.w3.org/2000/svg" width="80pt" height="30pt">
      <path d="M5 25 Q40 0 75 25" fill="none" stroke="#35a" stroke-width="2"/>
      <text x="40" y="10" text-anchor="middle" font-size="10">peak</text>
    </svg>
  ```)
}
```

See [examples/hydrogen.typ](examples/hydrogen.typ) for the complete example.

## API

```typst
panfig(
  source,
  format: auto,
  preamble: "",
  resolve: none,
  load: none,
)
```

- `source`: a caller-relative filename, Typst `path`, raw block, source string, or `bytes`.
- `format`: `auto` (default), `"pgf"` (aliases: `"tikz"`, `"tex"`, `"latex"`), or `"svg"`.
- `preamble`: extra LaTeX preamble lines for TeX-compiled figures.
- `resolve`: pass `resolve: p => path(p)` at the call site when importing a file that references other files via `\input` or `\includegraphics` across package/project boundaries.
- `load`: a dictionary or function supplying referenced files (for example `load: ("plot.jpg": path("figures/plot.jpg"))`). Required for extensionless or optional images in TeX.

## Notes

- **PGF and TikZ**: Simple drawing commands and inline math are parsed directly without invoking TeX. Full TikZ, macros, and complex layouts run in the bundled e-TeX/PGF engine, which ships a fixed LaTeX2e 2020 macro environment (`amsmath`, PGF, and `mhchem` v3.11) without bundled fonts. Supported text and math are lifted into Typst and rendered with your document/system fonts; missing fonts produce a Typst warning. XeTeX/LuaTeX primitives and PDF/PostScript-only specials are not supported.
- **SVG**: Expects a self-contained SVG (with embedded raster images). Arbitrary clipping paths, group opacity, gradients, skewed text, and very dense path batches are kept as SVG layers in their original paint order and viewport. Text already converted to outlines stays geometry.

## Development

Prebuilt plugins and TeX runtime assets in `runtime/` are not tracked in git; build them locally (or unpack a release archive) before running Typst:

```sh
make all     # build runtime/panfig.wasm and runtime/latex.wasm + TeX assets
make check   # run Rust and Typst tests
```

## License

MIT. See [LICENSE](LICENSE) for project code and [runtime/NOTICE](runtime/NOTICE) for the TeX runtime notices.
