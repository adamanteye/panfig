# panfig

Import PGF, TikZ, Matplotlib PGF, and SVG figures into Typst with native text
and math.

Labels and inline math are lifted into native Typst `text` and `math.equation`
elements so figures inherit your document fonts and work with normal `set` and
`show` rules. Vector geometry is coalesced into paint-ordered SVG layers, and
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

Self-contained figures work without extra options, even across package or
subdirectory boundaries. When a figure references external files (such as
Matplotlib's `\includegraphics` images or `\input`) across package or directory
boundaries, Typst cannot resolve sibling paths from inside a library; thus
panfig has to bind `resolve` once at the top of your document (or pass it on
that call):

```typst
#import "lib.typ": panfig
#let panfig = panfig.with(resolve: p => path(p))

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
- `resolve`: optional caller-site path resolver (`resolve: p => path(p)`). Not needed for self-contained figures; bind `#let panfig = panfig.with(resolve: p => path(p))` when importing figures that reference external files via `\input` or `\includegraphics` across package or directory boundaries.
- `load`: a dictionary or function supplying referenced files (for example `load: ("plot.jpg": path("figures/plot.jpg"))`). Required for extensionless or optional images in TeX.

## Notes

- **PGF and TikZ**: The Rust reader handles static Matplotlib paths, clipping, affine transforms, marker reuse, and text with inline math, coalescing geometry into paint-ordered SVG layers while lifting labels into Typst. Linked images load through the same caller-relative resolver as the figure. Full TikZ, macros, and complex layouts run in the bundled e-TeX/PGF engine, which ships a fixed LaTeX2e 2020 macro environment (`amsmath`, PGF, and `mhchem` v3.11) without bundled fonts. Supported text and math are lifted into Typst and rendered with your document/system fonts; missing fonts produce a Typst warning. XeTeX/LuaTeX primitives and PDF/PostScript-only specials are not supported.
- **SVG**: Expects a self-contained SVG (with embedded raster images). Representable `<text>` elements are lifted into native Typst text while geometry, clipping paths, group opacity, gradients, and skewed text stay intact in paint-ordered SVG layers. Text already converted to outlines stays geometry.

## Development

Prebuilt plugins and TeX runtime assets in `runtime/` are not tracked in git; build them locally (or unpack a release archive) before running Typst:

```sh
make all     # build runtime/panfig.wasm and runtime/latex.wasm + TeX assets
make check   # run Rust and Typst tests
```

## License

MIT. See [LICENSE](LICENSE) for project code and [runtime/NOTICE](runtime/NOTICE) for the TeX runtime notices.
