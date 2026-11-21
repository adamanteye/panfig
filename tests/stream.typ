#import "../lib.typ": panfig

#let source = read("figure.pgf")
#let from-file = panfig("figure.pgf")
#assert.eq(type(from-file), content)
#context {
  let size = measure(from-file)
  let absolute = source.replace(
    "\\pgfpointanchor{tip}{center}",
    "\\pgfqpoint{30pt}{10pt}",
  )
  assert.eq(size, measure(panfig(absolute)))
  for data in (
    source,
    bytes(source),
    raw(source, lang: "pgf"),
    path("figure.pgf"),
  ) {
    assert.eq(measure(panfig(data)), size)
  }
  assert(size.width > 30pt and size.height > 10pt)
}

#show curve: it => {
  metadata("curve")
  it
}
#show "峰值": it => {
  metadata("text")
  it
}
#from-file
#context assert.eq(query(metadata).map(it => it.value), ("curve", "text"))
