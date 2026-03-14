pub enum Segment {
    Text(String),
    Code { lang: String, code: String },
}

/// Split `input` into alternating text / fenced-code segments.
pub fn parse_segments(input: &str) -> Vec<Segment> {
    let mut result = Vec::new();
    let mut remaining = input;

    while !remaining.is_empty() {
        match remaining.find("```") {
            None => {
                result.push(Segment::Text(remaining.to_string()));
                break;
            }
            Some(fence_start) => {
                if fence_start > 0 {
                    result.push(Segment::Text(remaining[..fence_start].to_string()));
                }
                let after_fence = &remaining[fence_start + 3..];

                // First line is the language identifier (may be empty)
                let (lang, body) = match after_fence.find('\n') {
                    Some(nl) => (after_fence[..nl].trim().to_string(), &after_fence[nl + 1..]),
                    None => (String::new(), after_fence),
                };

                match body.find("```") {
                    Some(code_end) => {
                        let code = body[..code_end].trim_end_matches('\n').to_string();
                        result.push(Segment::Code { lang, code });
                        let tail = &body[code_end + 3..];
                        remaining = tail.strip_prefix('\n').unwrap_or(tail);
                    }
                    None => {
                        // Unclosed fence — treat everything as text
                        result.push(Segment::Text(remaining.to_string()));
                        break;
                    }
                }
            }
        }
    }
    result
}

/// Convert a plain-text paragraph that may contain **bold**, `code`, and
/// _italic_ into Pango markup. XML special characters are escaped first.
pub fn to_pango(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 64);
    let mut s = input;

    while !s.is_empty() {
        // **bold**
        if s.starts_with("**")
            && let Some(p) = s[2..].find("**") {
                out.push_str("<b>");
                push_escaped(&mut out, &s[2..2 + p]);
                out.push_str("</b>");
                s = &s[4 + p..];
                continue;
            }
        // `inline code`  (not ``` which starts a fence)
        if s.starts_with('`') && !s.starts_with("```")
            && let Some(p) = s[1..].find('`') {
                out.push_str("<tt>");
                push_escaped(&mut out, &s[1..1 + p]);
                out.push_str("</tt>");
                s = &s[2 + p..];
                continue;
            }
        // _italic_
        if s.starts_with('_')
            && let Some(p) = s[1..].find('_') {
                out.push_str("<i>");
                push_escaped(&mut out, &s[1..1 + p]);
                out.push_str("</i>");
                s = &s[2 + p..];
                continue;
            }
        // Plain character — escape XML
        let c = s.chars().next().unwrap();
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
        s = &s[c.len_utf8()..];
    }
    out
}

pub fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
}
