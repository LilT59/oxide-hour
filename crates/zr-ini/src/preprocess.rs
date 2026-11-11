//! Line reader: comment stripping plus the `#define` / `#include` extensions.

use crate::{ErrorKind, ParseError};
use std::collections::HashMap;
use std::rc::Rc;

/// One logical source line after comment stripping and macro expansion.
pub(crate) struct Line {
    pub file: Rc<str>,
    pub no: u32,
    pub text: String,
}

const MAX_INCLUDE_DEPTH: usize = 16;

type Loader<'a> = &'a mut dyn FnMut(&str) -> Option<String>;

pub(crate) fn expand(file: &str, text: &str, loader: Loader) -> Result<Vec<Line>, ParseError> {
    let mut out = Vec::new();
    let mut defines = HashMap::new();
    expand_into(file, text, loader, &mut defines, 0, &mut out)?;
    Ok(out)
}

fn expand_into(
    file: &str,
    text: &str,
    loader: Loader,
    defines: &mut HashMap<String, String>,
    depth: usize,
    out: &mut Vec<Line>,
) -> Result<(), ParseError> {
    let file_rc: Rc<str> = Rc::from(file);
    let err = |no: u32, kind| ParseError {
        file: file.to_owned(),
        line: no,
        kind,
    };
    for (i, raw) in text.split('\n').enumerate() {
        let no = i as u32 + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let (word, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            let arg = arg.trim();
            match word.to_ascii_lowercase().as_str() {
                "define" => {
                    let (name, value) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
                    if name.is_empty() {
                        return Err(err(
                            no,
                            ErrorKind::Directive("#define without a name".into()),
                        ));
                    }
                    defines.insert(name.to_owned(), value.trim().to_owned());
                }
                "include" => {
                    let path = arg.trim_matches(|c| c == '"' || c == '<' || c == '>');
                    if path.is_empty() || depth >= MAX_INCLUDE_DEPTH {
                        return Err(err(no, ErrorKind::Include(format!("`{arg}`"))));
                    }
                    let Some(body) = loader(path) else {
                        return Err(err(no, ErrorKind::Include(format!("cannot load `{path}`"))));
                    };
                    expand_into(path, &body, loader, defines, depth + 1, out)?;
                }
                other => {
                    return Err(err(no, ErrorKind::Directive(format!("unknown `#{other}`"))));
                }
            }
            continue;
        }
        out.push(Line {
            file: file_rc.clone(),
            no,
            text: substitute(line, defines),
        });
    }
    Ok(())
}

/// Removes a `;` comment (the engine's only comment form) or a `//` comment
/// that starts a whitespace-delimited word (extension; keeps `http://` intact).
fn strip_comment(line: &str) -> &str {
    let mut cut = line.find(';').unwrap_or(line.len());
    let b = line.as_bytes();
    for p in 0..cut.saturating_sub(1) {
        if b[p] == b'/' && b[p + 1] == b'/' && (p == 0 || b[p - 1].is_ascii_whitespace()) {
            cut = p;
            break;
        }
    }
    &line[..cut]
}

/// Replaces whole words (split on the engine's separators) that name a macro.
fn substitute(line: &str, defines: &HashMap<String, String>) -> String {
    if defines.is_empty() {
        return line.to_owned();
    }
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(
            defines
                .get(word.as_str())
                .map_or(word.as_str(), String::as_str),
        );
        word.clear();
    };
    let mut out = String::with_capacity(line.len());
    let mut word = String::new();
    for c in line.chars() {
        if matches!(c, ' ' | '\t' | '\r' | '=') {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}
