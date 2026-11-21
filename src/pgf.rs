use serde::Serialize;

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Event<'a> {
    Command { name: &'a str, args: Vec<Argument<'a>> },
    GroupStart,
    GroupEnd,
    Text { value: &'a str },
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Argument<'a> {
    Required(&'a str),
    Optional(&'a str),
}

// after the last argument, a brace opens a TeX group whose drawing commands
// belong in the outer stream. Macros with unknown signatures use all adjacent
// delimited arguments.
fn required_args(name: &str) -> usize {
    match name {
        "begingroup" | "endgroup" | "makeatletter" | "makeatother" | "pgfpathclose"
        | "pgfseteorule" | "pgfsetnonzerorule" | "pgfpointorigin" | "pgfpicture"
        | "endpgfpicture" | "pgfscope" | "endpgfscope" | "pgfsetbuttcap"
        | "pgfsetroundcap" | "pgfsetrectcap" | "pgfsetmiterjoin" | "pgfsetroundjoin"
        | "pgfsetbeveljoin" => 0,
        "begin"
        | "input"
        | "includegraphics"
        | "pgfimage"
        | "end"
        | "pgfpathmoveto"
        | "pgfpathlineto"
        | "pgfusepath"
        | "pgftext"
        | "pgfsetlinewidth"
        | "pgfsetcolor"
        | "pgfsetfillcolor"
        | "pgfsetstrokecolor"
        | "pgfsetfillopacity"
        | "pgfsetstrokeopacity"
        | "pgftransformshift" => 1,
        "pgfcoordinate"
        | "pgfpointanchor"
        | "pgfpointadd"
        | "pgfpoint"
        | "pgfqpoint"
        | "pgfpathquadraticcurveto"
        | "pgfpathrectangle"
        | "pgfpathrectanglecorners"
        | "pgfpathcircle"
        | "pgfsetdash" => 2,
        "definecolor" | "pgfpathcurveto" | "pgfpatharc" | "pgfpathellipse" => 3,
        _ => usize::MAX,
    }
}

#[derive(PartialEq)]
enum Scope<'a> {
    Group,
    Environment(&'a str),
}

impl Scope<'_> {
    fn closing(&self) -> String {
        match self {
            Self::Group => "}".into(),
            Self::Environment(name) => format!(r"\end{{{name}}}"),
        }
    }
}

pub struct Parser<'a> {
    source: &'a str,
    pos: usize,
    scopes: Vec<(Scope<'a>, usize)>,
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source, pos: 0, scopes: Vec::new() }
    }

    fn rest(&self) -> &'a str {
        &self.source[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn error(&self, at: usize, message: impl std::fmt::Display) -> String {
        let prefix = self.source[..at].replace("\r\n", "\n");
        let mut lines = prefix.split(['\n', '\r']);
        let column = lines.next_back().unwrap_or_default().chars().count() + 1;
        let line = lines.count() + 1;
        format!("line {line}, column {column}: {message}")
    }

    fn skip_comment(&mut self) {
        self.pos += self.rest().find(['\r', '\n']).unwrap_or(self.rest().len());
    }

    fn skip_trivia(&mut self) {
        loop {
            self.pos += self.rest().len() - self.rest().trim_start().len();
            if self.peek() != Some('%') {
                break;
            }
            self.skip_comment();
        }
    }

    // A backslash quotes one control symbol, or starts a control word.
    // treat @ as a letter for PGF backend commands such as \pgfsys@useobject.
    fn control_name(&mut self) -> Result<&'a str, String> {
        let start = self.pos;
        let first = self
            .bump()
            .ok_or_else(|| self.error(start - 1, "dangling backslash"))?;
        let letter = |c: char| c.is_ascii_alphabetic() || c == '@';
        if letter(first) {
            while self.peek().is_some_and(letter) {
                self.bump();
            }
        }
        Ok(&self.source[start..self.pos])
    }

    fn argument(&mut self, optional: bool) -> Result<Argument<'a>, String> {
        let open = self.pos;
        self.bump();
        let start = self.pos;
        let mut braces = 0;
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.control_name()?;
                }
                '%' => self.skip_comment(),
                '{' => braces += 1,
                '}' if braces > 0 => braces -= 1,
                '}' if optional => {
                    return Err(self.error(self.pos - 1, "unexpected } inside option"))
                }
                '}' => return Ok(Argument::Required(&self.source[start..self.pos - 1])),
                ']' if optional && braces == 0 => {
                    return Ok(Argument::Optional(&self.source[start..self.pos - 1]));
                }
                _ => {}
            }
        }
        Err(self.error(
            open,
            if optional { "unclosed [ argument" } else { "unclosed { argument" },
        ))
    }

    /// read one braced argument and return the unconsumed TeX input.
    pub fn group_argument(mut self) -> Option<(&'a str, &'a str)> {
        if self.peek() != Some('{') {
            return None;
        }
        match self.argument(false).ok()? {
            Argument::Required(body) => Some((body, self.rest())),
            _ => None,
        }
    }

    fn close_scope(&mut self, found: Scope<'a>, at: usize) -> Result<(), String> {
        match self.scopes.pop() {
            Some((open, _)) if open == found => Ok(()),
            Some((open, _)) => Err(self.error(
                at,
                format!("expected {}, found {}", open.closing(), found.closing()),
            )),
            None => Err(self.error(at, format!("unexpected {}", found.closing()))),
        }
    }

    fn command(&mut self, at: usize) -> Result<Event<'a>, String> {
        let name = self.control_name()?;
        let mut args = Vec::new();
        let mut remaining = required_args(name);
        loop {
            self.skip_trivia();
            match self.peek() {
                Some('{') if remaining > 0 => {
                    args.push(self.argument(false)?);
                    remaining -= 1;
                }
                Some('[') if remaining > 0 || name == "begin" => {
                    args.push(self.argument(true)?)
                }
                _ => break,
            }
        }
        if matches!(name, "begin" | "end") {
            let Some(Argument::Required(env)) = args.first() else {
                return Err(self
                    .error(at, format!(r"expected an environment name after \{name}")));
            };
            let scope = Scope::Environment(env);
            if name == "begin" {
                self.scopes.push((scope, at));
            } else {
                self.close_scope(scope, at)?;
            }
        }
        Ok(Event::Command { name, args })
    }

    pub fn next_event(&mut self) -> Result<Option<Event<'a>>, String> {
        self.skip_trivia();
        let start = self.pos;
        let event = match self.bump() {
            None => {
                return match self.scopes.last() {
                    Some((open, at)) => {
                        Err(self.error(*at, format!("missing {}", open.closing())))
                    }
                    None => Ok(None),
                }
            }
            Some('\\') => self.command(start)?,
            Some('{') => {
                self.scopes.push((Scope::Group, start));
                Event::GroupStart
            }
            Some('}') => {
                self.close_scope(Scope::Group, start)?;
                Event::GroupEnd
            }
            Some(_) => {
                let len =
                    self.rest().find(['\\', '{', '}', '%']).unwrap_or(self.rest().len());
                self.pos += len;
                Event::Text { value: self.source[start..self.pos].trim_end() }
            }
        };
        Ok(Some(event))
    }
}
