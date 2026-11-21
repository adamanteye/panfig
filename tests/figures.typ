#import "../lib.typ": panfig

#let pgf(body) = "\\begin{pgfpicture}" + body + "\\end{pgfpicture}"
#let mono = "\\fontspec{DejaVu Sans Mono}\\fontsize{12bp}{14bp}\\selectfont iiiiiiiiiiiiiii"
#context {
  let w = measure(text(
    font: "DejaVu Sans Mono",
    size: 12pt,
    "iiiiiiiiiiiiiii",
  )).width
  for preamble in ("", "% force TeX") {
    assert(
      calc.abs(
        measure(panfig(
          pgf("\\pgftext{" + mono + "}"),
          preamble: preamble,
        )).width
          - w,
      )
        < 0.01pt,
    )
    let height(body) = {
      measure(panfig(pgf("\\pgftext{" + body + "}"), preamble: preamble)).height
    }
    assert(height("Hg") > height("H") + 1pt)
  }
}

#show "Native": it => {
  metadata("native text")
  it
}
#show curve: it => {
  for c in it.components.filter(c => c.func() == curve.close) {
    assert.eq(c.mode, "straight")
  }
  metadata("curve")
  it
}
#show image: it => {
  metadata("image")
  it
}

#let shifted = pgf(
  "\\pgftransformshift{\\pgfpoint{5bp}{6bp}}\\pgfcoordinate{peak}{\\pgfpoint{10bp}{20bp}}\\pgftext[at={\\pgfpointanchor{peak}{center}}]{Native}",
)
#figure(panfig(shifted))
#let spectrum = ```svg
<svg xmlns="http://www.w3.org/2000/svg" width="120pt" height="54pt" viewBox="0 0 120 54">
  <defs>
    <clipPath id="axes"><rect x="5" y="5" width="108" height="38"/></clipPath>
  </defs>
  <g clip-path="url(#axes)">
    <path d="M0 40 Q25 40 38 28 T62 18 T84 26 T120 32" fill="none" stroke="#3355aa" stroke-width="2"/>
  </g>
  <path d="M5 5 V43 H113" fill="none" stroke="#222"/>
  <text id="peak" x="59" y="12" text-anchor="middle" font-size="10">peak</text>
</svg>
```
#{
  show math.equation: it => {
    metadata("svg replacement")
    it
  }
  show "peak": [$E_"max"$]
  panfig(spectrum)
}

#let inkscape = ```svg
<?xml version="1.0"?>
<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd">
<svg xmlns="http://www.w3.org/2000/svg" width="80pt" viewBox="-8 -4 40 20">
  <g opacity="0.4"><circle r="5"/><circle cx="3" r="5"/></g>
  <g transform="translate(8,10) rotate(-90)">
    <text id="axis"><tspan x="1" y="2" font-weight="bold">Native</tspan></text>
  </g>
  <rect x="16" width="3" height="5"/>
</svg>
```
#panfig(inkscape)
#context assert.eq(measure(panfig(inkscape)), (width: 80pt, height: 40pt))

#panfig("\\begin{tikzpicture}\\node (dot) at (1,2) {Native};\\end{tikzpicture}")

#let dense = pgf(
  "\\pgfpathcircle{\\pgfpointorigin}{1bp}\\pgfusepath{stroke}" * 150
    + "\\pgftext{Native}",
)
#panfig(dense)
#context {
  let kinds = query(metadata).map(it => it.value)
  assert.eq(kinds, (
    "native text",
    "image",
    "curve",
    "svg replacement",
    "image",
    "native text",
    "curve",
    "native text",
    "image",
    "native text",
    "lifted math",
    "lifted math",
  ))
}

#let icon = bytes(
  "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"2pt\" height=\"1pt\"><rect width=\"2\" height=\"1\"/></svg>",
)
#let resources = (
  "sub/one.tex": "\\input{two}",
  "two.tex": "\\pgftext{\\includegraphics{icon}}",
  "icon.svg": icon,
)
#context {
  for preamble in ("", "% force TeX") {
    let size = measure(panfig(
      pgf("\\input{sub/one.tex}"),
      preamble: preamble,
      load: resources,
    ))
    assert(calc.abs(size.width - 2pt) < 0.01pt)
    assert(calc.abs(size.height - 1pt) < 0.01pt)
  }
  assert.eq(measure(panfig(raw(str(icon), lang: "svg"))), (
    width: 2pt,
    height: 1pt,
  ))
  for body in (
    "\\pgfcoordinate{a}{\\pgfpointorigin}\\pgfcoordinate{a}{\\pgfpointorigin}",
    "\\pgfcoordinate{a.b}{\\pgfpointorigin}",
    "\\pgfpathcircle{\\pgfpointorigin}{0pt}\\pgfusepath{stroke}",
    "\\pgfpathmoveto{\\pgfpointorigin}\\pgfpathclose\\pgfusepath{stroke}",
  ) { let _ = measure(panfig(pgf(body))) }
  let node = panfig("\\begin{tikzpicture}\\node {中文};\\end{tikzpicture}")
  assert(measure(node).width > 0pt)
}

// matplotlib activates the percent character inside its text boxes.
#let tick = ```pgf
\begin{pgfpicture}
\pgftext[left,base]{\color{black}{\sffamily\fontsize{12}{14}\selectfont\catcode`\^=\active\def^{\ifmmode\sp\else\^{}\fi}\catcode`\%=\active\def%{\%}120}}%
\end{pgfpicture}
```
#context {
  let w = measure(text(size: (12 * 72 / 72.27) * 1pt, "120")).width
  assert(calc.abs(measure(panfig(tick)).width - w) < 0.01pt)
}

// recursive LaTeX math lifting on both fast and TeX+clip paths without bundled fonts.
#{
  show math.equation: it => {
    metadata("lifted math")
    it
  }
  panfig(pgf("\\pgftext{$P_{4\\ell}(r) = r^2 |R_{4\\ell}(r)|^2$}"))
  panfig(pgf("\\pgfpathrectangle{\\pgfpointorigin}{\\pgfqpoint{80bp}{40bp}}\\pgfusepath{use as bounding box, clip}\\pgftext[at={\\pgfqpoint{40bp}{20bp}},rotate=90]{$x / a_0$}"))
}
