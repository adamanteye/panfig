//! fast-path interpreter for static PGF pictures and Matplotlib PGF output.
use super::drawing::*;
use super::labels;
use super::pgf::{Argument, Event, Parser};
use kurbo::{Affine, BezPath, Circle, PathEl, Rect, Shape};
use std::collections::HashMap;
use std::rc::Rc;

fn opacity(s: &str) -> Result<f64> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| (0.0..=1.).contains(v))
        .ok_or_else(|| format!("invalid opacity {s:?}"))
}

pub(super) fn scalar(s: &str) -> Result<f64> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("expected a finite number in {s:?}"))
}

pub(super) fn dimension(s: &str) -> Result<f64> {
    let s = s.trim();
    for (unit, factor) in [
        ("bp", 1.),
        ("pt", TEX_PT),
        ("cm", CM),
        ("mm", CM / 10.),
        ("in", 72.),
        ("sp", TEX_PT / 65536.),
    ] {
        if let Some(n) = s.strip_suffix(unit) {
            if let Ok(n) = n.trim_end().parse::<f64>() {
                if (n * factor).is_finite() {
                    return Ok(n * factor);
                }
            }
        }
    }
    Err(format!("expected an absolute PGF dimension, got {s:?}"))
}

pub(super) fn point_command(s: &str) -> Result<(&str, Vec<Argument<'_>>)> {
    let mut parser = Parser::new(s);
    if let Some(Event::Command { name, args }) = parser.next_event()? {
        if parser.next_event()?.is_none() {
            return Ok((name, args));
        }
    }
    Err(format!("expected one PGF point in {s:?}"))
}

fn absolute(name: &str, args: &[Argument<'_>]) -> Result<Point> {
    use Argument::Required as R;
    match (name, args) {
        ("pgfpointorigin", []) => Ok([0.; 2]),
        ("pgfpoint" | "pgfqpoint", [R(x), R(y)]) => Ok([dimension(x)?, dimension(y)?]),
        _ => Err(format!("unsupported PGF point command {name:?}")),
    }
}

pub(super) fn point(s: &str) -> Result<Point> {
    // literal points account for nearly every argument in a Matplotlib path.
    if let Some((x, y)) = s
        .strip_prefix(r"\pgfqpoint{")
        .or_else(|| s.strip_prefix(r"\pgfpoint{"))
        .and_then(|s| s.strip_suffix('}'))
        .and_then(|s| s.split_once("}{"))
    {
        return Ok([dimension(x)?, dimension(y)?]);
    }
    let (name, args) = point_command(s)?;
    absolute(name, &args)
}

/// read a label position expressed in absolute or named coordinates.
pub(super) fn position(s: &str) -> Result<Position> {
    use Argument::Required as R;
    let (name, args) = point_command(s)?;
    match (name, args.as_slice()) {
        ("pgfpointanchor", [R(to), R("center")]) => {
            Ok(Position::Relative { to: to.to_string(), offset: [0.; 2] })
        }
        ("pgfpointadd", [R(base), R(delta)]) => {
            let (base_name, base_args) = point_command(base)?;
            match (base_name, base_args.as_slice()) {
                ("pgfpointanchor", [R(to), R("center")]) => {
                    Ok(Position::Relative { to: to.to_string(), offset: point(delta)? })
                }
                _ => Err("expected a named coordinate plus an absolute offset".into()),
            }
        }
        _ => absolute(name, &args).map(Position::Absolute),
    }
}

#[derive(Clone)]
struct State {
    transform: Affine,
    clips: Vec<Rc<Clip>>,
    viewport_clip: bool,
    stroke: Stroke,
    fill: Color,
    fill_opacity: f64,
    fill_rule: FillRule,
    colors: HashMap<String, Color>,
}

pub(super) fn resolve_color(
    colors: Option<&HashMap<String, Color>>,
    name: &str,
) -> Result<Color> {
    if let Some(c) = colors.and_then(|map| map.get(name)) {
        return Ok(*c);
    }
    let rgb = match name {
        "black" => BLACK,
        "white" => [255; 3],
        "red" => [255, 0, 0],
        "green" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "cyan" => [0, 255, 255],
        "magenta" => [255, 0, 255],
        "yellow" => [255, 255, 0],
        "gray" => [128; 3],
        _ => return Err(format!("unknown PGF color {name:?}")),
    };
    Ok(rgb)
}

fn define_color(model: &str, s: &str) -> Result<Color> {
    if model == "HTML" {
        return parse_hex(s);
    }
    if model == "gray" {
        let g = s
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| (0.0..=1.0).contains(v))
            .map(|n| (255. * n).round() as u8)
            .ok_or_else(|| format!("invalid gray component {s:?}"))?;
        return Ok([g; 3]);
    }
    let comps: Vec<_> = s.split(',').map(str::trim).collect();
    let [r, g, b] = comps[..] else {
        return Err(format!("expected three color components in {s:?}"));
    };
    let channel = |c: &str| match model {
        "RGB" => c.parse::<u8>().map_err(|_| format!("invalid RGB component {c}")),
        "rgb" => c
            .parse::<f64>()
            .ok()
            .filter(|c| (0.0..=1.0).contains(c))
            .map(|n| (255. * n).round() as u8)
            .ok_or_else(|| format!("invalid rgb component {c}")),
        _ => Err(format!("unsupported PGF color model {model}")),
    };
    Ok([channel(r)?, channel(g)?, channel(b)?])
}

fn vertex(s: &str) -> Result<kurbo::Point> {
    let [x, y] = point(s)?;
    Ok(kurbo::Point::new(x, y))
}

fn path_command(path: &mut BezPath, name: &str, args: &[Argument<'_>]) -> Result<bool> {
    use Argument::Required as R;
    match (name, args) {
        ("pgfpathmoveto", [R(p)]) => path.move_to(vertex(p)?),
        ("pgfpathrectangle" | "pgfpathrectanglecorners", [R(a), R(b)]) => {
            let a = vertex(a)?;
            let mut b = vertex(b)?;
            if name == "pgfpathrectangle" {
                b += a.to_vec2();
            }
            path.move_to(a);
            path.line_to((b.x, a.y));
            path.line_to(b);
            path.line_to((a.x, b.y));
            path.close_path();
        }
        ("pgfpathcircle", [R(at), R(radius)]) => {
            let radius = dimension(radius)?;
            if radius <= 0. {
                return Err("expected a positive circle radius".into());
            }
            path.extend(Circle::new(vertex(at)?, radius).path_elements(0.001));
        }
        _ if !name.starts_with("pgfpath") => return Ok(false),
        _ => {
            if matches!(path.elements().last(), None | Some(PathEl::ClosePath)) {
                return Err("expected an open PGF path".into());
            }
            match (name, args) {
                ("pgfpathlineto", [R(p)]) => path.line_to(vertex(p)?),
                ("pgfpathquadraticcurveto", [R(a), R(b)]) => {
                    path.quad_to(vertex(a)?, vertex(b)?)
                }
                ("pgfpathcurveto", [R(a), R(b), R(c)]) => {
                    path.curve_to(vertex(a)?, vertex(b)?, vertex(c)?)
                }
                ("pgfpathclose", [])
                    if !matches!(path.elements().last(), Some(PathEl::MoveTo(_))) =>
                {
                    path.close_path()
                }
                _ => return Err(format!("unsupported PGF path command {name}")),
            }
        }
    }
    Ok(true)
}

#[derive(Clone, Copy, Default)]
struct Actions {
    fill: bool,
    stroke: bool,
    clip: bool,
    bounding_box: bool,
}

impl Actions {
    fn parse(source: &str) -> Result<Self> {
        let mut actions = Self::default();
        for key in source.split(',').map(str::trim) {
            match key {
                "stroke" | "draw" => actions.stroke = true,
                "fill" => actions.fill = true,
                "clip" => actions.clip = true,
                "use as bounding box" => actions.bounding_box = true,
                "discard" | "" => {}
                _ => return Err(format!("unsupported PGF path action {key:?}")),
            }
        }
        Ok(actions)
    }
}

type Object = Rc<[(Rc<BezPath>, Actions)]>;

// a marker records geometry once. its fill and pen are inherited at each use.
fn object(body: &str) -> Result<Object> {
    let mut parser = Parser::new(body);
    let mut path = BezPath::new();
    let mut paths = Vec::new();
    while let Some(event) = parser.next_event()? {
        match event {
            Event::Command { name: "pgfusepath", args } => {
                let [Argument::Required(actions)] = args.as_slice() else {
                    return Err("invalid object path actions".into());
                };
                let actions = Actions::parse(actions)?;
                if actions.bounding_box {
                    return Err("bounding box in a static object".into());
                }
                paths.push((Rc::new(std::mem::take(&mut path)), actions));
            }
            Event::Command { name, args } if path_command(&mut path, name, &args)? => {}
            _ => return Err("unsupported static object body".into()),
        }
    }
    if !path.elements().is_empty() {
        return Err("unfinished object path".into());
    }
    Ok(paths.into())
}

fn rectangle(path: &BezPath) -> Option<Rect> {
    let [PathEl::MoveTo(a), PathEl::LineTo(b), PathEl::LineTo(c), PathEl::LineTo(d), PathEl::ClosePath] =
        path.elements()
    else {
        return None;
    };
    ((a.x == b.x && b.y == c.y && c.x == d.x && d.y == a.y)
        || (a.y == b.y && b.x == c.x && c.y == d.y && d.x == a.x))
        .then(|| Rect::from_points(*a, *c))
}

struct Reader {
    figure: Figure,
    path: BezPath,
    objects: HashMap<String, Object>,
    state: State,
    scopes: Vec<(String, State)>,
}

impl Reader {
    fn open(&mut self, name: &str) -> Result<()> {
        if !self.path.elements().is_empty() {
            return Err("scope change inside an unfinished path".into());
        }
        self.scopes.push((name.into(), self.state.clone()));
        Ok(())
    }

    fn close(&mut self, name: &str) -> Result<()> {
        if !self.path.elements().is_empty() {
            return Err("scope ends with an unfinished path".into());
        }
        let (open, state) =
            self.scopes.pop().ok_or_else(|| format!("unexpected end of {name}"))?;
        if open != name {
            return Err(format!("expected end of {open}, got {name}"));
        }
        self.state = state;
        Ok(())
    }

    fn place(&mut self, item: Item, angle: f64) -> Result<()> {
        if self.figure.viewport.is_some() && !self.state.viewport_clip {
            return Err("drawing outside the bounding box scope".into());
        }
        self.figure.items.push(Placed {
            item,
            transform: self.state.transform,
            angle,
            clips: self.state.clips.clone(),
        });
        Ok(())
    }

    fn use_path(&mut self, path: Rc<BezPath>, actions: Actions) -> Result<()> {
        let style = Style {
            stroke: actions.stroke.then(|| self.state.stroke.clone()),
            fill: actions.fill.then_some(self.state.fill),
            fill_opacity: self.state.fill_opacity,
            fill_rule: self.state.fill_rule,
        };
        let painted = actions.fill || actions.stroke;
        if !actions.clip && !actions.bounding_box {
            if !path.is_empty() {
                self.place(Item::Path { path, style }, 0.)?;
            }
            return Ok(());
        }
        if !path.is_finite() || path.is_empty() {
            return Err("invalid clipping path".into());
        }
        let world = self.state.transform * path.as_ref();
        if !world.is_finite() {
            return Err("non-finite transformed clip".into());
        }
        let rect = rectangle(&world);
        if actions.bounding_box {
            if !actions.clip
                || !self.figure.items.is_empty()
                || self.figure.viewport.is_some()
                || painted
            {
                return Err(
                    "expected an initial rectangular bounding box and clip".into()
                );
            }
            self.figure.viewport = Some(
                rect.filter(|r| !r.is_zero_area())
                    .ok_or("expected a rectangular viewport")?,
            );
            self.state.viewport_clip = true;
        } else {
            if painted {
                self.place(Item::Path { path, style }, 0.)?;
            }
            if self.figure.viewport.is_none() && self.state.clips.is_empty() {
                self.place(Item::Bounds(world.bounding_box()), 0.)?;
            }
            self.state.clips.push(Rc::new(Clip {
                path: world,
                rule: self.state.fill_rule,
                rectangle: rect,
            }));
        }
        Ok(())
    }

    fn command(&mut self, name: &str, args: &[Argument<'_>]) -> Result<()> {
        use Argument::{Optional as O, Required as R};
        match (name, args) {
            ("makeatletter" | "makeatother", []) => {}
            ("begin", [R(env @ ("pgfpicture" | "pgfscope"))]) => self.open(env)?,
            ("end", [R(env @ ("pgfpicture" | "pgfscope"))]) => self.close(env)?,
            ("begingroup" | "pgfpicture" | "pgfscope" | "pgfsys@beginscope", []) => {
                self.open(name)?
            }
            ("pgfsys@endscope", []) => self.close("pgfsys@beginscope")?,
            ("endgroup", []) => self.close("begingroup")?,
            ("endpgfpicture" | "endpgfscope", []) => self.close(&name[3..])?,
            ("definecolor", [R(key), R(model), R(spec)]) => {
                self.state.colors.insert(key.to_string(), define_color(model, spec)?);
            }
            ("pgfseteorule", []) => self.state.fill_rule = FillRule::EvenOdd,
            ("pgfsetnonzerorule", []) => self.state.fill_rule = FillRule::NonZero,
            ("pgfsetbuttcap", []) => self.state.stroke.cap = LineCap::Butt,
            ("pgfsetroundcap", []) => self.state.stroke.cap = LineCap::Round,
            ("pgfsetrectcap", []) => self.state.stroke.cap = LineCap::Square,
            ("pgfsetmiterjoin", []) => self.state.stroke.join = LineJoin::Miter,
            ("pgfsetroundjoin", []) => self.state.stroke.join = LineJoin::Round,
            ("pgfsetbeveljoin", []) => self.state.stroke.join = LineJoin::Bevel,
            ("pgfsetstrokeopacity", [R(n)]) => self.state.stroke.opacity = opacity(n)?,
            ("pgfsetfillopacity", [R(n)]) => self.state.fill_opacity = opacity(n)?,
            ("pgfsetdash", [R(pattern), R(phase)]) => {
                let mut array = Vec::new();
                let mut rest = pattern.trim();
                while !rest.is_empty() {
                    let (n, tail) = rest
                        .strip_prefix('{')
                        .and_then(|s| s.split_once('}'))
                        .ok_or("invalid PGF dash pattern")?;
                    array.push(dimension(n)?);
                    rest = tail.trim_start();
                }
                let phase = dimension(phase)?;
                self.state.stroke.dash =
                    (!array.is_empty()).then_some(Dash { array, phase });
            }
            ("pgfsetlinewidth", [R(v)]) => self.state.stroke.width = dimension(v)?,
            ("pgfsetmiterlimit", [R(v)]) => {
                let miter = scalar(v)?;
                if miter < 1. {
                    return Err("expected a miter limit >= 1".into());
                }
                self.state.stroke.miter = miter;
            }
            ("pgfsetstrokecolor", [R(v)]) => {
                self.state.stroke.color = resolve_color(Some(&self.state.colors), v)?;
            }
            ("pgfsetfillcolor", [R(v)]) => {
                self.state.fill = resolve_color(Some(&self.state.colors), v)?;
            }
            ("pgfsetcolor", [R(v)]) => {
                self.state.fill = resolve_color(Some(&self.state.colors), v)?;
                self.state.stroke.color = self.state.fill;
            }
            ("pgfsys@transformshift", [R(x), R(y)]) => {
                self.state.transform *= Affine::translate((dimension(x)?, dimension(y)?));
            }
            ("pgfsys@transformcm", [R(a), R(b), R(c), R(d), R(x), R(y)]) => {
                self.state.transform *= Affine::new([
                    scalar(a)?,
                    scalar(b)?,
                    scalar(c)?,
                    scalar(d)?,
                    dimension(x)?,
                    dimension(y)?,
                ]);
            }
            ("pgfsys@defobject", [R(key), R(min), R(max), R(body)]) => {
                point(min)?;
                point(max)?;
                if !key.chars().all(|c| c.is_alphanumeric() || "-_@".contains(c)) {
                    return Err("dynamic object name".into());
                }
                self.objects.insert(key.to_string(), object(body)?);
            }
            ("pgfsys@useobject", [R(key), R("")]) => {
                let paths =
                    self.objects.get(*key).cloned().ok_or("unknown static object")?;
                let depth = self.state.clips.len();
                for (path, actions) in paths.iter() {
                    self.use_path(path.clone(), *actions)?;
                }
                self.state.clips.truncate(depth);
            }
            ("pgfusepath", [R(actions)]) => {
                let path = std::mem::take(&mut self.path);
                self.use_path(Rc::new(path), Actions::parse(actions)?)?;
            }
            ("pgfcoordinate", [R(name), R(at)]) => {
                self.place(Item::Anchor { name: name.to_string(), at: point(at)? }, 0.)?;
            }
            ("pgftext", [options @ .., R(body)]) => {
                let options = match options {
                    [] => "",
                    [O(s)] => s,
                    _ => return Err("invalid PGF text options".into()),
                };
                let (item, angle) = labels::parse(
                    options,
                    body,
                    Some(&self.state.colors),
                    self.state.fill_opacity,
                )?;
                self.place(item, angle)?;
            }
            _ if path_command(&mut self.path, name, args)? => {}
            _ => return Err(format!("unsupported PGF command or arguments: \\{name}")),
        }
        Ok(())
    }
}

pub fn read_with(
    source: &str,
    resources: &mut super::resources::Inputs,
) -> Result<Figure> {
    let mut r = Reader {
        state: State {
            transform: Affine::IDENTITY,
            clips: Vec::new(),
            viewport_clip: false,
            stroke: Stroke::black(0.4 * TEX_PT),
            fill: BLACK,
            fill_opacity: 1.,
            fill_rule: FillRule::NonZero,
            colors: HashMap::new(),
        },
        scopes: vec![],
        path: BezPath::new(),
        objects: HashMap::new(),
        figure: Figure::default(),
    };
    fn parse(
        r: &mut Reader,
        source: &str,
        inputs: &mut super::resources::Inputs,
    ) -> Result<()> {
        let source = labels::uncomment(source);
        let mut parser = Parser::new(&source);
        while let Some(event) = parser.next_event()? {
            match event {
                Event::Command { name: "input", args } => {
                    let [Argument::Required(name)] = args.as_slice() else {
                        return Err("expected a braced input path".into());
                    };
                    let mut path = super::resources::resolve(name)?;
                    if std::path::Path::new(&path).extension().is_none() {
                        path.push_str(".tex");
                    }
                    inputs.include(&path, |data, inputs| parse(r, data, inputs))?;
                }
                Event::Command { name, args } => {
                    r.command(name, &args).map_err(|e| format!("PGF \\{name}: {e}"))?
                }
                Event::GroupStart => r.open("{}")?,
                Event::GroupEnd => r.close("{}")?,
                Event::Text { value } => {
                    return Err(format!("unexpected PGF text {value:?}"))
                }
            }
        }
        Ok(())
    }
    parse(&mut r, source, resources)?;
    if !r.scopes.is_empty() || !r.path.elements().is_empty() {
        return Err("unfinished PGF scope or path".into());
    }
    Ok(r.figure)
}
