#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SubcommandSegment {
    pub raw: String,
    pub piped_from_previous: bool,
    pub pipes_into_next: bool,
}

/// Detects unquoted subshells `(...)`, brace groups `{...}`, backticks `` `...` ``,
/// `$` variable/command/ANSI-C expansions, or unbalanced quotes.
pub(super) fn has_unsafe_shell_expansion(cmd: &str) -> bool {
    let chars: Vec<char> = cmd.chars().collect();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && !in_single {
            i += 2;
            continue;
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if !in_single && (c == '`' || c == '$') {
            return true;
        }
        if !in_single && !in_double && matches!(c, '(' | ')' | '{' | '}') {
            return true;
        }
        i += 1;
    }

    // Unbalanced quotes cannot be safely reasoned about.
    in_single || in_double
}

#[allow(clippy::too_many_lines)]
pub(super) fn split_shell_segments(cmd: &str) -> Option<Vec<SubcommandSegment>> {
    let chars: Vec<char> = cmd.chars().collect();
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_was_pipe = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && !in_single {
            current.push(c);
            if let Some(&next) = chars.get(i + 1) {
                current.push(next);
            }
            i += 2;
            continue;
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            current.push(c);
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            current.push(c);
            i += 1;
            continue;
        }

        if !in_single && !in_double {
            if c == ';' || c == '\n' {
                let seg_str = current.trim().to_string();
                if !seg_str.is_empty() {
                    segments.push(SubcommandSegment {
                        raw: seg_str,
                        piped_from_previous: prev_was_pipe,
                        pipes_into_next: false,
                    });
                } else if prev_was_pipe {
                    return None;
                }
                current.clear();
                prev_was_pipe = false;
                i += 1;
                continue;
            }
            if c == '&' {
                if chars.get(i + 1) == Some(&'&') {
                    let seg_str = current.trim().to_string();
                    if !seg_str.is_empty() {
                        segments.push(SubcommandSegment {
                            raw: seg_str,
                            piped_from_previous: prev_was_pipe,
                            pipes_into_next: false,
                        });
                    } else if prev_was_pipe {
                        return None;
                    }
                    current.clear();
                    prev_was_pipe = false;
                    i += 2;
                    continue;
                } else if chars.get(i + 1) == Some(&'>')
                    || (i > 0 && chars.get(i - 1) == Some(&'>'))
                {
                    // `&>` or `2>&1` redirection stays inside the current subcommand
                    current.push(c);
                    i += 1;
                    continue;
                }
                // Background `&` operator
                let seg_str = current.trim().to_string();
                if !seg_str.is_empty() {
                    segments.push(SubcommandSegment {
                        raw: seg_str,
                        piped_from_previous: prev_was_pipe,
                        pipes_into_next: false,
                    });
                }
                current.clear();
                prev_was_pipe = false;
                i += 1;
                continue;
            }
            if c == '|' {
                if i > 0 && chars.get(i - 1) == Some(&'>') {
                    // `>|` noclobber redirection stays inside the current subcommand
                    current.push(c);
                    i += 1;
                    continue;
                } else if chars.get(i + 1) == Some(&'|') {
                    let seg_str = current.trim().to_string();
                    if !seg_str.is_empty() {
                        segments.push(SubcommandSegment {
                            raw: seg_str,
                            piped_from_previous: prev_was_pipe,
                            pipes_into_next: false,
                        });
                    } else if prev_was_pipe {
                        return None;
                    }
                    current.clear();
                    prev_was_pipe = false;
                    i += 2;
                    continue;
                }
                let seg_str = current.trim().to_string();
                if seg_str.is_empty() {
                    return None;
                }
                segments.push(SubcommandSegment {
                    raw: seg_str,
                    piped_from_previous: prev_was_pipe,
                    pipes_into_next: true,
                });
                current.clear();
                prev_was_pipe = true;
                i += 1;
                continue;
            }
        }

        current.push(c);
        i += 1;
    }

    let tail = current.trim().to_string();
    if !tail.is_empty() {
        segments.push(SubcommandSegment {
            raw: tail,
            piped_from_previous: prev_was_pipe,
            pipes_into_next: false,
        });
    } else if prev_was_pipe {
        return None;
    }

    Some(segments)
}

/// Tokenizes a single subcommand while extracting any input/output redirection targets
/// (`<`, `<>`, `>`, `>>`, `>|`, `&>`). Rejects heredocs (`<<`) and herestrings (`<<<`).
#[allow(clippy::too_many_lines)]
pub(super) fn tokenize_subcommand(raw: &str) -> Option<(Vec<String>, Vec<String>)> {
    let chars: Vec<char> = raw.chars().collect();
    let mut tokens = Vec::new();
    let mut redirects = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut next_is_redirect_target = false;
    let mut i = 0;

    let push_token = |tok: &mut String,
                      tokens: &mut Vec<String>,
                      redirects: &mut Vec<String>,
                      next_redir: &mut bool| {
        if !tok.is_empty() {
            if *next_redir {
                redirects.push(std::mem::take(tok));
                *next_redir = false;
            } else {
                tokens.push(std::mem::take(tok));
            }
        }
    };

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && !in_single {
            if let Some(&next) = chars.get(i + 1) {
                current.push(next);
            }
            i += 2;
            continue;
        }
        if c == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }

        if !in_single && !in_double {
            if c.is_whitespace() {
                push_token(
                    &mut current,
                    &mut tokens,
                    &mut redirects,
                    &mut next_is_redirect_target,
                );
                i += 1;
                continue;
            }
            if c == '<' {
                if next_is_redirect_target {
                    return None;
                }
                if !current.is_empty() && current.chars().all(|ch| ch.is_ascii_digit()) {
                    current.clear();
                } else {
                    push_token(
                        &mut current,
                        &mut tokens,
                        &mut redirects,
                        &mut next_is_redirect_target,
                    );
                }
                match chars.get(i + 1) {
                    Some('<' | '&') => return None,
                    Some('>') => i += 2,
                    _ => i += 1,
                }
                next_is_redirect_target = true;
                continue;
            }
            if c == '>' {
                if next_is_redirect_target {
                    return None;
                }
                if current == "&"
                    || (!current.is_empty() && current.chars().all(|ch| ch.is_ascii_digit()))
                {
                    current.clear();
                } else {
                    push_token(
                        &mut current,
                        &mut tokens,
                        &mut redirects,
                        &mut next_is_redirect_target,
                    );
                }
                match chars.get(i + 1) {
                    Some('>' | '|') => i += 2,
                    Some('&') => {
                        i += 2;
                        let start = i;
                        while i < chars.len() && !chars[i].is_whitespace() {
                            if !chars[i].is_ascii_digit() && chars[i] != '-' {
                                return None;
                            }
                            i += 1;
                        }
                        if i == start {
                            return None;
                        }
                        continue;
                    }
                    _ => i += 1,
                }
                next_is_redirect_target = true;
                continue;
            }
        }

        current.push(c);
        i += 1;
    }

    push_token(
        &mut current,
        &mut tokens,
        &mut redirects,
        &mut next_is_redirect_target,
    );

    if next_is_redirect_target {
        return None;
    }

    Some((tokens, redirects))
}
