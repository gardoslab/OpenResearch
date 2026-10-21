//! Standard Markdown, which is what an agent writes, to Slack `mrkdwn`, which
//! is what a Slack message renders.
//!
//! Slack's dialect differs where it hurts: bold is `*x*`, links are
//! `<url|text>`, there are no headings or tables, and `&`, `<` and `>` must be
//! sent as entities. Text that skips this arrives with its asterisks, brackets
//! and pipes showing. See <https://docs.slack.dev/messaging/formatting-message-text/>.
//!
//! Only text an agent wrote goes through [`from_markdown`]. Text orx composed
//! is already mrkdwn and only needs [`escape`] on the parts that came from
//! elsewhere; running mrkdwn through here would turn its `*bold*` into italics.

/// Slack's entity escapes, required for `&`, `<` and `>` anywhere in message
/// text, code included.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

// Private-use characters that cannot appear in the text itself: they stand in
// for finished pieces (links, code spans) and for bold while italics are sorted
// out, so no later step reads them as Markdown.
const SLOT_OPEN: char = '\u{E001}';
const SLOT_CLOSE: char = '\u{E002}';
const BOLD: char = '\u{E003}';

/// Converts agent-written Markdown to Slack mrkdwn.
pub fn from_markdown(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with("```") {
            // Contents are literal. The language tag goes: Slack has none.
            out.push("```".to_string());
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                out.push(escape(lines[i]));
                i += 1;
            }
            out.push("```".to_string());
            i += 1;
        } else if let Some(end) = table_end(&lines, i) {
            out.extend(table(&lines[i..end]));
            i = end;
        } else {
            out.push(block_line(line));
            i += 1;
        }
    }
    out.join("\n")
}

fn block_line(line: &str) -> String {
    let indent = &line[..line.len() - line.trim_start().len()];
    let rest = line.trim_start();

    if is_rule(rest) {
        return "\u{2500}".repeat(12);
    }
    let hashes = rest.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
        // Slack has no headings; a bold line is the nearest thing.
        let title = rest[hashes..].trim().trim_end_matches('#').trim_end();
        return format!("*{}*", inline(&title.replace("**", "")));
    }
    if let Some(quoted) = rest.strip_prefix('>') {
        return format!("> {}", inline(quoted.trim_start()));
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(item) = rest.strip_prefix(marker) {
            let (bullet, item) = if let Some(item) = item.strip_prefix("[ ] ") {
                ("\u{2610}", item)
            } else if let Some(item) = item
                .strip_prefix("[x] ")
                .or_else(|| item.strip_prefix("[X] "))
            {
                ("\u{2611}", item)
            } else {
                ("\u{2022}", item)
            };
            return format!("{indent}{bullet} {}", inline(item));
        }
    }
    format!("{indent}{}", inline(rest))
}

fn is_rule(line: &str) -> bool {
    let line = line.trim();
    let Some(first) = line.chars().next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_')
        && line.chars().filter(|c| !c.is_whitespace()).count() >= 3
        && line.chars().all(|c| c == first || c.is_whitespace())
}

// --- tables -----------------------------------------------------------------

fn table_cells(line: &str) -> Vec<String> {
    let line = line.trim().replace("\\|", "\u{E004}");
    let line = line.strip_prefix('|').unwrap_or(&line);
    let line = line.strip_suffix('|').unwrap_or(line);
    line.split('|')
        .map(|cell| cell.trim().replace('\u{E004}', "|"))
        .collect()
}

fn is_separator(line: &str) -> bool {
    line.contains('-')
        && line.contains('|')
        && table_cells(line)
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':' | ' ')))
}

/// The line after a table that starts at `start`, or `None` when no table does.
fn table_end(lines: &[&str], start: usize) -> Option<usize> {
    let header = lines.get(start)?;
    if !header.contains('|') || !is_separator(lines.get(start + 1)?) {
        return None;
    }
    let mut end = start + 2;
    while end < lines.len() && lines[end].contains('|') && !lines[end].trim().is_empty() {
        end += 1;
    }
    Some(end)
}

/// Slack cannot draw a table, so it becomes aligned columns in a code block,
/// which keeps them lined up in a monospace font.
fn table(lines: &[&str]) -> Vec<String> {
    let rows: Vec<Vec<String>> = lines
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 1)
        .map(|(_, line)| {
            table_cells(line)
                .iter()
                .map(|cell| plain(cell))
                .collect::<Vec<_>>()
        })
        .collect();
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let render = |row: &Vec<String>| {
        let cells: Vec<String> = (0..columns)
            .map(|column| {
                let cell = row.get(column).map(String::as_str).unwrap_or("");
                let pad = widths[column] - cell.chars().count();
                format!("{}{}", escape(cell), " ".repeat(pad))
            })
            .collect();
        cells.join(" | ").trim_end().to_string()
    };
    let rule = widths
        .iter()
        .map(|width| "-".repeat(*width))
        .collect::<Vec<_>>()
        .join("-+-");
    let mut out = vec!["```".to_string(), render(&rows[0]), rule];
    out.extend(rows[1..].iter().map(render));
    out.push("```".to_string());
    out
}

/// Cell text with its Markdown stripped, for use inside a code block where
/// formatting cannot show.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((label, _, used)) = link_at(rest) {
            out.push_str(&label);
            rest = &rest[used..];
            continue;
        }
        let c = rest.chars().next().unwrap();
        if c != '`' && c != '*' && c != '~' {
            out.push(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    out
}

// --- inline -----------------------------------------------------------------

/// `[label](url)` or `![label](url)` at the start of `s`: its label, its URL,
/// and how many bytes it spans.
fn link_at(s: &str) -> Option<(String, String, usize)> {
    let body = s.strip_prefix('!').unwrap_or(s);
    let skipped = s.len() - body.len();
    let after_bracket = body.strip_prefix('[')?;
    let close = after_bracket.find("](")?;
    let label = &after_bracket[..close];
    if label.contains('[') {
        return None;
    }
    let url_part = &after_bracket[close + 2..];
    let mut depth = 0;
    let mut url_end = None;
    for (index, c) in url_part.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                url_end = Some(index);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let url = url_part[..url_end?].trim();
    let used = skipped + 1 + close + 2 + url_end? + 1;
    Some((label.to_string(), url.to_string(), used))
}

fn is_web_url(url: &str) -> bool {
    ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
        && !url.contains(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '|'))
}

/// The value of `name="..."` inside an HTML-ish tag like `<file path="x" />`.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let after = &tag[tag.find(&format!("{name}=\""))? + name.len() + 2..];
    Some(&after[..after.find('"')?])
}

/// A finished mrkdwn piece found at the start of `s`, and the bytes it used:
/// a code span, a link, or one of the reference tags agents emit in chat.
fn piece_at(s: &str) -> Option<(String, usize)> {
    if s.starts_with('`') {
        let ticks = s.chars().take_while(|&c| c == '`').count();
        let fence = &s[..ticks];
        let close = s[ticks..].find(fence)? + ticks;
        let code = s[ticks..close].trim();
        return Some((format!("`{}`", escape(code)), close + ticks));
    }
    if s.starts_with('[') || s.starts_with("![") {
        let (label, url, used) = link_at(s)?;
        let label = if label.trim().is_empty() {
            url.clone()
        } else {
            label
        };
        let label = escape(&label.replace('|', "\u{a6}"));
        return Some(if is_web_url(&url) {
            (format!("<{url}|{label}>"), used)
        } else {
            // A relative path or a scheme Slack would not open: the label
            // alone beats a dead link.
            (label, used)
        });
    }
    if let Some(inner) = s.strip_prefix('<') {
        let end = inner.find('>')?;
        let tag = &inner[..end];
        let used = end + 2;
        if is_web_url(tag) {
            return Some((format!("<{tag}>"), used));
        }
        // The chat's own reference tags mean nothing in Slack; keep what they point at.
        if tag.starts_with("file ") {
            let path = attribute(tag, "path")?;
            return Some((format!("`{}`", escape(path)), used));
        }
        if tag.starts_with("run ") {
            let shown = attribute(tag, "label").or_else(|| attribute(tag, "id"))?;
            return Some((format!("`{}`", escape(shown)), used));
        }
    }
    None
}

fn inline(text: &str) -> String {
    let mut pieces: Vec<String> = Vec::new();
    let mut prose = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some((piece, used)) = piece_at(rest) {
            prose.push(SLOT_OPEN);
            prose.push_str(&pieces.len().to_string());
            prose.push(SLOT_CLOSE);
            pieces.push(piece);
            rest = &rest[used..];
            continue;
        }
        let c = rest.chars().next().unwrap();
        prose.push(c);
        rest = &rest[c.len_utf8()..];
    }

    let prose = escape(&prose);
    let prose = pair(&prose, "**", &BOLD.to_string(), &BOLD.to_string());
    let prose = pair(&prose, "~~", "~", "~");
    let prose = pair(&prose, "*", "_", "_");
    let mut out = prose.replace(BOLD, "*");
    for (index, piece) in pieces.iter().enumerate() {
        out = out.replace(&format!("{SLOT_OPEN}{index}{SLOT_CLOSE}"), piece);
    }
    out
}

/// Rewrites each `delim`-wrapped span to `open`..`close`. A delimiter with
/// whitespace on its inside, or no partner, is ordinary text (`3 * 4 * 5`).
fn pair(text: &str, delim: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(delim) {
        let after = &rest[start + delim.len()..];
        let closing = after
            .match_indices(delim)
            .map(|(index, _)| index)
            .find(|&index| index > 0 && !after[..index].ends_with(char::is_whitespace));
        match closing {
            Some(end) if !after.starts_with(char::is_whitespace) => {
                out.push_str(&rest[..start]);
                out.push_str(open);
                out.push_str(&after[..end]);
                out.push_str(close);
                rest = &after[end + delim.len()..];
            }
            _ => {
                out.push_str(&rest[..start + delim.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_links_and_entities_use_slack_syntax() {
        assert_eq!(
            from_markdown(
                "**Done**: see [the log](https://x.test/a?b=1&c=2), _really_ ~~not~~ *good* & fast"
            ),
            "*Done*: see <https://x.test/a?b=1&c=2|the log>, _really_ ~not~ _good_ &amp; fast"
        );
        assert_eq!(
            from_markdown("3 * 4 * 5 and a < b"),
            "3 * 4 * 5 and a &lt; b"
        );
    }

    #[test]
    fn link_urls_keep_their_ampersands_and_unusable_links_keep_their_label() {
        assert_eq!(
            from_markdown("[a](https://x.test?a=1&b=2)"),
            "<https://x.test?a=1&b=2|a>"
        );
        assert_eq!(from_markdown("[plot](artifacts/plot.png)"), "plot");
        assert_eq!(from_markdown("![curve](artifacts/c.png)"), "curve");
        assert_eq!(from_markdown("<https://x.test>"), "<https://x.test>");
    }

    #[test]
    fn headings_lists_quotes_and_rules() {
        let md = "# Results\n## Next steps\n- one\n  - nested\n* two\n1. first\n- [x] done\n- [ ] todo\n> note\n---";
        assert_eq!(
            from_markdown(md),
            "*Results*\n*Next steps*\n\u{2022} one\n  \u{2022} nested\n\u{2022} two\n1. first\n\u{2611} done\n\u{2610} todo\n> note\n\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"
        );
    }

    #[test]
    fn code_is_left_alone_apart_from_entities() {
        assert_eq!(
            from_markdown("run `a **b** <c>` now\n```python\nx = 1 * 2 < 3\n```"),
            "run `a **b** &lt;c&gt;` now\n```\nx = 1 * 2 &lt; 3\n```"
        );
    }

    #[test]
    fn tables_become_aligned_code_blocks() {
        let md = "Scores:\n\n| Model | **F1** |\n|:--|--:|\n| base | 0.81 |\n| [big](https://x.test) | 0.9 |\n\nDone";
        assert_eq!(
            from_markdown(md),
            "Scores:\n\n```\nModel | F1\n------+-----\nbase  | 0.81\nbig   | 0.9\n```\n\nDone"
        );
    }

    #[test]
    fn chat_reference_tags_keep_what_they_point_at() {
        assert_eq!(
            from_markdown(
                r#"See <file path="src/a.py" lines="1-3" /> and <run id="run_9" label="+3.6pp" />."#
            ),
            "See `src/a.py` and `+3.6pp`."
        );
    }

    #[test]
    fn nothing_is_lost_on_odd_input() {
        assert_eq!(from_markdown("a [b](c"), "a [b](c");
        assert_eq!(from_markdown("**unclosed"), "**unclosed");
        assert_eq!(from_markdown("snake_case_name"), "snake_case_name");
        assert_eq!(from_markdown(""), "");
    }
}
