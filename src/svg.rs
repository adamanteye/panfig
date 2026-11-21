//! lift representable SVG elements while preserving the viewport and the paint order of SVG slices.
use super::drawing::{Dash, FillRule, LineCap, LineJoin, Result, Stroke, Style};
use super::render;
use kurbo::{Affine, BezPath, Circle, Rect, Shape};
use roxmltree::Node;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use svgtypes::{Color, Transform, ViewBox};

type Element<'a> = Node<'a, 'a>;

fn opening<'a>(source: &'a str, n: Element) -> &'a str {
    // an attribute value may contain a literal '>'.
    let tag = &source[n.range()];
    let mut quote = None;
    for (i, c) in tag.char_indices() {
        match (quote, c) {
            (Some(q), _) if q == c => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return &tag[..=i],
            _ => (),
        }
    }
    unreachable!("parsed XML element has an opening tag")
}

fn closing(open: &str) -> String {
    let name = open[1..].split([' ', '\t', '\r', '\n', '/', '>']).next().unwrap();
    format!("</{name}>")
}

fn number(s: &str) -> Option<f64> {
    s.trim().parse().ok().filter(|v: &f64| v.is_finite())
}

fn length(s: &str) -> Option<f64> {
    let len: svgtypes::Length = s.trim().parse().ok()?;
    use svgtypes::LengthUnit::*;
    let factor = match len.unit {
        None | Px => 1.,
        Pt => 96. / 72.,
        Pc => 16.,
        In => 96.,
        Cm => 96. / 2.54,
        Mm => 96. / 25.4,
        _ => return Option::None,
    };
    Some(len.number * factor)
}

#[derive(Clone, Default)]
struct Paint<'a> {
    matrix: Affine,
    props: HashMap<&'a str, &'a str>,
}

impl<'a> Paint<'a> {
    fn get(&self, key: &str, default: &'a str) -> &'a str {
        self.props.get(key).copied().unwrap_or(default)
    }

    fn apply(&mut self, node: Element<'a>, geometry: &[&str]) -> Option<()> {
        let mut property = |k: &'a str, v: &'a str| {
            match k {
                "color" | "fill" | "fill-opacity" | "fill-rule" | "stroke"
                | "stroke-width" | "stroke-linejoin" | "stroke-linecap"
                | "stroke-miterlimit" | "stroke-opacity" | "stroke-dasharray"
                | "stroke-dashoffset" | "font-family" | "font-size" | "font-weight"
                | "font-style" | "text-anchor" | "space" => {
                    self.props.insert(k, v);
                }
                _ => return None,
            }
            Some(())
        };
        for a in node.attributes() {
            // editor metadata does not participate in SVG painting.
            if matches!(
                a.namespace(),
                Some(
                    "http://www.inkscape.org/namespaces/inkscape"
                        | "http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd"
                )
            ) {
                continue;
            }
            match a.name() {
                "id" | "style" => (),
                "transform" => {
                    let t: Transform = a.value().parse().ok()?;
                    self.matrix *= Affine::new([t.a, t.b, t.c, t.d, t.e, t.f]);
                }
                k if geometry.contains(&k) => (),
                k => property(k, a.value())?,
            }
        }
        for decl in node
            .attribute("style")
            .unwrap_or("")
            .split(';')
            .filter(|s| !s.trim().is_empty())
        {
            let (k, v) = decl.split_once(':')?;
            property(k.trim(), v.trim())?;
        }
        Some(())
    }

    fn color(&self, key: &str, default: &'a str) -> Option<Color> {
        let s = self.get(key, default);
        (if s == "currentColor" { self.get("color", "black") } else { s })
            .parse()
            .ok()
    }

    fn style(&self) -> Option<Style> {
        let alpha = |key| number(self.get(key, "1")).map(|v| v.clamp(0., 1.));
        let mut fill_opacity = alpha("fill-opacity")?;
        let fill = if self.get("fill", "black") == "none" {
            None
        } else {
            let c = self.color("fill", "black")?;
            fill_opacity *= f64::from(c.alpha) / 255.;
            Some([c.red, c.green, c.blue])
        };
        let stroke = if self.get("stroke", "none") == "none" {
            None
        } else {
            let scale = uniform_scale(self.matrix)?;
            let c = self.color("stroke", "none")?;
            let width = length(self.get("stroke-width", "1"))? * scale;
            if width < 0. {
                return None;
            }
            let mut s = Stroke::black(width);
            s.color = [c.red, c.green, c.blue];
            s.opacity = alpha("stroke-opacity")? * f64::from(c.alpha) / 255.;
            s.cap = match self.get("stroke-linecap", "butt") {
                "butt" => LineCap::Butt,
                "round" => LineCap::Round,
                "square" => LineCap::Square,
                _ => return None,
            };
            s.join = match self.get("stroke-linejoin", "miter") {
                "miter" => LineJoin::Miter,
                "round" => LineJoin::Round,
                "bevel" => LineJoin::Bevel,
                _ => return None,
            };
            s.miter = number(self.get("stroke-miterlimit", "4"))?;
            if s.miter < 1. {
                return None;
            }
            if self.get("stroke-dasharray", "none") != "none" {
                let array = self
                    .get("stroke-dasharray", "")
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|v| !v.is_empty())
                    .map(|v| length(v).map(|n| n * scale))
                    .collect::<Option<Vec<_>>>()?;
                if array.is_empty() || array.iter().any(|n| *n < 0.) {
                    return None;
                }
                if array.iter().any(|n| *n > 0.) {
                    s.dash = Some(Dash {
                        array,
                        phase: length(self.get("stroke-dashoffset", "0"))? * scale,
                    });
                }
            }
            Some(s)
        };
        let fill_rule = match self.get("fill-rule", "nonzero") {
            "evenodd" => FillRule::EvenOdd,
            "nonzero" => FillRule::NonZero,
            _ => return None,
        };
        Some(Style { fill, stroke, fill_rule, fill_opacity })
    }
}

fn uniform_scale(matrix: Affine) -> Option<f64> {
    let [a, b, c, d, _, _] = matrix.as_coeffs();
    let scale = a.hypot(b);
    (scale > 0.
        && (scale - c.hypot(d)).abs() < scale * 1e-8
        && (a * c + b * d).abs() < scale * scale * 1e-8)
        .then_some(scale)
}

fn geometry<'a>(n: Element<'a>, mut paint: Paint<'a>) -> Option<Value> {
    let dim = |key| length(n.attribute(key).unwrap_or("0"));
    let path = match n.tag_name().name() {
        "path" => {
            paint.apply(n, &["d"])?;
            let mut p = BezPath::new();
            for el in svgtypes::SimplifyingPathParser::from(n.attribute("d")?) {
                use svgtypes::SimplePathSegment::*;
                match el.ok()? {
                    MoveTo { x, y } => p.move_to((x, y)),
                    LineTo { x, y } => p.line_to((x, y)),
                    Quadratic { x1, y1, x, y } => p.quad_to((x1, y1), (x, y)),
                    CurveTo { x1, y1, x2, y2, x, y } => {
                        p.curve_to((x1, y1), (x2, y2), (x, y))
                    }
                    ClosePath => p.close_path(),
                }
                if p.elements().len() > render::NATIVE_SEGMENTS {
                    return None;
                }
            }
            p
        }
        "line" => {
            paint.apply(n, &["x1", "y1", "x2", "y2"])?;
            let mut p = BezPath::new();
            p.move_to((dim("x1")?, dim("y1")?));
            p.line_to((dim("x2")?, dim("y2")?));
            p
        }
        "rect" => {
            paint.apply(n, &["x", "y", "width", "height"])?;
            let (x, y, w, h) = (dim("x")?, dim("y")?, dim("width")?, dim("height")?);
            if w <= 0. || h <= 0. {
                return None;
            }
            Rect::new(x, y, x + w, y + h).to_path(0.001)
        }
        "circle" => {
            paint.apply(n, &["cx", "cy", "r"])?;
            let r = dim("r")?;
            if r <= 0. {
                return None;
            }
            Circle::new((dim("cx")?, dim("cy")?), r).to_path(0.001)
        }
        _ => return None,
    };
    if n.children().any(|n| n.is_element())
        || path.is_empty()
        || path.elements().len() > render::NATIVE_SEGMENTS
    {
        return None;
    }
    Some(render::native(&(paint.matrix * path), &paint.style()?))
}

fn text_layer<'a>(n: Element<'a>, mut paint: Paint<'a>, tex: bool) -> Option<Value> {
    paint.apply(n, &["x", "y", "data-panfig-label"])?;
    // a positioned tspan is common in single-line Inkscape labels.
    let span = n.first_element_child().filter(|c| {
        c.has_tag_name("tspan")
            && n.last_element_child() == Some(*c)
            && n.children()
                .all(|s| !s.is_text() || s.text().unwrap_or("").trim().is_empty())
    });
    let marker = n.attribute("data-panfig-label").filter(|_| tex);
    let span = span.filter(|_| marker.is_none());
    if let Some(span) = span {
        if span.has_attribute("transform") {
            return None;
        }
        paint.apply(span, &["x", "y"])?;
    }
    let coord = |key| {
        length(
            span.and_then(|s| s.attribute(key))
                .or_else(|| n.attribute(key))
                .unwrap_or("0"),
        )
    };
    let (x, y) = (coord("x")?, coord("y")?);
    let [a, b, c, d, _, _] = paint.matrix.as_coeffs();
    let (sx, sy) = (a.hypot(b), c.hypot(d));
    if sx <= 0.
        || sy <= 0.
        || (a * c + b * d).abs() > sx * sy * 1e-8
        || paint.get("stroke", "none") != "none"
    {
        return None;
    }
    let sy = sy.copysign(a * d - b * c);
    let angle = b.atan2(a).to_degrees();
    let pt = paint.matrix * kurbo::Point::new(x, y);
    let at = [pt.x, -pt.y];
    let c = paint.color("fill", "black")?;
    let alpha =
        f64::from(c.alpha) * number(paint.get("fill-opacity", "1"))?.clamp(0., 1.);
    let color = [c.red, c.green, c.blue, alpha.round() as u8];
    if let Some(id) = marker {
        // TeX rounds image sizes to scaled points before emitting the placement.
        let snap = |s: f64| if (s - 1.).abs() < 0.001 { 1. } else { s };
        return Some(
            json!({"kind":"host-text","id":id,"at":at,"color":color,"scale":[snap(sx),snap(sy)],"angle":angle}),
        );
    }
    let n = span.unwrap_or(n);
    if n.children().any(|n| n.is_element()) {
        return None;
    }
    let raw: String = n
        .children()
        .filter(|c| c.is_text())
        .filter_map(|c| c.text())
        .collect();
    let text = if paint.get("space", "default") == "preserve" {
        raw.replace(['\t', '\r', '\n'], " ")
    } else {
        raw.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
    };
    let anchor = match paint.get("text-anchor", "start") {
        "start" => "base-west",
        "middle" => "base",
        "end" => "base-east",
        _ => return None,
    };
    let bold = match paint.get("font-weight", "normal") {
        "bold" | "700" => true,
        "normal" | "400" => false,
        _ => return None,
    };
    let italic = match paint.get("font-style", "normal") {
        "italic" => true,
        "normal" => false,
        _ => return None,
    };
    let size = length(paint.get("font-size", "16"))? * sx;
    if size <= 0. {
        return None;
    }
    let font = paint.props.get("font-family").map(|s| {
        s.split(',')
            .map(|s| s.trim().trim_matches(['\'', '"']))
            .collect::<Vec<_>>()
    });
    Some(
        json!({"kind":"text","at":at,"anchor":anchor,"scale":[1.,sy/sx],"angle":angle,"runs":[{"text":text,"bold":bold,"italic":italic,"fill":color}],"size":size,"font":font}),
    )
}

fn is_viewport_clip(n: Element, matrix: Affine, size: [f64; 2]) -> bool {
    let mut children = n.children().filter(Node::is_element);
    let Some(child) = children.next() else {
        return false;
    };
    if children.next().is_some() || child.attribute("transform").is_some() {
        return false;
    }
    let path = match child.tag_name().name() {
        "path" => {
            let Some(d) = child.attribute("d") else {
                return false;
            };
            let mut p = BezPath::new();
            for el in svgtypes::SimplifyingPathParser::from(d) {
                use svgtypes::SimplePathSegment::*;
                match el {
                    Ok(MoveTo { x, y }) => p.move_to((x, y)),
                    Ok(LineTo { x, y }) => p.line_to((x, y)),
                    Ok(ClosePath) => p.close_path(),
                    _ => return false,
                }
            }
            p
        }
        "rect" => {
            let dim = |k| length(child.attribute(k).unwrap_or("0"));
            let (Some(x), Some(y), Some(w), Some(h)) =
                (dim("x"), dim("y"), dim("width"), dim("height"))
            else {
                return false;
            };
            Rect::new(x, y, x + w, y + h).to_path(0.001)
        }
        _ => return false,
    };
    let path = matrix * path;
    let b = path.bounding_box();
    let eps = 0.5;
    if !(b.x0 <= eps && b.y0 <= eps && b.x1 >= size[0] - eps && b.y1 >= size[1] - eps) {
        return false;
    }
    // ensure every vertex lies on the outer rectangle corners (not a diamond or polygon).
    let on_corner = |p: kurbo::Point| {
        ((p.x - b.x0).abs() <= eps || (p.x - b.x1).abs() <= eps)
            && ((p.y - b.y0).abs() <= eps || (p.y - b.y1).abs() <= eps)
    };
    let mut count = 0usize;
    for el in path.elements() {
        match *el {
            kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => {
                if !on_corner(p) {
                    return false;
                }
                count += 1;
            }
            kurbo::PathEl::ClosePath => {}
            _ => return false,
        }
    }
    (4..=5).contains(&count)
}

// consecutive fallback elements share ancestor wrappers and the original definitions.
struct Slices<'a> {
    source: &'a str,
    root: Element<'a>,
    defs: String,
    viewport_clips: HashSet<&'a str>,
    groups: Vec<Element<'a>>,
    body: String,
    layers: Vec<Value>,
    size: [f64; 2],
}

impl<'a> Slices<'a> {
    fn groups(&mut self, groups: &[Element<'a>]) {
        let common = groups.iter().zip(&self.groups).take_while(|(a, b)| a == b).count();
        for old in self.groups.drain(common..).rev() {
            self.body.push_str(&closing(opening(self.source, old)));
        }
        for &g in &groups[common..] {
            self.body.push_str(opening(self.source, g));
            self.groups.push(g);
        }
    }
    fn fallback(&mut self, groups: &[Element<'a>], n: Element) {
        self.groups(groups);
        self.body.push_str(&self.source[n.range()]);
    }
    fn flush(&mut self) {
        if self.body.is_empty() {
            return;
        }
        self.groups(&[]);
        let root = opening(self.source, self.root);
        let svg = format!("{}{}{}{}", root, self.defs, self.body, closing(root));
        self.layers
            .push(json!({"kind":"vector","at":[0.,0.],"size":self.size,"svg":svg}));
        self.body.clear();
    }
    fn walk(
        &mut self,
        n: Element<'a>,
        groups: &mut Vec<Element<'a>>,
        paint: &Paint<'a>,
        dense: bool,
        tex: bool,
    ) {
        if n.has_tag_name("clipPath") {
            if let Some(id) = n.attribute("id") {
                if is_viewport_clip(n, paint.matrix, self.size) {
                    self.viewport_clips.insert(id);
                }
            }
            for &g in groups.iter() {
                self.defs.push_str(opening(self.source, g));
            }
            self.defs.push_str(&self.source[n.range()]);
            for &g in groups.iter().rev() {
                self.defs.push_str(&closing(opening(self.source, g)));
            }
            return;
        }
        if n.has_tag_name("g") && groups.len() < 128 {
            let mut next = paint.clone();
            let skip_clip = n
                .attribute("clip-path")
                .and_then(|v| v.trim().strip_prefix("url(#")?.strip_suffix(')'))
                .is_some_and(|id| self.viewport_clips.contains(id));
            let ignore: &[&str] = if skip_clip { &["clip-path"] } else { &[] };
            if next.apply(n, ignore).is_some() {
                groups.push(n);
                for child in n.children().filter(Node::is_element) {
                    self.walk(child, groups, &next, dense, tex);
                }
                groups.pop();
                return;
            }
        }
        let native = if n.has_tag_name("text") {
            text_layer(n, paint.clone(), tex)
        } else if dense {
            None
        } else {
            geometry(n, paint.clone())
        };
        if let Some(layer) = native {
            self.flush();
            self.layers.push(layer);
        } else {
            self.fallback(groups, n);
        }
    }
}

pub fn read(source: &str, tex: bool) -> Result<Vec<u8>> {
    let doc = roxmltree::Document::parse_with_options(
        source,
        roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() },
    )
    .map_err(|e| format!("invalid SVG: {e}"))?;
    let root = doc.root_element();
    if !root.has_tag_name("svg") {
        return Err("expected an SVG root element".into());
    }
    let vb = root
        .attribute("viewBox")
        .map(str::parse::<ViewBox>)
        .transpose()
        .map_err(|_| "invalid SVG viewBox")?;
    let physical = |key| root.attribute(key).and_then(length).map(|v| v * 0.75);
    let (w, h) = match (physical("width"), physical("height"), vb.as_ref()) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(v)) => (w, w * v.h / v.w),
        (None, Some(h), Some(v)) => (h * v.w / v.h, h),
        (None, None, Some(v)) => (v.w * 0.75, v.h * 0.75),
        (w, h, None) => (w.unwrap_or(225.), h.unwrap_or(112.5)),
    };
    if ![w, h].iter().all(|v| v.is_finite() && *v > 0.) {
        return Err("invalid intrinsic dimensions for SVG".into());
    }
    let size = [w, h];
    let whole = || {
        super::json(
            &json!({"size":size,"layers":[{"kind":"vector","at":[0,0],"size":size,"svg":source}]}),
        )
    };
    let mut paint = Paint::default();
    // preserve global CSS, entity declarations and references to painted elements as one SVG.
    let defs: Vec<_> = root.children().filter(|n| n.has_tag_name("defs")).collect();
    let ids: HashSet<_> = defs
        .iter()
        .flat_map(|d| d.descendants())
        .filter_map(|n| n.attribute("id"))
        .collect();
    let mut count = 0;
    for n in root.descendants().filter(Node::is_element) {
        if matches!(n.tag_name().name(), "path" | "line" | "rect" | "circle") {
            count += 1
        }
        if (matches!(
            n.tag_name().name(),
            "mask"
                | "linearGradient"
                | "radialGradient"
                | "filter"
                | "pattern"
                | "marker"
                | "defs"
        ) && !defs.iter().any(|d| n.ancestors().any(|p| p == *d)))
            || n.has_tag_name("style")
            || n.attributes().any(|a| {
                a.name() == "href"
                    && a.value().strip_prefix('#').is_some_and(|id| !ids.contains(id))
            })
        {
            return whole();
        }
    }
    if source.contains("<!ENTITY")
        || source.contains("mix-blend-mode")
        || source.contains("BackgroundImage")
        || source.contains("BackgroundAlpha")
        || paint
            .apply(
                root,
                &["width", "height", "viewBox", "version", "preserveAspectRatio"],
            )
            .is_none()
        || root.attribute("transform").is_some()
    {
        return whole();
    }
    let matrix = if let Some(v) = vb {
        let (sx, sy) = (w / v.w, h / v.h);
        let align = root.attribute("preserveAspectRatio").unwrap_or("xMidYMid meet");
        let (sx, sy) = match align.trim() {
            "none" => (sx, sy),
            "xMidYMid" | "xMidYMid meet" => (sx.min(sy), sx.min(sy)),
            _ => return whole(),
        };
        Affine::new([
            sx,
            0.,
            0.,
            sy,
            (w - v.w * sx) / 2. - v.x * sx,
            (h - v.h * sy) / 2. - v.y * sy,
        ])
    } else {
        Affine::scale(0.75)
    };
    paint.matrix = matrix;
    let mut slices = Slices {
        source,
        root,
        size,
        defs: defs.iter().map(|d| &source[d.range()]).collect(),
        viewport_clips: HashSet::new(),
        groups: Vec::new(),
        body: String::new(),
        layers: Vec::new(),
    };
    for n in root.children().filter(Node::is_element) {
        if n.tag_name().namespace() == root.tag_name().namespace()
            && !matches!(n.tag_name().name(), "defs" | "metadata" | "title" | "desc")
        {
            slices.walk(n, &mut Vec::new(), &paint, count > render::NATIVE_PATHS, tex);
        }
    }
    slices.flush();
    super::json(&json!({"size":size,"layers":slices.layers}))
}
