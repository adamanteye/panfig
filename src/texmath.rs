//! convert inline label math with MiTeX, leaving dynamic TeX to the PGF engine.

use mitex::ContextFeature;
use mitex_lexer::{BraceKind, CommandName, Lexer, Token};
use mitex_spec_gen::DEFAULT_SPEC;

// both wrappers preserve their argument inside a Matplotlib math label.
const PREAMBLE: &str = r"\newcommand{\mathdefault}[1]{#1}\newcommand{\ensuremath}[1]{#1}";

// MiTeX copies the optional color model verbatim into Typst markup.
fn check_color_model(lexer: &mut Lexer<'_>) -> Result<(), String> {
    while lexer.peek().is_some_and(|token| token.is_trivia()) {
        lexer.eat();
    }
    if lexer.peek() == Some(Token::Left(BraceKind::Bracket)) {
        lexer.eat();
        let model = lexer.eat().is_some_and(|(token, name)| {
            token == Token::Word && name.bytes().all(|b| b.is_ascii_alphabetic())
        });
        if !model
            || lexer.eat().map(|(token, _)| token)
                != Some(Token::Right(BraceKind::Bracket))
        {
            return Err("unsupported inline math color model".into());
        }
    }
    Ok(())
}

pub(super) fn to_typst(source: &str) -> Result<String, String> {
    let spec = DEFAULT_SPEC.clone();
    let mut lexer = Lexer::<()>::new(source, spec.clone());
    let mut depth = 0usize;
    while let Some((token, text)) = lexer.eat() {
        match token {
            Token::CommandName(CommandName::Generic) => {
                let name = &text[1..];
                // leave document commands, macro definitions and handlers that evaluate
                // their TeX arguments as Typst code to the TeX engine.
                if spec.get_cmd(name).and_then(|cmd| cmd.alias.as_deref()) == Some("")
                    || ["includegraphics", "label", "raisebox", "vspace", "hspace"]
                        .contains(&name)
                {
                    return Err(format!("unsupported inline math command {text}"));
                }
                if matches!(name, "color" | "textcolor" | "colorbox") {
                    check_color_model(&mut lexer)?;
                }
            }
            Token::CommandName(CommandName::BeginEnvironment) => {
                if !spec.get_env(text).is_some_and(|env| {
                    matches!(
                        env.ctx_feature,
                        ContextFeature::IsMath
                            | ContextFeature::IsMatrix
                            | ContextFeature::IsCases
                    )
                }) {
                    return Err(format!("unsupported inline math environment {text}"));
                }
                depth += 1;
            }
            Token::Left(BraceKind::Curly) | Token::CommandName(CommandName::Left) => {
                depth += 1
            }
            Token::Right(BraceKind::Curly)
            | Token::CommandName(CommandName::EndEnvironment | CommandName::Right) => {
                depth = depth.checked_sub(1).ok_or("unmatched inline math delimiter")?;
            }
            // includes MiTeX's raw Typst conditional.
            Token::CommandName(_) => {
                return Err(format!("unsupported inline math command {text}"))
            }
            _ => {}
        }
        if depth > 64 {
            return Err("inline math nesting exceeds 64 groups".into());
        }
    }
    if depth != 0 {
        return Err("unclosed inline math group".into());
    }
    let converted = mitex::convert_math(&format!("{PREAMBLE}{source}"), Some(spec))?;
    let math = normalize_bars(converted.trim());
    if math.is_empty() {
        return Err("empty inline math".into());
    }
    Ok(math)
}

// TeX treats bare | and \| as ordinary symbols, while Typst pads whitespace-delimited
// fence bars. keep bars raw at stretchy delimiter positions (lr/big/middle).
fn normalize_bars(input: &str) -> String {
    if !input.contains('|') {
        return input.to_owned();
    }
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len() + 32);
    // each open '(' stores (is_delimiter_call, only_ws_since_open).
    let mut stack: Vec<(bool, bool)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let mut saw_token = true;
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
                out.push_str(&input[start..i]);
            }
            b'\\' => {
                let n = 1 + input[i + 1..].chars().next().map_or(0, char::len_utf8);
                out.push_str(&input[i..i + n]);
                i += n;
            }
            b'(' => {
                let is_delim = matches!(
                    ident_before(input, i),
                    Some("lr" | "mid" | "middle" | "big" | "Big" | "bigg" | "Bigg")
                );
                if let Some(top) = stack.last_mut() {
                    top.1 = false;
                }
                stack.push((is_delim, true));
                out.push('(');
                i += 1;
                saw_token = false;
            }
            b')' => {
                stack.pop();
                out.push(')');
                i += 1;
            }
            b'|' => {
                let double = bytes.get(i + 1) == Some(&b'|');
                let end = i + if double { 2 } else { 1 };
                let (in_delim, at_open) = stack.last().copied().unwrap_or((false, false));
                let at_close = in_delim && input[end..].trim_start().starts_with(')');
                if in_delim && (at_open || at_close) {
                    out.push_str(&input[i..end]);
                } else {
                    if !out.is_empty()
                        && !out.ends_with(|c: char| {
                            c.is_ascii_whitespace() || matches!(c, '(' | ',' | ';')
                        })
                    {
                        out.push(' ');
                    }
                    out.push_str(if double {
                        r#"class("normal", bar.v.double)"#
                    } else {
                        r#"class("normal", bar.v)"#
                    });
                }
                i = end;
            }
            b if b.is_ascii_whitespace() => {
                out.push(b as char);
                i += 1;
                saw_token = false;
            }
            _ => {
                let n = input[i..].chars().next().unwrap().len_utf8();
                out.push_str(&input[i..i + n]);
                i += n;
            }
        }
        if saw_token {
            if let Some(top) = stack.last_mut() {
                top.1 = false;
            }
        }
    }
    out
}

fn ident_before(s: &str, pos: usize) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut start = pos;
    while start > 0 && bytes[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }
    if start < pos
        && (start == 0
            || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_'))
    {
        Some(&s[start..pos])
    } else {
        None
    }
}
