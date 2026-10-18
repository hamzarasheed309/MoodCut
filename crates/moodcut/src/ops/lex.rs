//! Lexer for MoodCut Script (MCS): line-based commands of the form
//! `verb arg arg key=value key='quoted value'`, separated by newlines or `;`.
//! Comments: lines starting with `#` + space, or `//` anywhere outside quotes.

use anyhow::{bail, Result};

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    /// Bare word.
    W(String),
    /// Quoted string.
    Q(String),
    /// key=value (value unquoted or quoted).
    KV(String, String),
}

#[derive(Clone, Debug)]
pub struct Cmd {
    pub line: usize,
    pub text: String,
    pub verb: String,
    pub args: Vec<Tok>,
}

impl Cmd {
    /// Positional (non key=value) arguments as strings.
    pub fn pos(&self) -> Vec<String> {
        self.args
            .iter()
            .filter_map(|t| match t {
                Tok::W(s) | Tok::Q(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn kv(&self) -> Vec<(String, String)> {
        self.args
            .iter()
            .filter_map(|t| match t {
                Tok::KV(k, v) => Some((k.clone(), v.clone())),
                _ => None,
            })
            .collect()
    }
}

pub fn parse_script(src: &str) -> Result<Vec<Cmd>> {
    let mut cmds = vec![];
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1;
    let mut toks: Vec<Tok> = vec![];
    let mut text = String::new();
    let mut cmd_line = 1;

    let flush = |toks: &mut Vec<Tok>, text: &mut String, cmds: &mut Vec<Cmd>, ln: usize| {
        if !toks.is_empty() {
            let mut args = std::mem::take(toks);
            let verb = match args.remove(0) {
                Tok::W(w) => w.to_ascii_lowercase(),
                Tok::Q(q) => q,
                Tok::KV(k, v) => format!("{k}={v}"),
            };
            cmds.push(Cmd { line: ln, text: text.trim().to_string(), verb, args });
        }
        text.clear();
    };

    while i < chars.len() {
        let c = chars[i];
        // comments
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '#' && toks.is_empty() && (i + 1 >= chars.len() || chars[i + 1] == ' ' || chars[i + 1] == '\n' || chars[i + 1] == '#') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '\n' || c == ';' {
            flush(&mut toks, &mut text, &mut cmds, cmd_line);
            if c == '\n' {
                line += 1;
            }
            i += 1;
            cmd_line = line;
            continue;
        }
        if c.is_whitespace() {
            if !toks.is_empty() {
                text.push(' ');
            }
            i += 1;
            continue;
        }
        if toks.is_empty() {
            cmd_line = line;
        }
        // token
        if c == '\'' || c == '"' {
            let (s, ni) = read_quoted(&chars, i, line)?;
            text.extend(&chars[i..ni]);
            toks.push(Tok::Q(s));
            i = ni;
            continue;
        }
        // bare word, possibly key=value
        let st = i;
        let mut word = String::new();
        while i < chars.len() {
            let ch = chars[i];
            if ch.is_whitespace() || ch == ';' {
                break;
            }
            if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
                break;
            }
            if ch == '=' && !word.is_empty() && !word.contains('=') {
                // key=value
                i += 1;
                let key = std::mem::take(&mut word);
                if i < chars.len() && (chars[i] == '\'' || chars[i] == '"') {
                    let (s, ni) = read_quoted(&chars, i, line)?;
                    i = ni;
                    toks.push(Tok::KV(key, s));
                } else {
                    let mut val = String::new();
                    while i < chars.len() && !chars[i].is_whitespace() && chars[i] != ';' {
                        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
                            break;
                        }
                        val.push(chars[i]);
                        i += 1;
                    }
                    toks.push(Tok::KV(key, val));
                }
                word.clear();
                text.extend(&chars[st..i]);
                break;
            }
            word.push(ch);
            i += 1;
        }
        if !word.is_empty() {
            text.extend(&chars[st..i]);
            toks.push(Tok::W(word));
        }
    }
    flush(&mut toks, &mut text, &mut cmds, cmd_line);
    Ok(cmds)
}

fn read_quoted(chars: &[char], start: usize, line: usize) -> Result<(String, usize)> {
    let q = chars[start];
    let mut i = start + 1;
    let mut s = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            s.push(match n {
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            i += 2;
            continue;
        }
        if c == q {
            return Ok((s, i + 1));
        }
        s.push(c);
        i += 1;
    }
    bail!("line {line}: unterminated {q}quote{q}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let c = parse_script("add m1 V1 at=0 in=12 out=25.5\ntext 'Hello world' size=96 color=#fff; cut c1 at=5").unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].verb, "add");
        assert_eq!(c[0].pos(), vec!["m1", "V1"]);
        assert_eq!(c[0].kv()[1], ("in".into(), "12".into()));
        assert_eq!(c[1].pos(), vec!["Hello world"]);
        assert_eq!(c[1].kv()[1], ("color".into(), "#fff".into()));
        assert_eq!(c[2].verb, "cut");
        assert_eq!(c[2].line, 2);
    }

    #[test]
    fn comments_and_quotes() {
        let c = parse_script("# comment\n// another\nset t1 text=\"it's \\\"fine\\\"\" // trailing\n  \n").unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kv()[0].1, "it's \"fine\"");
        let c = parse_script("solid #000 at=0").unwrap();
        assert_eq!(c[0].pos(), vec!["#000"]);
    }

    #[test]
    fn unterminated() {
        assert!(parse_script("text 'oops").is_err());
    }
}
