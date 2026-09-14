//! Plain text to Markdown.
//!
//! Plain mail is very nearly Markdown already — `>` is a quote in both, `*` has meant emphasis
//! since before either existed, and a list is a list. So this does not translate; it decides how
//! much of the text's *shape* is meaningful and lets the rest through untouched. That is also what
//! makes `text/markdown` free: a message somebody actually wrote in Markdown takes this same road
//! and comes out as what they typed.
//!
//! Three things need deciding:
//!
//! - **Where the lines really end.** A mail hard-wrapped at 72 columns should reflow into whatever
//!   pane it is read in; a signature, a table drawn in spaces or a code paste must not. RFC 3676
//!   `format=flowed` says which is which, and when the sender did not say, [`wrapping`] guesses
//!   from the shape of the text rather than reflowing everything and hoping.
//! - **Quote depth.** `>>` is two levels in mail and two levels in Markdown, so this is a matter of
//!   rewriting `>>text` as `> > text` and letting the renderer nest them.
//! - **Links.** A bare URL is a link in mail by convention and in Markdown only inside angle
//!   brackets, so bare ones get them.

/// Converts one plain-text body, given whatever `Content-Type` said about it.
pub fn to_markdown(text: &str, flowed: bool, delete_space: bool) -> String {
    let lines: Vec<Line> = text.lines().map(Line::split).collect();
    let reflow = flowed || wrapping(&lines);
    let mut out = String::new();
    let mut index = 0;

    while index < lines.len() {
        let line = &lines[index];
        // In a flowed message a line ending in a space continues into the next one at the same
        // quote depth; everywhere else each line stands alone.
        let mut joined = line.text.to_string();
        let mut last = index;
        if reflow {
            while lines[last].continues(flowed)
                && lines.get(last + 1).is_some_and(|next| next.quote == line.quote && !next.text.is_empty())
            {
                if flowed && delete_space {
                    joined.pop();
                } else if !flowed {
                    // A `format=flowed` line ends in the space that marks the flow, so joining is
                    // concatenation. A line a mail client merely hard-wrapped does not, and
                    // joining two of those without putting the space back gives "in unixi'm".
                    joined.push(' ');
                }
                joined.push_str(lines[last + 1].text);
                last += 1;
            }
        }

        let prefix = "> ".repeat(line.quote);
        let body = autolink(joined.trim_end());
        if body.is_empty() {
            // A blank line inside a quote has to keep the quote, or the quote ends there.
            out.push_str(prefix.trim_end());
        } else {
            out.push_str(&prefix);
            out.push_str(&body);
            // Without reflow every line break is one the sender meant, and Markdown needs telling.
            // Two spaces rather than a trailing backslash: a backslash left at the end of a
            // paragraph renders as a backslash, and trailing spaces are simply dropped.
            // A hard break belongs between two lines of prose, not around a line Markdown is
            // already going to treat as its own block: a list item, a heading, a fence.
            let ends_block = lines.get(last + 1).is_none_or(|next| next.quote != line.quote || next.text.is_empty());
            let structural = starts_a_block(&body) || lines.get(last + 1).is_some_and(|next| starts_a_block(next.text));
            if !reflow && !ends_block && !structural {
                out.push_str("  ");
            }
        }
        out.push('\n');
        index = last + 1;
    }
    out.trim_end().to_string()
}

/// One source line, split into its quote depth and what it says.
struct Line<'a> {
    quote: usize,
    text: &'a str,
}

impl<'a> Line<'a> {
    fn split(line: &'a str) -> Line<'a> {
        let mut rest = line;
        let mut quote = 0;
        loop {
            let trimmed = rest.strip_prefix(' ').unwrap_or(rest);
            match trimmed.strip_prefix('>') {
                Some(after) => {
                    quote += 1;
                    rest = after;
                }
                None => break,
            }
        }
        // RFC 3676 space-stuffing: one leading space is armour for a line that would otherwise
        // start with a space, a `>`, or `From `, and is not part of the text.
        Line { quote, text: rest.strip_prefix(' ').unwrap_or(rest) }
    }

    /// Whether this line runs into the next. In `format=flowed` that is exactly a trailing space;
    /// guessing, it is a line long enough to have been wrapped rather than ended.
    fn continues(&self, flowed: bool) -> bool {
        if self.text.is_empty() {
            return false;
        }
        if flowed {
            return self.text.ends_with(' ');
        }
        self.text.len() >= WRAP_AT && !self.text.ends_with(['.', '!', '?', ':', ';'])
    }
}

/// The column a hard-wrapped mail is wrapped at, near enough. Mail clients wrap at 72 or 76; a
/// line that reaches 64 characters and keeps going was wrapped by a machine, not ended by a person.
const WRAP_AT: usize = 64;

/// Whether this text looks like a paragraph a machine wrapped, as opposed to lines a person meant.
///
/// The tell is uniformity: hard-wrapped prose is a run of long lines that all stop in the same
/// narrow band, because they all stopped for the same reason. A signature, a table drawn in spaces,
/// a code paste or a shopping list has lines of every length, and reflowing it destroys it. So the
/// bar is high: most of the body has to be long lines before any of it is joined.
fn wrapping(lines: &[Line<'_>]) -> bool {
    let content: Vec<usize> =
        lines.iter().filter(|line| !line.text.trim().is_empty()).map(|line| line.text.len()).collect();
    if content.len() < 3 {
        return false;
    }
    let long = content.iter().filter(|length| **length >= WRAP_AT).count();
    // Indentation anywhere is a strong sign the layout is deliberate.
    let indented = lines.iter().any(|line| line.text.starts_with("  ") || line.text.starts_with('\t'));
    !indented && long * 2 > content.len()
}

/// Whether this line is something Markdown already reads as the start of a block, and so does not
/// need — or want — to be told that the line before it ended.
fn starts_a_block(line: &str) -> bool {
    let line = line.trim_start();
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return true;
    }
    if line.starts_with('#') || line.starts_with('>') || line.starts_with('|') {
        return true;
    }
    if line.starts_with("```") || line.starts_with("~~~") || line.starts_with("---") {
        return true;
    }
    // `1. ` and `1) `, an ordered list.
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    !digits.is_empty() && line[digits.len()..].starts_with(['.', ')'])
}

/// The prefixes that make a bare URL a link. `www.` is here because it is what people type.
const LINKY: &[&str] = &["https://", "http://", "ftp://", "mailto:", "www."];

/// Wraps bare URLs in the angle brackets Markdown needs to see them as links.
///
/// A URL that is already inside Markdown link syntax is left alone: the give-away is the character
/// in front of it, which is `(` or `<` in every spelling that already works.
fn autolink(line: &str) -> String {
    if !LINKY.iter().any(|prefix| line.contains(prefix)) {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 2);
    let mut rest = line;
    'scan: while !rest.is_empty() {
        let found = LINKY.iter().filter_map(|prefix| rest.find(prefix).map(|at| (at, *prefix))).min();
        let Some((at, prefix)) = found else { break };
        let before = out.chars().last().or_else(|| rest[..at].chars().last());
        let end = rest[at..].find(char::is_whitespace).map_or(rest.len(), |offset| at + offset);
        // Sentence punctuation belongs to the sentence, not to the address.
        let url = rest[at..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'', '>']);
        out.push_str(&rest[..at]);
        // Already a link, or already in brackets.
        if let Some('(' | '<' | '[') = before {
            out.push_str(&rest[at..end]);
            rest = &rest[end..];
            continue 'scan;
        }
        out.push('<');
        if prefix == "www." {
            out.push_str("https://");
        }
        out.push_str(url);
        out.push('>');
        out.push_str(&rest[at + url.len()..end]);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quote_becomes_a_quote_at_the_depth_it_was_written_at() {
        let text = "> they said\n>> and before that\nmy reply";
        assert_eq!(to_markdown(text, false, false), "> they said\n> > and before that\nmy reply");
    }

    /// A blank line inside a quote is still inside the quote.
    #[test]
    fn a_blank_line_inside_a_quote_keeps_the_quote() {
        assert_eq!(to_markdown("> one\n>\n> two", false, false), "> one\n>\n> two");
    }

    #[test]
    fn a_flowed_message_reflows_and_a_fixed_one_keeps_its_lines() {
        let flowed = "This is a long line that \nkeeps going.\nAnd this one does not.";
        assert_eq!(to_markdown(flowed, true, false), "This is a long line that keeps going.\nAnd this one does not.");
        // delsp=yes means the space that marked the flow is not part of the text.
        assert_eq!(to_markdown("half \nway", true, true), "halfway");
    }

    /// Two trailing spaces, not a backslash: a backslash at the end of a paragraph renders as one.
    #[test]
    fn a_line_the_sender_meant_to_end_is_told_to_markdown_as_a_hard_break() {
        let signature = "Jae\nnoctalia.dev\n+1 555 0100";
        let out = to_markdown(signature, false, false);
        assert_eq!(out, "Jae  \nnoctalia.dev  \n+1 555 0100");
        assert!(!out.ends_with(' '), "the last line of a block owes nothing to the next one");
    }

    /// The thing hard-wrapped mail must not do to a signature or an ASCII table.
    #[test]
    fn deliberate_short_lines_are_never_reflowed_and_wrapped_prose_is() {
        let art = "  +------+\n  | box  |\n  +------+\n  done";
        assert!(!wrapping(&art.lines().map(Line::split).collect::<Vec<_>>()), "indentation is deliberate");

        let prose = "Thunderbird retains a great deal about a message even though it keeps no\n\
             maildir at all, which is the thing that took the longest to establish and\n\
             is why the plan lost a milestone between one draft and the next one out.";
        assert!(wrapping(&prose.lines().map(Line::split).collect::<Vec<_>>()));
        let reflowed = to_markdown(prose, false, false);
        assert!(!reflowed.contains('\n'), "it reflows into one paragraph");
        // The wrap ate a space when it broke the line; putting the lines back has to put it back.
        assert!(reflowed.contains("keeps no\nmaildir") || reflowed.contains("keeps no maildir"), "{reflowed}");
        assert!(!reflowed.contains("nomaildir"), "the wrapped words ran together: {reflowed}");
    }

    #[test]
    fn a_bare_url_becomes_a_link_and_one_that_already_is_stays_put() {
        assert_eq!(
            to_markdown("see https://example.com/x for more", false, false),
            "see <https://example.com/x> for more"
        );
        assert_eq!(to_markdown("at www.example.com.", false, false), "at <https://www.example.com>.");
        // Sentence punctuation is not part of the address.
        assert_eq!(
            to_markdown("go to https://example.com/a, then home", false, false),
            "go to <https://example.com/a>, then home"
        );
        // Already-linked text is left exactly as written.
        assert_eq!(to_markdown("[home](https://example.com)", false, false), "[home](https://example.com)");
        assert_eq!(to_markdown("<https://example.com>", false, false), "<https://example.com>");
    }

    #[test]
    fn markdown_a_person_typed_arrives_as_markdown() {
        let typed = "Here is **the thing**:\n\n- one\n- two\n\nSee `mail.rs`.";
        assert_eq!(to_markdown(typed, false, false), typed);
    }

    #[test]
    fn an_empty_body_is_an_empty_body() {
        assert_eq!(to_markdown("", false, false), "");
        assert_eq!(to_markdown("\n\n\n", false, false), "");
    }
}
