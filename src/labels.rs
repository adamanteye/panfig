//! parse PGF text labels and rewrite literal labels for the TeX fallback bridge.
use super::drawing::*;
use super::pgf::{Argument, Event, Parser, MPL_CATCODES};
use super::pgfcodec::{dimension, point_command, position, resolve_color, scalar};
use kurbo::Affine;
use std::collections::HashMap;
use std::fmt::Write;

pub(super) fn font_size(s: &str) -> Result<f64> {
    dimension(s).or_else(|_| dimension(&format!("{s}pt")))
}

pub(super) fn uncomment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    let mut depth = 0usize;
    let mut active = None;
    while let Some(i) = rest.find(['%', '\\', '{', '}']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match rest.as_bytes()[0] {
            b'%' if active.is_none() => {
                let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
                rest = rest[end..].strip_prefix('\r').unwrap_or(&rest[end..]);
                rest = rest
                    .strip_prefix('\n')
                    .unwrap_or(rest)
                    .trim_start_matches([' ', '\t']);
            }
            b'%' => {
                out.push_str(r"\%");
                rest = &rest[1..];
            }
            b'\\' if rest.starts_with(MPL_CATCODES) => {
                active = Some(depth);
                out.push_str(MPL_CATCODES);
                rest = &rest[MPL_CATCODES.len()..];
            }
            b'\\' => {
                let n = 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
                out.push_str(&rest[..n]);
                rest = &rest[n..];
            }
            c => {
                if c == b'{' {
                    depth += 1;
                } else {
                    if active == Some(depth) {
                        active = None;
                    }
                    depth = depth.saturating_sub(1);
                }
                out.push(c as char);
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn label_runs(s: &str) -> Result<Vec<Run>> {
    fn group(
        s: &mut &str,
        runs: &mut Vec<Run>,
        bold: bool,
        italic: bool,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            return Err("PGF label nesting exceeds 64 groups".into());
        }
        let mut buf = [0u8; 4];
        while let Some(c) = s.chars().next() {
            *s = &s[c.len_utf8()..];
            match c {
                '}' if depth > 0 => return Ok(()),
                '{' => group(s, runs, bold, italic, depth + 1)?,
                '\\' => {
                    let first = s.chars().next().ok_or("dangling label escape")?;
                    if first.is_ascii_alphabetic() {
                        let len = s.bytes().take_while(u8::is_ascii_alphabetic).count();
                        let name = &s[..len];
                        *s = &s[len..];
                        if name == "textcolor" {
                            let rest = s
                                .strip_prefix("[HTML]{")
                                .ok_or("unsupported label macro \\textcolor")?;
                            let (hex, tail) = rest
                                .split_once("}{")
                                .ok_or("expected a text color group")?;
                            let fill = parse_hex(hex)?;
                            *s = tail;
                            let mut colored = Vec::new();
                            group(s, &mut colored, bold, italic, depth + 1)?;
                            for run in &mut colored {
                                run.fill.get_or_insert(fill);
                            }
                            runs.extend(colored);
                            continue;
                        }
                        let Some(rest) = s.strip_prefix('{') else {
                            return Err(format!("unsupported label macro \\{name}"));
                        };
                        *s = rest;
                        match name {
                            "textbf" => group(s, runs, true, italic, depth + 1)?,
                            "textit" | "emph" => group(s, runs, bold, true, depth + 1)?,
                            "textasciitilde" | "textasciicircum" | "textbackslash" => {
                                let Some(rest) = s.strip_prefix('}') else {
                                    return Err(format!(
                                        "expected empty group after \\{name}"
                                    ));
                                };
                                *s = rest;
                                let value = match name {
                                    "textasciitilde" => "~",
                                    "textasciicircum" => "^",
                                    _ => "\\",
                                };
                                push_run(runs, value, bold, italic);
                            }
                            _ => return Err(format!("unsupported label macro \\{name}")),
                        }
                    } else if "{}%&#_$ ".contains(first) {
                        *s = &s[first.len_utf8()..];
                        push_run(runs, first.encode_utf8(&mut buf), bold, italic);
                    } else {
                        return Err(format!("unsupported PGF label escape \\{first}"));
                    }
                }
                '$' => {
                    if s.starts_with('$') {
                        return Err("unsupported display math $$".into());
                    }
                    let mut math = String::new();
                    let mut closed = false;
                    while let Some(mc) = s.chars().next() {
                        *s = &s[mc.len_utf8()..];
                        if mc == '\\' {
                            math.push(mc);
                            if let Some(next) = s.chars().next() {
                                *s = &s[next.len_utf8()..];
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
                    let translated = super::texmath::to_typst(&math)?;
                    push_math(runs, translated);
                }
                '#' | '^' | '_' | '~' | '&' | '}' => {
                    return Err(format!("unsupported PGF label syntax {c}"))
                }
                c if c.is_whitespace() => {
                    *s = s.trim_start();
                    push_run(runs, " ", bold, italic);
                }
                _ => push_run(runs, c.encode_utf8(&mut buf), bold, italic),
            }
        }
        if depth == 0 {
            Ok(())
        } else {
            Err("unclosed label group".into())
        }
    }
    let mut runs = Vec::new();
    let mut cursor = s;
    group(&mut cursor, &mut runs, false, false, 0)?;
    Ok(runs)
}

fn leading_word(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix('\\')?;
    let len = rest
        .bytes()
        .take_while(|c| c.is_ascii_alphabetic() || *c == b'@')
        .count();
    (len > 0).then(|| (&rest[..len], rest[len..].trim_start()))
}

pub(super) fn parse(
    options: &str,
    text: &str,
    colors: Option<&HashMap<String, Color>>,
    opacity: f64,
) -> Result<(Item, f64)> {
    let mut angle: f64 = 0.;
    let (mut at, mut vertical, mut horizontal) = (Position::Absolute([0., 0.]), "", "");
    for opt in options.split(',').map(str::trim) {
        match opt {
            "" | "center" => {}
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
                    "rotate" => angle += scalar(value)?,
                    "x" | "y" | "at" => {
                        let delta = match key.trim() {
                            "x" => Position::Absolute([dimension(value)?, 0.]),
                            "y" => Position::Absolute([0., dimension(value)?]),
                            _ => position(value)?,
                        };
                        match delta {
                            Position::Absolute([x, y]) => {
                                let offset = Affine::rotate(angle.to_radians())
                                    * kurbo::Point::new(x, y);
                                let p = match &mut at {
                                    Position::Absolute(p)
                                    | Position::Relative { offset: p, .. } => p,
                                };
                                p[0] += offset.x;
                                p[1] += offset.y;
                            }
                            relative
                                if angle == 0. && at == Position::Absolute([0., 0.]) =>
                            {
                                at = relative
                            }
                            _ => {
                                return Err(
                                    "unsupported transformed named coordinate".into()
                                )
                            }
                        }
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
        return Ok((
            Item::Image {
                at,
                anchor,
                file: super::resources::resolve(name)?,
                width,
                height,
            },
            angle,
        ));
    }
    let mut body = text;
    let mut color = None;
    if let Some(rest) = body.strip_prefix(r"\color") {
        let (name, rest) =
            Parser::new(rest).group_argument().ok_or("expected label color")?;
        color = Some(resolve_color(colors, name)?);
        let rest = rest.trim_start();
        body = match Parser::new(rest).group_argument() {
            Some((inner, tail)) if tail.trim().is_empty() => inner,
            _ => rest,
        };
    }
    let mut font = None;
    let mut size = None;
    let mut italic = false;
    let mut bold = false;
    let mut pending_select = false;
    loop {
        let trimmed = body.trim_start();
        if let Some(rest) = trimmed.strip_prefix(r"\fontspec{") {
            let (family, rest) = rest.split_once('}').ok_or("unclosed font family")?;
            font = Some(family.to_owned());
            body = rest;
        } else if let Some(rest) = trimmed.strip_prefix(r"\fontsize{") {
            let (value, rest) =
                rest.split_once("}{").ok_or("expected fontsize and baseline skip")?;
            let (_, rest) = rest.split_once('}').ok_or("unclosed fontsize")?;
            size = Some(font_size(value)?);
            pending_select = true;
            body = rest;
        } else if let Some((cmd, rest)) = leading_word(trimmed) {
            match cmd {
                "sffamily" | "rmfamily" | "ttfamily" => {}
                "itshape" | "slshape" => italic = true,
                "upshape" => italic = false,
                "bfseries" => bold = true,
                "mdseries" => bold = false,
                "normalfont" => {
                    italic = false;
                    bold = false;
                }
                "selectfont" => pending_select = false,
                _ => break,
            }
            body = rest;
        } else {
            break;
        }
    }
    if pending_select {
        return Err("expected selectfont after fontsize".into());
    }
    let mut runs = label_runs(body.strip_prefix(MPL_CATCODES).unwrap_or(body))?;
    for run in &mut runs {
        run.fill = run.fill.or(color);
        run.italic |= italic;
        run.bold |= bold;
    }
    Ok((Item::Text { at, anchor, runs, font, size, opacity }, angle))
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

// preserve TeX math and its surrounding glue. only replace literal spans once
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
fn matplotlib_label(
    line: &str,
    labels: &mut Vec<(String, HostLabel)>,
    prefix: &str,
) -> Option<String> {
    let (setup, rest) = line.split_once(MPL_CATCODES)?;
    let body = rest.strip_suffix("}}%").or_else(|| rest.strip_suffix("}}"))?;
    let (face, dimensions) = setup.rsplit_once(r"\fontsize{")?;
    let outer = [r"{\sffamily", r"{\rmfamily", r"{\ttfamily"]
        .iter()
        .find_map(|f| face.strip_suffix(f))?;
    if !dimensions.ends_with(r"}\selectfont") || body.contains('%') {
        return None;
    }
    let (size, _) = dimensions.split_once("}{")?;
    let size = font_size(size).ok()?;
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
    Some(format!("{setup}{MPL_CATCODES}{replaced}{}", &rest[body.len()..]))
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
                let replaced = match parse("", &clean, None, 1.) {
                    Ok((Item::Text { runs, font, size, .. }, _)) => Some(placeholder(
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
                                        face.size = font_size(size).ok();
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
        &serde_json::json!({"source": out, "declarations": declarations, "labels": labels}),
    )
    .map_err(|e| e.to_string())
}
