// import external figures into the current Typst layout context.
#let panfig = {
  let wasm = plugin("runtime/panfig.wasm")
  let encode(value) = bytes(json.encode(value))

  let escape(s) = s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;").replace("\"", "&quot;")
  let number(size) = json.encode(size / 1pt)

  let bounds(at, anchor, width, height, ascent) = {
    let dx = if anchor.ends-with("west") { 0pt } else if anchor.ends-with("east") { width } else { width / 2 }
    let dy = if anchor.starts-with("base") { ascent } else if anchor.starts-with("north") { 0pt } else if anchor.starts-with("south") { height } else { height / 2 }
    (x: at.at(0) * 1pt - dx, y: -at.at(1) * 1pt - dy, width: width, height: height, ascent: ascent)
  }

  let label(body, at, anchor) = {
    let ascent = measure(text(top-edge: "bounds", bottom-edge: "baseline", body)).height
    let body = text(top-edge: "bounds", bottom-edge: "bounds", body)
    let (width, height) = measure(body)
    (..bounds(at, anchor, width, height, ascent), body: body)
  }

  let run-text(runs, font, size) = {
    let body = runs.map(r => {
      if "math" in r {
        let eq = eval("$" + r.math + "$")
        return if "fill" in r { text(fill: rgb(..r.fill), eq) } else { eq }
      }
      let body = [#(r.text)]
      if r.italic { body = text(style: "italic", body) }
      if r.bold { body = text(weight: "bold", body) }
      if "fill" in r { body = text(fill: rgb(..r.fill), body) }
      body
    }).join([])
    if font != none { body = text(font: font, body) }
    if size != none { body = text(size: size * 1pt, body) }
    body
  }

  // the marker contains a drawable copy for clips and transforms that require SVG.
  let svg-label(id, runs, font, size) = {
    let em = if size == none { text.size } else { size * 1pt }
    let entry = label(run-text(runs, font, size), (0, 0), "north-west")
    let family = if font == none { text.font } else { font }
    let family = if type(family) == array { family } else { (family,) }
    let family = family.map(f => repr(if type(f) == dictionary { f.name } else { f })).join(", ")
    let weights = (thin: 100, extralight: 200, light: 300, regular: 400, medium: 500,
      semibold: 600, bold: 700, extrabold: 800, black: 900)
    let weight = if type(text.weight) == str { weights.at(text.weight) } else { text.weight }
    let spans = runs.map(r => {
      let w = if r.bold { " font-weight=\"bold\"" } else { "" }
      let i = if r.italic { " font-style=\"italic\"" } else { "" }
      let c = if "fill" in r { " fill=\"" + rgb(..r.fill).to-hex() + "\"" } else { "" }
      let content = if "math" in r { r.math } else { r.text }
      "<tspan" + w + i + c + ">" + escape(content) + "</tspan>"
    }).join("")
    let body = ("<text data-panfig-label=\"" + id + "\" x=\"" + number(entry.x) + "\" y=\"" + number(entry.y + entry.ascent)
      + "\" font-family=\"" + escape(family) + "\" font-size=\"" + number(em)
      + "\" font-weight=\"" + str(weight) + "\" font-style=\"" + text.style + "\" fill=\"currentColor"
      + "\" xml:space=\"preserve\">" + spans + "</text>")
    (..entry, body: body)
  }

  let packed(data) = str(wasm.pack(bytes(data)))
  let tex-engine() = {
    let base = plugin("runtime/latex.wasm")
    plugin.transition(base.initialize, read("runtime/texmf.tar.gz", encoding: none), read("runtime/format.gz", encoding: none))
  }
  let compile(job) = {
    let files = job.files
    let defs = ""
    for (id, item) in job.labels {
      let macro = "\\expandafter\\def\\csname " + id + "\\endcsname"
      let size = item.at("size", default: none)
      let entry = svg-label(id, item.runs, item.at("font", default: none), size)
      let w = number(calc.max(0.001pt, entry.width))
      let h = number(calc.max(0.001pt, entry.height))
      let svg = ("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"" + w + "pt\" height=\"" + h
        + "pt\" viewBox=\"0 0 " + w + " " + h + "\">" + entry.body + "</svg>")
      let file = id + ".svg"
      files.insert(file, packed(svg))
      defs += macro + "{\\begingroup\\catcode37=14\\relax\\lower " + number(entry.height - entry.ascent) + "bp\\hbox{\\includegraphics{" + file + "}}\\endgroup}%\n"
    }
    json(tex-engine().compile(bytes(defs + job.declarations + job.document), encode(files)))
  }

  let point(p) = p.map(n => n * 1pt)
  let native-curve(layer) = {
    let style = layer.style
    let components = layer.components.map(c => {
      let (op, ..points) = c
      let build = (move: curve.move, line: curve.line, quad: curve.quad, cubic: curve.cubic, close: curve.close.with(mode: "straight")).at(op)
      build(..points.map(point))
    })
    let paint(color, opacity) = rgb(..color, opacity * 100%)
    let fill = if style.fill != none { paint(style.fill, style.fill-opacity) }
    let stroke = if style.stroke != none {
      let s = style.stroke
      let dash = if "dash" in s { (array: point(s.dash.array), phase: s.dash.phase * 1pt) }
      (paint: paint(s.color, s.opacity), thickness: s.width * 1pt, cap: s.cap,
       join: s.join, miter-limit: s.miter, dash: dash)
    }
    curve(..components, stroke: stroke, fill: fill, fill-rule: style.fill-rule)
  }

  let positioned(at, size, body) = (
    x: at.first() * 1pt, y: -at.last() * 1pt, width: size.first() * 1pt, height: size.last() * 1pt, body: body,
  )

  let canvas(entries, viewport: none) = {
    let (x, y, w, h) = if viewport != none { (0pt, 0pt, ..point(viewport)) }
      else if entries.len() == 0 { (0pt, 0pt, 0pt, 0pt) }
      else {
        let x = calc.min(..entries.map(e => e.x))
        let y = calc.min(..entries.map(e => e.y))
        (x, y, calc.max(..entries.map(e => e.x + e.width)) - x, calc.max(..entries.map(e => e.y + e.height)) - y)
      }
    box(width: w, height: h, clip: viewport != none, {
      for e in entries { place(top + left, dx: e.x - x, dy: e.y - y, e.body) }
    })
  }

  let svg-text(layer, body, anchor) = {
    let (sx, sy) = layer.at("scale", default: (1, 1))
    let angle = layer.at("angle", default: 0)
    if (sx, sy, angle) == (1, 1, 0) { return label(body, layer.at, anchor) }
    let e = label(body, (0, 0), anchor)
    let frame = box(width: 0pt, height: 0pt, place(top + left, dx: e.x, dy: e.y, box(width: e.width, height: e.height, e.body)))
    let frame = scale(x: sx * 100%, y: sy * 100%, origin: top + left, frame)
    positioned(layer.at, (0, 0), rotate(angle * 1deg, origin: top + left, frame))
  }

  /// import a filename, path, raw block, string or bytes as a Typst figure.
  let import-figure(format: auto, load: none, preamble: "", resolve: none, ..input) = context {
    assert(input.named() == (:), message: "Unknown panfig option: " + input.named().keys().join(", "))
    assert(input.pos().len() == 1, message: "Pass one drawing source or filename.")
    let source = input.pos().first()
    let (format, load, resolve) = (format, load, resolve)
    let lang = none
    let is-file = type(source) == str and not source.contains("\\") and (
      source.contains(regex("(?i)^[^\r\n]+[.](pgf|tikz|tex|svg|typ|cetz)$"))
      and not source.starts-with(regex("^\\s*(%|<)"))
    )
    let path-loader = false
    if resolve != none or is-file {
      assert(type(source) == str, message: "Pass a filename when using resolve.")
      let filename = source
      lang = lower(filename.split(".").last())
      // forwarding the original argument lets path resolve it at the document's call site.
      source = if resolve == none { path(..input) } else { resolve(filename) }
      assert(type(source) == path, message: "resolve must return a Typst path.")
      if load == none {
        if resolve == none {
          resolve = name => {
            assert(not filename.split("/").contains("..") and path(filename) == source,
              message: "For linked project files, pass resolve: p => path(p) at the document call site.")
            path(name)
          }
        }
        let dir = filename.replace(regex("[^/]*$"), "")
        path-loader = true
        load = name => resolve(if name.starts-with("/") { name } else { dir + name })
      }
    } else if type(source) == content {
      assert(source.func() == raw, message: "Pass a raw code block for inline drawing source.")
      lang = source.at("lang", default: none)
      source = source.text
    }
    let resource(name) = {
      assert(load != none, message: "panfig needs " + repr(name) + ". For linked files use resolve: p => path(p), or provide a load dictionary.")
      let data = if type(load) == dictionary { load.at(name, default: none) } else { load(name) }
      if type(data) == path { read(data, encoding: none) } else { data }
    }
    let inputs = (:)
    let input = if type(source) == path { read(source, encoding: none) } else { bytes(source) }
    if format == auto {
      format = if lang == "svg" or str(input).trim().starts-with("<") or str(input).trim().starts-with("\u{feff}<") { "svg" }
        else if lang in ("cetz","typ","typst") { "cetz" } else { "pgf" }
    }
    if format in ("tex", "latex", "tikz") { format = "pgf" }
    assert(format in ("pgf", "svg"), message: "Unsupported input format " + repr(format) + ". Import PGF, TikZ or SVG. Use CeTZ canvases directly as Typst content.")
    let viewport = none
    let host-labels = (:)
    let warnings = ()
    let figure = if format == "svg" {
      let svg = json(wasm.read_svg(input, bytes("svg")))
      viewport = svg.size
      svg.layers
    } else if preamble != "" { (tex: "") } else {
      json(wasm.read_pgf(input, encode(inputs)))
    }
    while type(figure) == dictionary and "input" in figure {
      let name = figure.input
      assert(name not in inputs, message: "repeated PGF input request " + repr(name))
      let data = resource(name)
      assert(data != none, message: "Missing PGF input " + repr(name))
      inputs.insert(name, str(data))
      figure = json(wasm.read_pgf(input, encode(inputs)))
    }
    if type(figure) == dictionary and "tex" in figure {
      let prepare(data, prefix) = {
        if not data.contains("\\pgftext") and not data.contains("\\node") and not data.contains(regex("[^\\x00-\\xFF]")) { return (data, (), "") }
        let result = json(wasm.labels(bytes(data), bytes(str(prefix))))
        (result.source, result.labels, result.declarations)
      }
      let (main, labels, declarations) = prepare(str(input), 0)
      let files = (:)
      for (name, data) in inputs {
        let (source, extra, decls) = prepare(data, files.len()+1)
        declarations += decls
        labels += extra
        files.insert(name, packed(source))
      }
      let chemistry = if main.starts-with("%% Creator: Matplotlib, PGF backend") and preamble == "" and main.contains("%%   \\usepackage[version=4]{mhchem}") {
        let code = main.replace(regex("(?m)^\\s*%[^\\n]*"), "")
        let other = code.replace(regex("\\\\ce\\s*\\{(?:\\^[0-9]+)?(?:[A-Z][a-z]?[0-9]*)+\\}"), "")
        assert(not other.contains(regex("\\\\(?:ce|cf|pu|mhchemoptions)\\b")),
          message: "This figure requests mhchem v4 syntax. The bundled mhchem is v3.11. Supply a compatible preamble or render the original with a v4 TeX installation.")
        "\\usepackage[version=3]{mhchem}\n"
      } else { "" }
      let document = "\\usepackage{amsmath}\n" + chemistry + preamble + "\n\\begin{document}\n" + main + "\n\\end{document}\n"
      // preload declared resources so optional TeX probes see the caller's actual files.
      if type(load) == dictionary {
        for name in load.keys() {
          if name in files { continue }
          let data = resource(name)
          if data == none { continue }
          if lower(name).ends-with(".tex") or lower(name).ends-with(".pgf") {
            let (source, extra, decls) = prepare(str(data), files.len()+1)
            declarations += decls
            labels += extra
            files.insert(name, packed(source))
          } else { files.insert(name, packed(data)) }
        }
      }
      let job = (document: document, declarations: declarations, labels: labels, files: files)
      let result = compile(job)
      while "error" in result {
        let pending = result.missing.filter(n => n not in job.files)
        assert(load != none and pending.len() > 0 and type(load) != dictionary, message: result.error)
        assert(pending.len() == 1 or (not path-loader and type(load) == function), message: result.error + "\nProvide a load dictionary for extensionless images.")
        let found = false
        for name in pending {
          let data = resource(name)
          if data == none { continue }
          if name.ends-with(".pgf") or name.ends-with(".tex") {
            let (source, extra, decls) = prepare(str(data), job.files.len()+1)
            job.declarations += decls
            job.labels += extra
            job.files.insert(name, packed(source))
          } else { job.files.insert(name, packed(data)) }
          found = true
          break
        }
        assert(found, message: result.error)
        result = compile(job)
      }
      host-labels = job.labels.to-dict()
      warnings = result.at("warnings", default: ())
      let svg = json(wasm.read_svg(bytes(result.svg), bytes("tex")))
      viewport = svg.size
      figure = svg.layers
    }

    let entries = ()
    for layer in figure {
      if layer.kind == "curve" {
        entries.push(positioned(layer.at, layer.size, native-curve(layer)))
      } else if layer.kind == "vector" {
        let (w, h) = point(layer.size)
        entries.push(positioned(layer.at, layer.size, image(bytes(layer.svg), format: "svg", width: w, height: h)))
      } else if layer.kind == "host-text" {
        let item = host-labels.at(layer.id)
        let body = run-text(item.runs, item.at("font", default: none), item.at("size", default: none))
        if layer.color != (0, 0, 0, 255) { body = text(fill: rgb(..layer.color), body) }
        entries.push(svg-text(layer, body, "base-west"))
      } else if layer.kind == "text" {
        let body = run-text(layer.runs, layer.font, layer.size)
        entries.push(svg-text(layer, body, layer.anchor))
      } else if layer.kind == "image" {
        let name = layer.file
        if not name.split("/").last().contains(".") {
          assert(type(load) == dictionary, message: "Give the image an extension or include it in the load dictionary: " + name)
          name = (".png", ".jpg", ".jpeg", ".svg").map(ext => name + ext).find(n => n in load)
          assert(name != none, message: "Missing image " + layer.file)
        }
        let data = resource(name)
        assert(data != none, message: "Missing image " + name)
        let data = bytes(data)
        let (w, h) = json(wasm.image_size(data, bytes(name)))
        let width = layer.width
        let height = layer.height
        if width == none { width = if height == none { w } else { height * w / h } }
        if height == none { height = width * h / w }
        let body = image(data, width: width * 1pt, height: height * 1pt, fit: "stretch")
        entries.push((..bounds(layer.at, layer.anchor, width * 1pt, height * 1pt, height * 1pt), body: body))
      }
    }
    let figure = canvas(entries, viewport: viewport)
    if warnings.len() == 0 { return figure }
    // surface missing TeX fonts as native Typst warnings without altering figure geometry.
    box({
      for font in warnings { place(hide(text(font: font)[a])) }
      figure
    })
  }
  import-figure
}
