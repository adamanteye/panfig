use super::drawing::*;
use super::pgf::{Argument, Event, Parser};
use std::collections::HashMap;
use std::fmt::Write;

fn opacity(s: &str) -> Result<f64> {
    s.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| (0.0..=1.).contains(v))
        .ok_or_else(|| format!("invalid opacity {s:?}"))
}

fn dimension(s: &str) -> Result<f64> {
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

fn point_command(s: &str) -> Result<(&str, Vec<Argument<'_>>)> {
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

fn point(s: &str) -> Result<Point> {
    let (name, args) = point_command(s)?;
    absolute(name, &args)
}

/// read a label position expressed in absolute or named coordinates.
fn position(s: &str) -> Result<Position> {
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

fn uncomment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '%' => {
                while chars.next_if(|c| !matches!(c, '\r' | '\n')).is_some() {}
                chars.next_if_eq(&'\r');
                chars.next_if_eq(&'\n');
                while chars.next_if(|c| matches!(c, ' ' | '\t')).is_some() {}
            }
            '\\' => {
                out.push(c);
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[derive(Clone)]
struct State {
    stroke: Stroke,
    fill: Color,
    fill_opacity: f64,
    fill_rule: FillRule,
    colors: HashMap<String, Color>,
}

impl State {
    fn color(&self, name: &str) -> Result<Color> {
        if let Some(c) = self.colors.get(name) {
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
}

fn define_color(model: &str, s: &str) -> Result<Color> {
    if model == "HTML" {
        return parse_hex(s);
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

fn label_runs(s: &str) -> Result<Vec<Run>> {
    fn group(
        chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
        runs: &mut Vec<Run>,
        bold: bool,
        italic: bool,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            return Err("PGF label nesting exceeds 64 groups".into());
        }
        while let Some(c) = chars.next() {
            match c {
                '}' if depth > 0 => return Ok(()),
                '{' => group(chars, runs, bold, italic, depth + 1)?,
                '\\' => {
                    let c = chars.next().ok_or("dangling label escape")?;
                    if c.is_ascii_alphabetic() {
                        let mut name = c.to_string();
                        while let Some(c) = chars.next_if(|c| c.is_ascii_alphabetic()) {
                            name.push(c);
                        }
                        if name == "textcolor"
                            && chars.by_ref().take(7).collect::<String>() == "[HTML]{"
                        {
                            let hex = chars
                                .by_ref()
                                .take_while(|c| *c != '}')
                                .collect::<String>();
                            let fill = parse_hex(&hex)?;
                            if chars.next() != Some('{') {
                                return Err("expected a text color group".into());
                            }
                            let mut colored = Vec::new();
                            group(chars, &mut colored, bold, italic, depth + 1)?;
                            for run in &mut colored {
                                run.fill.get_or_insert(fill);
                            }
                            runs.extend(colored);
                            continue;
                        }
                        if chars.next() != Some('{') {
                            return Err(format!("unsupported label macro \\{name}"));
                        }
                        match name.as_str() {
                            "textbf" => group(chars, runs, true, italic, depth + 1)?,
                            "textit" | "emph" => {
                                group(chars, runs, bold, true, depth + 1)?
                            }
                            "textasciitilde" | "textasciicircum" | "textbackslash" => {
                                if chars.next() != Some('}') {
                                    return Err(format!(
                                        "expected empty group after \\{name}"
                                    ));
                                }
                                let value = match name.as_str() {
                                    "textasciitilde" => "~",
                                    "textasciicircum" => "^",
                                    _ => "\\",
                                };
                                push_run(runs, value, bold, italic);
                            }
                            _ => return Err(format!("unsupported label macro \\{name}")),
                        }
                    } else if "{}%&#_$ ".contains(c) {
                        push_run(runs, &c.to_string(), bold, italic);
                    } else {
                        return Err(format!("unsupported PGF label escape \\{c}"));
                    }
                }
                '$' => {
                    if chars.peek() == Some(&'$') {
                        return Err("unsupported display math $$".into());
                    }
                    let mut math = String::new();
                    let mut closed = false;
                    while let Some(mc) = chars.next() {
                        if mc == '\\' {
                            math.push(mc);
                            if let Some(next) = chars.next() {
                                math.push(next);
                            }
                        } else if mc == '$' {
                            closed = true;
                            break;
                        } else {
                            math.push(mc);
                        }
                    }
                    if !closed {
                        return Err("unclosed inline math".into());
                    }
                    let translated = super::texmath::to_typst(&math)
                        .ok_or_else(|| format!("unsupported TeX math: {math}"))?;
                    push_math(runs, translated);
                }
                '#' | '^' | '_' | '~' | '&' | '}' => {
                    return Err(format!("unsupported PGF label syntax {c}"))
                }
                c if c.is_whitespace() => {
                    while chars.next_if(|c| c.is_whitespace()).is_some() {}
                    push_run(runs, " ", bold, italic);
                }
                _ => push_run(runs, &c.to_string(), bold, italic),
            }
        }
        if depth == 0 {
            Ok(())
        } else {
            Err("unclosed label group".into())
        }
    }
    let mut runs = Vec::new();
    group(&mut s.chars().peekable(), &mut runs, false, false, 0)?;
    Ok(runs)
}

fn label(options: &str, text: &str) -> Result<Item> {
    let (mut at, mut vertical, mut horizontal) = (Position::Absolute([0., 0.]), "", "");
    for opt in options.split(',').map(str::trim) {
        match opt {
            "" => {}
            "left" => horizontal = "west",
            "right" => horizontal = "east",
            "top" => vertical = "north",
            "bottom" => vertical = "south",
            "base" => vertical = "base",
            _ => {
                let (key, value) = opt
                    .split_once('=')
                    .ok_or_else(|| format!("unsupported PGF text option {opt}"))?;
                let value = value.trim();
                let value = value
                    .strip_prefix('{')
                    .and_then(|v| v.strip_suffix('}'))
                    .unwrap_or(value);
                match key.trim() {
                    "at" => at = position(value)?,
                    "x" | "y" => {
                        let Position::Absolute(p) = &mut at else {
                            return Err(
                                "use pgfpointadd for offsets from a named coordinate"
                                    .into(),
                            );
                        };
                        p[usize::from(key.trim() == "y")] = dimension(value)?;
                    }
                    _ => return Err(format!("unsupported PGF text option {opt}")),
                }
            }
        }
    }
    let mut anchor = [vertical, horizontal]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if anchor.is_empty() {
        anchor = "center".into();
    }
    let image_text = text.trim();
    if image_text.starts_with(r"\includegraphics") || image_text.starts_with(r"\pgfimage")
    {
        let (cmd, args) = point_command(image_text)?;
        use Argument::{Optional as O, Required as R};
        let (opts, name) = match args.as_slice() {
            [R(name)] => ("", *name),
            [O(opts), R(name)] => (*opts, *name),
            _ => return Err(format!("invalid arguments to {cmd}")),
        };
        let (mut width, mut height) = (None, None);
        for opt in opts.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match opt.split_once('=') {
                Some(("width", v)) => width = Some(dimension(v)?),
                Some(("height", v)) => height = Some(dimension(v)?),
                Some(("interpolate", "true" | "false")) => {}
                _ => return Err(format!("unsupported image option {opt}")),
            }
        }
        return Ok(Item::Image {
            at,
            anchor,
            file: super::resources::resolve(name)?,
            width,
            height,
        });
    }
    let mut body = text;
    let mut font = None;
    let mut size = None;
    if let Some(rest) = body.strip_prefix(r"\fontspec{") {
        let (family, rest) = rest.split_once('}').ok_or("unclosed font family")?;
        font = Some(family.to_owned());
        body = rest;
    }
    if let Some(rest) = body.strip_prefix(r"\fontsize{") {
        let (value, rest) =
            rest.split_once("}{").ok_or("expected fontsize and baseline skip")?;
        let (_, rest) = rest.split_once('}').ok_or("unclosed fontsize")?;
        size = Some(dimension(value).or_else(|_| dimension(&format!("{value}pt")))?);
        body = rest
            .strip_prefix(r"\selectfont")
            .ok_or("expected selectfont after fontsize")?
            .trim_start();
    }
    Ok(Item::Text { at, anchor, runs: label_runs(body)?, font, size })
}

#[derive(serde::Serialize)]
struct HostLabel {
    runs: Vec<Run>,
    font: Option<String>,
    size: Option<f64>,
}

fn placeholder(
    labels: &mut Vec<(String, HostLabel)>,
    prefix: &str,
    label: HostLabel,
) -> String {
    let key = format!("panfig-label-{prefix}-{}", labels.len());
    let command = format!(r"\csname {key}\endcsname{{}}");
    labels.push((key, label));
    command
}

// preserve TeX math and its surrounding glue. Only replace literal spans once
// the complete label has been recognized, since arbitrary macros can consume text.
fn mixed_label(
    body: &str,
    size: Option<f64>,
    labels: &mut Vec<(String, HostLabel)>,
    prefix: &str,
) -> Option<String> {
    if body.contains(r"\$") || body.contains("$$") {
        return None;
    }
    let pieces: Vec<_> = body.split('$').collect();
    if pieces.len() % 2 == 0 {
        return None;
    }
    let mut spans = Vec::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i % 2 == 0 && !piece.trim().is_empty() {
            spans.push((i, label_runs(piece.trim()).ok()?));
        }
    }
    if spans.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut spans = spans.into_iter().peekable();
    for (i, s) in pieces.into_iter().enumerate() {
        if i % 2 == 1 {
            out.push('$');
            out.push_str(s);
            out.push('$');
        } else if let Some((_, runs)) = spans.next_if(|(idx, _)| *idx == i) {
            out.push_str(&s[..s.len() - s.trim_start().len()]);
            out.push_str(&placeholder(
                labels,
                prefix,
                HostLabel { runs, font: None, size },
            ));
            out.push_str(&s[s.trim_end().len()..]);
        } else {
            out.push_str(s);
        }
    }
    Some(out)
}

// matplotlib emits these active-character definitions inside the argument.
// the percent after the definition command names the active character.
fn matplotlib_label(
    line: &str,
    labels: &mut Vec<(String, HostLabel)>,
    prefix: &str,
) -> Option<String> {
    const CATCODES: &str =
        r"\catcode`\^=\active\def^{\ifmmode\sp\else\^{}\fi}\catcode`\%=\active\def%{\%}";
    let (setup, rest) = line.split_once(CATCODES)?;
    let body = rest.strip_suffix("}}%").or_else(|| rest.strip_suffix("}}"))?;
    let (face, dimensions) = setup.rsplit_once(r"\fontsize{")?;
    let outer = [r"{\sffamily", r"{\rmfamily", r"{\ttfamily"]
        .iter()
        .find_map(|f| face.strip_suffix(f))?;
    if !dimensions.ends_with(r"}\selectfont") || body.contains('%') {
        return None;
    }
    let (size, _) = dimensions.split_once("}{")?;
    let size = dimension(size).or_else(|_| dimension(&format!("{size}pt"))).ok()?;
    // lift the full text+math label into Typst and drop the TeX font selection so
    // e-TeX does not need external .tfm metrics for host-rendered labels.
    if let Ok(runs) = label_runs(body) {
        let closing = rest[body.len()..].strip_prefix('}')?;
        let key =
            placeholder(labels, prefix, HostLabel { runs, font: None, size: Some(size) });
        return Some(format!("{outer}{key}{closing}"));
    }
    let text = body.replace(r"\ensuremath{-}", "$-$");
    let replaced = mixed_label(&text, Some(size), labels, prefix)?;
    Some(format!("{setup}{CATCODES}{replaced}{}", &rest[body.len()..]))
}

fn node_body(rest: &str) -> Option<&str> {
    // let TikZ execute coordinate and shape options before placing the text.
    let i = rest.find(['{', ';', '%', '\\'])?;
    let (body, tail) = Parser::new(&rest[i..]).group_argument()?;
    tail.trim_start().starts_with(';').then_some(body)
}

/// bridge literal text to the host while TeX keeps control of math, color and transforms.
pub fn named_labels(source: &str, prefix: &str) -> Result<Vec<u8>> {
    #[derive(Clone, Default)]
    struct Face {
        font: Option<String>,
        size: Option<f64>,
        bold: bool,
        italic: bool,
    }
    let mut labels = Vec::new();
    let emit = |labels: &mut Vec<(String, HostLabel)>, text: &str, face: &Face| {
        let mut runs = Vec::new();
        push_run(&mut runs, text, face.bold, face.italic);
        placeholder(
            labels,
            prefix,
            HostLabel { runs, font: face.font.clone(), size: face.size },
        )
    };
    fn control(s: &str) -> (&str, &str) {
        let s = &s[1..];
        let mut n = s
            .bytes()
            .take_while(|c| c.is_ascii_alphabetic() || *c == b'@')
            .count();
        if n == 0 {
            n = s.chars().next().map_or(0, char::len_utf8);
        }
        s.split_at(n)
    }
    fn arg(s: &str) -> Option<(&str, &str)> {
        let s = s.trim_start().strip_prefix('{')?;
        let end = s.find('}')?;
        Some((&s[..end], &s[end + 1..]))
    }
    let mut out = String::new();
    let tikz = source.contains("tikzpicture");
    let mut input = source;
    while let Some(i) = input.find(['%', '\\']) {
        out.push_str(&input[..i]);
        let tail = &input[i..];
        if tail.starts_with('%') {
            let n = tail.find(['\n', '\r']).unwrap_or(tail.len());
            out.push_str(&tail[..n]);
            input = &tail[n..];
            continue;
        }
        let (cmd, rest) = control(tail);
        if cmd == "pgftext" {
            let line = tail.lines().next().unwrap();
            if let Some(replaced) = matplotlib_label(line, &mut labels, prefix) {
                out.push_str(&replaced);
                input = &tail[line.len()..];
                continue;
            }
        }
        let body = match cmd {
            "pgftext" => match Parser::new(tail).next_event() {
                Ok(Some(Event::Command { args, .. })) => {
                    args.last().and_then(|arg| match arg {
                        Argument::Required(body) => Some(*body),
                        _ => None,
                    })
                }
                _ => None,
            },
            "node" if tikz => node_body(rest),
            _ => None,
        };
        if let Some(body) = body {
            let n = body.as_ptr() as usize - tail.as_ptr() as usize;
            out.push_str(&tail[..n]);
            // let Typst measure a complete literal label before TeX places the box.
            let clean = uncomment(body);
            if !clean.is_empty() {
                let replaced = match label("", &clean) {
                    Ok(Item::Text { runs, font, size, .. }) => Some(placeholder(
                        &mut labels,
                        prefix,
                        HostLabel { runs, font, size },
                    )),
                    _ if clean.contains('$') => {
                        mixed_label(&clean, None, &mut labels, prefix)
                    }
                    _ => None,
                };
                if let Some(replaced) = replaced {
                    out.push_str(&replaced);
                    input = &tail[n + body.len()..];
                    continue;
                }
            }
            if !body.contains(r"\fontspec") && body.chars().all(|ch| ch as u32 <= 255) {
                out.push_str(body);
                input = &tail[n + body.len()..];
                continue;
            }
            let mut s = clean.as_str();
            let mut face = Face::default();
            let mut stack = vec![];
            let mut math = false;
            while !s.is_empty() {
                let start = s;
                let ch = s.chars().next().unwrap();
                s = &s[ch.len_utf8()..];
                match ch {
                    '$' => math = !math,
                    '{' => stack.push(face.clone()),
                    '}' => face = stack.pop().unwrap_or_default(),
                    '\\' => {
                        let (name, after) = control(start);
                        s = after;
                        if !math {
                            match name {
                                "fontspec" => {
                                    if let Some((family, next)) = arg(s) {
                                        face.font = Some(family.to_owned());
                                        s = next;
                                        out.push_str(r"\relax{}");
                                        continue;
                                    }
                                }
                                "fontsize" => {
                                    if let Some((size, (_, next))) =
                                        arg(s).and_then(|(v, n)| Some((v, arg(n)?)))
                                    {
                                        face.size = dimension(size)
                                            .or_else(|_| dimension(&format!("{size}pt")))
                                            .ok();
                                        s = next;
                                    }
                                }
                                "bfseries" => face.bold = true,
                                "itshape" | "slshape" => face.italic = true,
                                "color" | "includegraphics" | "pgfimage" => {
                                    let opts = s.trim_start();
                                    let rest = if opts.starts_with('[') {
                                        opts.split_once(']')
                                            .map(|(_, r)| r)
                                            .unwrap_or(opts)
                                    } else {
                                        opts
                                    };
                                    if let Some((_, next)) = arg(rest) {
                                        s = next;
                                    }
                                }
                                _ => (),
                            }
                        }
                    }
                    _ if !math
                        && (ch as u32 > 255
                            || (face.font.is_some()
                                && !ch.is_whitespace()
                                && !"#_^~&".contains(ch))) =>
                    {
                        let end = s
                            .find(|c: char| {
                                if face.font.is_some() {
                                    "\\{}$%#_^~&".contains(c)
                                } else {
                                    c as u32 <= 255
                                }
                            })
                            .unwrap_or(s.len());
                        let count = ch.len_utf8() + end;
                        out.push_str(&emit(&mut labels, &start[..count], &face));
                        s = &start[count..];
                        continue;
                    }
                    _ => (),
                }
                out.push_str(&start[..start.len() - s.len()]);
            }
            input = &tail[n + body.len()..];
            continue;
        }
        out.push_str(&tail[..tail.len() - rest.len()]);
        input = rest;
    }
    out.push_str(input);
    // nodes and macro-expanded character arguments go through inputenc.
    let mut declarations = String::new();
    for ch in uncomment(&out)
        .chars()
        .filter(|c| *c as u32 > 255)
        .collect::<std::collections::BTreeSet<_>>()
    {
        writeln!(
            declarations,
            r"\DeclareUnicodeCharacter{{{:X}}}{{{}}}",
            ch as u32,
            emit(&mut labels, &ch.to_string(), &Face::default())
        )
        .unwrap();
    }
    serde_json::to_vec(
        &serde_json::json!({"source":out,"declarations":declarations,"labels":labels}),
    )
    .map_err(|e| e.to_string())
}

struct Reader {
    figure: Figure,
    contours: Vec<Contour>,
    state: State,
    scopes: Vec<(String, State)>,
}

impl Reader {
    fn open(&mut self, name: &str) -> Result<()> {
        if !self.contours.is_empty() {
            return Err("scope change inside an unfinished path".into());
        }
        self.scopes.push((name.into(), self.state.clone()));
        Ok(())
    }
    fn close(&mut self, name: &str) -> Result<()> {
        if !self.contours.is_empty() {
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
    fn curve(&mut self) -> Result<(&mut Vec<Segment>, &mut bool)> {
        match self.contours.last_mut() {
            Some(Contour::Curve { segments, closed, .. }) if !*closed => {
                Ok((segments, closed))
            }
            _ => Err("expected an open path after pgfpathmoveto".into()),
        }
    }
    fn use_path(&mut self, actions: &str) -> Result<()> {
        let mut style = Style {
            stroke: None,
            fill: None,
            fill_opacity: 1.,
            fill_rule: self.state.fill_rule,
        };
        for action in actions.split(',').map(str::trim) {
            match action {
                "stroke" | "draw" => style.stroke = Some(self.state.stroke.clone()),
                "fill" => {
                    style.fill = Some(self.state.fill);
                    style.fill_opacity = self.state.fill_opacity;
                }
                "discard" | "" => {}
                _ => return Err(format!("unsupported PGF path action {action:?}")),
            }
        }
        let contours = std::mem::take(&mut self.contours);
        if !contours.is_empty() {
            self.figure.push(Item::Path { contours, style });
        }
        Ok(())
    }
    fn command(&mut self, name: &str, args: &[Argument<'_>]) -> Result<()> {
        use Argument::{Optional as O, Required as R};
        match (name, args) {
            ("makeatletter" | "makeatother", []) => {}
            ("begin", [R(env @ ("pgfpicture" | "pgfscope"))]) => self.open(env)?,
            ("end", [R(env @ ("pgfpicture" | "pgfscope"))]) => self.close(env)?,
            ("begingroup" | "pgfpicture" | "pgfscope", []) => self.open(name)?,
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
            ("pgfsetstrokecolor", [R(v)]) => {
                self.state.stroke.color = self.state.color(v)?
            }
            ("pgfsetfillcolor", [R(v)]) => self.state.fill = self.state.color(v)?,
            ("pgfsetcolor", [R(v)]) => {
                self.state.fill = self.state.color(v)?;
                self.state.stroke.color = self.state.fill;
            }
            ("pgfpathmoveto", [R(p)]) => self.contours.push(Contour::Curve {
                start: point(p)?,
                segments: vec![],
                closed: false,
            }),
            ("pgfpathlineto", [R(p)]) => {
                self.curve()?.0.push(Segment::Line { to: point(p)? })
            }
            ("pgfpathquadraticcurveto", [R(a), R(b)]) => self
                .curve()?
                .0
                .push(Segment::Quadratic { control: point(a)?, to: point(b)? }),
            ("pgfpathcurveto", [R(a), R(b), R(c)]) => {
                self.curve()?.0.push(Segment::Cubic {
                    first: point(a)?,
                    second: point(b)?,
                    to: point(c)?,
                })
            }
            ("pgfpathclose", []) => *self.curve()?.1 = true,
            ("pgfpathrectangle" | "pgfpathrectanglecorners", [R(a), R(b)]) => {
                let a = point(a)?;
                let mut b = point(b)?;
                if name == "pgfpathrectangle" {
                    b = [a[0] + b[0], a[1] + b[1]];
                }
                let contour = if (a[0] > b[0]) != (a[1] > b[1]) {
                    Contour::Curve {
                        start: a,
                        segments: vec![
                            Segment::Line { to: [b[0], a[1]] },
                            Segment::Line { to: b },
                            Segment::Line { to: [a[0], b[1]] },
                        ],
                        closed: true,
                    }
                } else {
                    Contour::rectangle(a, b)
                };
                self.contours.push(contour);
            }
            ("pgfpathcircle", [R(p), R(r)]) => self
                .contours
                .push(Contour::Circle { center: point(p)?, radius: dimension(r)? }),
            ("pgfusepath", [R(actions)]) => self.use_path(actions)?,
            ("pgfcoordinate", [R(name), R(at)]) => self
                .figure
                .push(Item::Anchor { name: name.to_string(), at: point(at)? }),
            ("pgftext", [R(text)]) => self.figure.push(label("", text)?),
            ("pgftext", [O(opts), R(text)]) => self.figure.push(label(opts, text)?),
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
            stroke: Stroke::black(0.4 * TEX_PT),
            fill: BLACK,
            fill_opacity: 1.,
            fill_rule: FillRule::NonZero,
            colors: HashMap::new(),
        },
        scopes: vec![],
        contours: vec![],
        figure: vec![],
    };
    fn parse(
        r: &mut Reader,
        source: &str,
        inputs: &mut super::resources::Inputs,
    ) -> Result<()> {
        let source = uncomment(source);
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
    if !r.scopes.is_empty() || !r.contours.is_empty() {
        return Err("unfinished PGF scope or path".into());
    }
    Ok(r.figure)
}
