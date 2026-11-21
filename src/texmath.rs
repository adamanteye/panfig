//! recursively translate common LaTeX math into Typst math syntax.

const MAX_DEPTH: usize = 64;

/// translate a LaTeX math expression (without surrounding `$`) into Typst math.
/// returns `None` if the expression uses syntax that should stay in TeX.
pub fn to_typst(source: &str) -> Option<String> {
    let mut p = Parser { src: source, pos: 0 };
    let out = p.seq(false, false, 0)?;
    if p.pos != p.src.len() {
        return None;
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_owned())
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn control(&mut self) -> Option<&'a str> {
        let start = self.pos;
        let first = self.bump()?;
        if first.is_ascii_alphabetic() {
            while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                self.bump();
            }
        }
        Some(&self.src[start..self.pos])
    }

    /// read a braced `{...}` group verbatim (raw inner slice, balanced braces).
    fn raw_braced(&mut self) -> Option<&'a str> {
        self.skip_ws();
        if self.peek() != Some('{') {
            return None;
        }
        self.bump();
        let start = self.pos;
        let mut depth = 0usize;
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.bump()?;
                }
                '{' => depth += 1,
                '}' if depth > 0 => depth -= 1,
                '}' => return Some(&self.src[start..self.pos - 1]),
                _ => {}
            }
        }
        None
    }

    fn raw_bracketed(&mut self) -> Option<&'a str> {
        self.skip_ws();
        if self.peek() != Some('[') {
            return None;
        }
        self.bump();
        let start = self.pos;
        let mut depth = 0usize;
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.bump()?;
                }
                '{' => depth += 1,
                '}' if depth > 0 => depth -= 1,
                ']' if depth == 0 => return Some(&self.src[start..self.pos - 1]),
                _ => {}
            }
        }
        None
    }

    /// parse a single required math argument (either `{...}` or a single atom).
    fn arg(&mut self, depth: usize) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.skip_ws();
        if self.peek() == Some('{') {
            self.bump();
            return self.seq(true, false, depth + 1);
        }
        self.atom(depth + 1, false)
    }

    fn delim(&mut self) -> Option<&'static str> {
        self.skip_ws();
        match self.bump()? {
            '.' => Some(""),
            '(' => Some("("),
            ')' => Some(")"),
            '[' => Some("["),
            ']' => Some("]"),
            '|' => Some("|"),
            '/' => Some(r"\/"),
            '\\' => match self.control()? {
                "{" => Some(r"\{"),
                "}" => Some(r"\}"),
                "|" => Some("bar.v.double"),
                "langle" => Some("chevron.l"),
                "rangle" => Some("chevron.r"),
                "lfloor" => Some("floor.l"),
                "rfloor" => Some("floor.r"),
                "lceil" => Some("ceil.l"),
                "rceil" => Some("ceil.r"),
                "vert" => Some("|"),
                "Vert" => Some("bar.v.double"),
                "backslash" => Some(r"\\"),
                _ => None,
            },
            _ => None,
        }
    }

    fn text_arg(&mut self, depth: usize) -> Option<String> {
        let raw = self.raw_braced()?;
        if raw.contains(['\\', '$', '{', '}', '^', '_']) {
            let inner = Parser { src: raw, pos: 0 }.seq(false, false, depth + 1)?;
            return Some(format!("upright({inner})"));
        }
        Some(quote(raw))
    }

    fn command(&mut self, name: &str, depth: usize) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        match name {
            // layout/sizing wrappers with no semantic change
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "!"
            | "limits" | "nolimits" => Some(String::new()),
            "mathdefault" | "ensuremath" | "mathord" | "mathbin" | "mathrel"
            | "mathop" | "mathopen" | "mathclose" | "mathpunct" | "mathinner" => {
                self.arg(depth + 1)
            }
            // delimiter sizing
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr"
            | "Bigl" | "Bigr" | "biggl" | "biggr" | "Biggl" | "Biggr" => {
                self.delim().map(str::to_owned)
            }
            // fractions and roots
            "frac" | "dfrac" | "tfrac" => {
                let num = self.arg(depth + 1)?;
                let den = self.arg(depth + 1)?;
                Some(format!("frac({num}, {den})"))
            }
            "sqrt" => {
                let idx = self.raw_bracketed();
                let rad = self.arg(depth + 1)?;
                if let Some(n) = idx {
                    let n = Parser { src: n, pos: 0 }.seq(false, false, depth + 1)?;
                    Some(format!("root({n}, {rad})"))
                } else {
                    Some(format!("sqrt({rad})"))
                }
            }
            // text and font styles inside math
            "text" | "textrm" | "mathrm" | "operatorname" => self.text_arg(depth),
            "textbf" | "mathbf" | "boldsymbol" | "bm" => {
                Some(format!("bold({})", self.arg(depth + 1)?))
            }
            "textit" | "mathit" => Some(format!("italic({})", self.arg(depth + 1)?)),
            "mathcal" => Some(format!("cal({})", self.arg(depth + 1)?)),
            "mathbb" => Some(format!("bb({})", self.arg(depth + 1)?)),
            "mathfrak" => Some(format!("frak({})", self.arg(depth + 1)?)),
            "mathsf" => Some(format!("sans({})", self.arg(depth + 1)?)),
            "mathtt" => Some(format!("mono({})", self.arg(depth + 1)?)),
            // accents and decorations
            "hat" | "widehat" => Some(format!("hat({})", self.arg(depth + 1)?)),
            "tilde" | "widetilde" => Some(format!("tilde({})", self.arg(depth + 1)?)),
            "bar" => Some(format!("macron({})", self.arg(depth + 1)?)),
            "overline" => Some(format!("overline({})", self.arg(depth + 1)?)),
            "underline" => Some(format!("underline({})", self.arg(depth + 1)?)),
            "vec" => Some(format!("arrow({})", self.arg(depth + 1)?)),
            "dot" => Some(format!("dot({})", self.arg(depth + 1)?)),
            "ddot" => Some(format!("dot.double({})", self.arg(depth + 1)?)),
            // spacing
            "," => Some("thin".into()),
            ":" | ">" => Some("med".into()),
            ";" => Some("thick".into()),
            "quad" => Some("quad".into()),
            "qquad" => Some("wide".into()),
            " " => Some("\" \"".into()),
            // escaped punctuation
            "{" => Some(r"\{".into()),
            "}" => Some(r"\}".into()),
            "|" => Some("bar.v.double".into()),
            "%" => Some(r"\%".into()),
            "&" => Some(r"\&".into()),
            "#" => Some(r"\#".into()),
            "_" => Some(r"\_".into()),
            "$" => Some(r"\$".into()),
            _ => symbol(name).map(str::to_owned),
        }
    }

    /// check if an odd number of unnested `|` remain before a closing delimiter,
    /// meaning the just-consumed `|` opens a paired `|...|` absolute value.
    fn has_matching_bar(&self) -> bool {
        let mut chars = self.rest().chars();
        let (mut braces, mut parens, mut bars) = (0usize, 0usize, 0usize);
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '{' => braces += 1,
                '}' => {
                    if braces == 0 {
                        break;
                    }
                    braces -= 1;
                }
                '(' => parens += 1,
                ')' => {
                    if braces == 0 && parens == 0 {
                        break;
                    }
                    parens = parens.saturating_sub(1);
                }
                '|' if braces == 0 && parens == 0 => bars += 1,
                _ => {}
            }
        }
        bars % 2 == 1
    }

    fn atom(&mut self, depth: usize, stop_bar: bool) -> Option<String> {
        let c = self.bump()?;
        match c {
            '\\' => {
                let name = self.control()?;
                self.command(name, depth)
            }
            '{' => self.seq(true, false, depth + 1),
            '|' if !stop_bar && self.has_matching_bar() => {
                let inner = self.seq(false, true, depth + 1)?;
                Some(format!("abs({inner})"))
            }
            '}' | '^' | '_' | '$' | '%' | '&' | '#' => None,
            '~' => Some("\" \"".into()),
            '/' => Some(r"\/".into()),
            '"' => Some(r#"\""#.into()),
            '\'' => Some("'".into()),
            c if c.is_ascii_digit() => {
                let mut s = String::from(c);
                while let Some(next) = self.peek() {
                    let decimal = next == '.'
                        && self.rest()[1..]
                            .chars()
                            .next()
                            .is_some_and(|d| d.is_ascii_digit());
                    if next.is_ascii_digit() || decimal {
                        s.push(self.bump().unwrap());
                    } else {
                        break;
                    }
                }
                Some(s)
            }
            c => Some(c.to_string()),
        }
    }

    fn seq(&mut self, stop_brace: bool, stop_bar: bool, depth: usize) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let mut out = String::new();
        let mut has_atom = false;
        let mut parens = 0usize;
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else {
                return (!stop_brace && !stop_bar).then_some(out);
            };
            if c == '}' {
                if stop_brace {
                    self.bump();
                    return Some(out);
                }
                return None;
            }
            if c == '|' && stop_bar && parens == 0 {
                self.bump();
                return Some(out);
            }
            if c == '^' || c == '_' {
                self.bump();
                let script = self.arg(depth + 1)?;
                if !has_atom {
                    push_token(&mut out, "\"\"");
                    has_atom = true;
                }
                out.push(c);
                if is_simple_script(&script) {
                    out.push_str(&script);
                } else {
                    out.push('(');
                    out.push_str(&script);
                    out.push(')');
                }
                continue;
            }
            if c == '\'' {
                self.bump();
                if !has_atom {
                    push_token(&mut out, "\"\"");
                    has_atom = true;
                }
                out.push('\'');
                continue;
            }
            if c == '(' {
                parens += 1;
            } else if c == ')' {
                parens = parens.saturating_sub(1);
            }
            let part = self.atom(depth, stop_bar && parens == 0)?;
            if !part.is_empty() {
                push_token(&mut out, &part);
                has_atom = true;
            }
        }
    }
}

fn push_token(out: &mut String, token: &str) {
    if !out.is_empty() {
        out.push(' ');
    }
    out.push_str(token);
}

fn is_simple_script(s: &str) -> bool {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => c.is_ascii_alphanumeric(),
        _ => false,
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' | '"' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        // greek letters
        "alpha" => "alpha",
        "beta" => "beta",
        "gamma" => "gamma",
        "delta" => "delta",
        "epsilon" => "epsilon.alt",
        "varepsilon" => "epsilon",
        "zeta" => "zeta",
        "eta" => "eta",
        "theta" => "theta",
        "vartheta" => "theta.alt",
        "iota" => "iota",
        "kappa" => "kappa",
        "lambda" => "lambda",
        "mu" => "mu",
        "nu" => "nu",
        "xi" => "xi",
        "pi" => "pi",
        "varpi" => "pi.alt",
        "rho" => "rho",
        "varrho" => "rho.alt",
        "sigma" => "sigma",
        "varsigma" => "sigma.alt",
        "tau" => "tau",
        "upsilon" => "upsilon",
        "phi" => "phi.alt",
        "varphi" => "phi",
        "chi" => "chi",
        "psi" => "psi",
        "omega" => "omega",
        "Gamma" => "Gamma",
        "Delta" => "Delta",
        "Theta" => "Theta",
        "Lambda" => "Lambda",
        "Xi" => "Xi",
        "Pi" => "Pi",
        "Sigma" => "Sigma",
        "Upsilon" => "Upsilon",
        "Phi" => "Phi",
        "Psi" => "Psi",
        "Omega" => "Omega",
        // letter-like symbols
        "ell" => "ell",
        "hbar" => "\u{210f}",
        "infty" => "infinity",
        "partial" => "diff",
        "nabla" => "nabla",
        "Re" => "Re",
        "Im" => "Im",
        "aleph" => "aleph",
        "wp" => "\u{2118}",
        "emptyset" | "varnothing" => "emptyset",
        "degree" => "degree",
        "circ" => "circle.stroked.tiny",
        "bullet" => "bullet",
        "star" => "star",
        // operators and relations
        "pm" => "plus.minus",
        "mp" => "minus.plus",
        "times" => "times",
        "div" => "div",
        "cdot" => "dot.op",
        "cdots" => "dots.c",
        "ldots" | "dots" => "dots.h",
        "vdots" => "dots.v",
        "ddots" => "dots.down",
        "leq" | "le" => "<=",
        "geq" | "ge" => ">=",
        "neq" | "ne" => "!=",
        "approx" => "approx",
        "equiv" => "equiv",
        "sim" => "tilde.op",
        "simeq" => "tilde.eq",
        "cong" => "tilde.equiv",
        "propto" => "prop",
        "ll" => "lt.double",
        "gg" => "gt.double",
        "in" => "in",
        "notin" => "in.not",
        "ni" => "in.rev",
        "subset" => "subset",
        "subseteq" => "subset.eq",
        "supset" => "supset",
        "supseteq" => "supset.eq",
        "cup" => "union",
        "cap" => "inter",
        "setminus" => "without",
        "to" | "rightarrow" => "->",
        "leftarrow" => "<-",
        "leftrightarrow" => "<->",
        "Rightarrow" => "arrow.r.double",
        "Leftarrow" => "arrow.l.double",
        "Leftrightarrow" => "arrow.l.r.double",
        "mapsto" => "|->",
        "forall" => "forall",
        "exists" => "exists",
        "neg" | "lnot" => "not",
        "land" | "wedge" => "and",
        "lor" | "vee" => "or",
        "perp" => "perp",
        "parallel" => "parallel",
        "angle" => "angle",
        "langle" => "chevron.l",
        "rangle" => "chevron.r",
        "lfloor" => "floor.l",
        "rfloor" => "floor.r",
        "lceil" => "ceil.l",
        "rceil" => "ceil.r",
        "vert" => "|",
        "Vert" => "bar.v.double",
        "dagger" => "dagger",
        "ddagger" => "dagger.double",
        "prime" => "prime",
        // big operators and functions
        "sum" => "sum",
        "prod" => "product",
        "int" => "integral",
        "iint" => "integral.double",
        "iiint" => "integral.triple",
        "oint" => "integral.cont",
        "bigcup" => "union.big",
        "bigcap" => "inter.big",
        "lim" => "lim",
        "limsup" => "limsup",
        "liminf" => "liminf",
        "sup" => "sup",
        "inf" => "inf",
        "max" => "max",
        "min" => "min",
        "arg" => "arg",
        "det" => "det",
        "dim" => "dim",
        "exp" => "exp",
        "gcd" => "gcd",
        "hom" => "hom",
        "ker" => "ker",
        "lg" => "lg",
        "ln" => "ln",
        "log" => "log",
        "Pr" => "Pr",
        "sin" => "sin",
        "cos" => "cos",
        "tan" => "tan",
        "cot" => "cot",
        "sec" => "sec",
        "csc" => "csc",
        "arcsin" => "arcsin",
        "arccos" => "arccos",
        "arctan" => "arctan",
        "sinh" => "sinh",
        "cosh" => "cosh",
        "tanh" => "tanh",
        "coth" => "coth",
        "deg" => "deg",
        "bmod" => "mod",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::to_typst;

    #[test]
    fn translates_hydrogen_and_common_math() {
        assert_eq!(to_typst(r"\mathdefault{\ensuremath{-}20}").as_deref(), Some("- 20"));
        assert_eq!(to_typst(r"x / a_0").as_deref(), Some(r"x \/ a_0"));
        assert_eq!(
            to_typst(r"3p_0\; (3,1,0)").as_deref(),
            Some("3 p_0 thick ( 3 , 1 , 0 )")
        );
        assert_eq!(
            to_typst(r"|\psi|^2 / |\psi|_{\max}^2").as_deref(),
            Some(r"abs(psi)^2 \/ abs(psi)_(max)^2")
        );
        assert_eq!(
            to_typst(r"P_{4\ell}(r) = r^2 |R_{4\ell}(r)|^2").as_deref(),
            Some("P_(4 ell) ( r ) = r^2 abs(R_(4 ell) ( r ))^2")
        );
        assert_eq!(to_typst(r"\frac{3\pi}{2}").as_deref(), Some("frac(3 pi, 2)"));
        assert_eq!(
            to_typst(r"\langle r \rangle").as_deref(),
            Some("chevron.l r chevron.r")
        );
        assert_eq!(
            to_typst(r"|Y_\ell^m(\theta,0)|^2").as_deref(),
            Some("abs(Y_(ell)^m ( theta , 0 ))^2")
        );
        assert_eq!(
            to_typst(r"|\psi_{n\ell m}(r,\theta,\phi)|^2").as_deref(),
            Some("abs(psi_(n ell m) ( r , theta , phi.alt ))^2")
        );
        assert_eq!(to_typst(r"P(A | B)").as_deref(), Some("P ( A | B )"));
        assert_eq!(to_typst(r"\unknowncmd{x}"), None);
    }
}
