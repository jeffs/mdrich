//! Quill deltas, the editor state Slack puts on the pasteboard.
//!
//! A delta is `{"ops": [...]}`, where each op inserts either a run of text or
//! an embed object. Attributes on a text op style its characters; attributes
//! on the newline that ends a line style the whole line. Neither Quill's
//! vocabulary nor Slack's use of it is specified anywhere authoritative, so
//! this recognizes the conventional keys and ignores the rest.

use serde_json::{Map, Value};

type Attributes = Map<String, Value>;

pub fn to_markdown(delta: &str) -> crate::Result<String> {
    let delta: Value = serde_json::from_str(delta)?;
    let ops = delta
        .get("ops")
        .and_then(Value::as_array)
        .ok_or("delta has no ops")?;

    let mut document = Document::default();
    for op in ops {
        document.push(op);
    }
    Ok(document.render())
}

/// The kind of line a set of line attributes describes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Paragraph,
    Heading(usize),
    Quote,
    Code,
    Item { marker: Marker, indent: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Marker {
    Bullet,
    Ordered,
    Task { done: bool },
}

impl Block {
    fn from_attributes(attributes: Option<&Attributes>) -> Self {
        let Some(attributes) = attributes else {
            return Self::Paragraph;
        };
        if flag(attributes, "code-block") {
            return Self::Code;
        }
        if flag(attributes, "blockquote") {
            return Self::Quote;
        }
        if let Some(level) = attributes.get("header").and_then(Value::as_u64) {
            return Self::Heading(usize::try_from(level.clamp(1, 6)).unwrap_or(1));
        }
        if let Some(list) = attributes.get("list").and_then(Value::as_str) {
            let marker = match list {
                "ordered" => Marker::Ordered,
                "checked" => Marker::Task { done: true },
                "unchecked" => Marker::Task { done: false },
                _ => Marker::Bullet,
            };
            let indent = attributes
                .get("indent")
                .and_then(Value::as_u64)
                .and_then(|indent| usize::try_from(indent).ok())
                .unwrap_or(0);
            return Self::Item { marker, indent };
        }
        Self::Paragraph
    }

    /// Whether a line of this kind belongs to the same Markdown block as the
    /// line before it, which must therefore not be separated by a blank line.
    fn continues(self, previous: Self) -> bool {
        matches!(
            (previous, self),
            (Self::Paragraph, Self::Paragraph)
                | (Self::Quote, Self::Quote)
                | (Self::Code, Self::Code)
                | (Self::Item { .. }, Self::Item { .. })
        )
    }
}

/// An attribute Quill treats as set. Quill 2 gives `code-block` the language
/// as a string rather than `true`, so anything but `false` and `null` counts.
fn flag(attributes: &Attributes, key: &str) -> bool {
    attributes
        .get(key)
        .is_some_and(|value| !matches!(value, Value::Bool(false) | Value::Null))
}

struct Line {
    block: Block,
    /// The line rendered as Markdown, for every block but `Code`.
    styled: String,
    /// The line's characters as typed, for `Code`, which takes no markup.
    literal: String,
}

#[derive(Default)]
struct Document {
    lines: Vec<Line>,
    styled: String,
    literal: String,
}

impl Document {
    fn push(&mut self, op: &Value) {
        let attributes = op.get("attributes").and_then(Value::as_object);
        match op.get("insert") {
            Some(Value::String(text)) => self.push_text(text, attributes),
            Some(embed) => {
                if let Some(text) = embed_text(embed) {
                    self.styled.push_str(&inline::escape(&text));
                    self.literal.push_str(&text);
                }
            }
            None => {}
        }
    }

    fn push_text(&mut self, text: &str, attributes: Option<&Attributes>) {
        if !text.contains('\n') {
            self.styled.push_str(&inline::style(text, attributes));
            self.literal.push_str(text);
            return;
        }
        // Attributes on an op that spans a line break describe the line
        // rather than its characters, so the text itself goes in unstyled.
        let mut segments = text.split('\n').peekable();
        while let Some(segment) = segments.next() {
            self.styled.push_str(&inline::escape(segment));
            self.literal.push_str(segment);
            if segments.peek().is_some() {
                self.end_line(attributes);
            }
        }
    }

    fn end_line(&mut self, attributes: Option<&Attributes>) {
        self.lines.push(Line {
            block: Block::from_attributes(attributes),
            styled: std::mem::take(&mut self.styled),
            literal: std::mem::take(&mut self.literal),
        });
    }

    fn render(mut self) -> String {
        if !self.styled.is_empty() || !self.literal.is_empty() {
            self.end_line(None);
        }

        let mut markdown = Emitter::default();
        let mut previous: Option<Block> = None;
        let mut ordinals = Ordinals::default();

        for line in &self.lines {
            if previous == Some(Block::Code) && line.block != Block::Code {
                markdown.push("```");
            }
            if previous.is_none_or(|previous| !line.block.continues(previous)) {
                markdown.separate();
                ordinals.reset();
                if line.block == Block::Code {
                    markdown.push("```");
                }
            }
            match &line.block {
                Block::Paragraph if line.styled.is_empty() => markdown.separate(),
                Block::Paragraph => markdown.push(&escape_leader(&line.styled)),
                Block::Heading(level) => {
                    markdown.push(&format!("{} {}", "#".repeat(*level), line.styled));
                }
                Block::Quote if line.styled.is_empty() => markdown.push(">"),
                Block::Quote => markdown.push(&format!("> {}", line.styled)),
                Block::Code => markdown.push(&line.literal),
                Block::Item { marker, indent } => {
                    let bullet = ordinals.marker(*marker, *indent);
                    markdown.push(&format!("{}{bullet}{}", "  ".repeat(*indent), line.styled));
                }
            }
            previous = Some(line.block);
        }
        if previous == Some(Block::Code) {
            markdown.push("```");
        }
        markdown.finish()
    }
}

/// Renders an embed insert. Slack documents neither the embeds it emits nor
/// their shapes, so this takes the first plausible display string it finds.
fn embed_text(embed: &Value) -> Option<String> {
    const NAMES: [&str; 6] = ["text", "name", "label", "title", "alt", "url"];
    let (_, value) = embed.as_object()?.iter().next()?;
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(fields) => NAMES
            .iter()
            .find_map(|name| fields.get(*name).and_then(Value::as_str))
            .map(ToOwned::to_owned),
        _ => None,
    }
}

/// Escapes a leading character that would otherwise open a block: a heading,
/// a quote, a list item, or a setext underline.
fn escape_leader(line: &str) -> String {
    let leader = line.trim_start_matches(' ');
    let opener = leader
        .split_once(' ')
        .map_or(leader, |(opener, _)| opener)
        .trim_end_matches(['.', ')']);
    let opens_block = matches!(leader.chars().next(), Some('#' | '>' | '-' | '+' | '='))
        || (!opener.is_empty() && opener.chars().all(|digit| digit.is_ascii_digit()));
    if opens_block {
        let split = line.len() - leader.len();
        format!("{}\\{}", &line[..split], leader)
    } else {
        line.to_owned()
    }
}

/// The running numbers of the ordered lists open at each indent level.
#[derive(Default)]
struct Ordinals(Vec<usize>);

impl Ordinals {
    fn reset(&mut self) {
        self.0.clear();
    }

    fn marker(&mut self, marker: Marker, indent: usize) -> String {
        self.0.truncate(indent + 1);
        self.0.resize(indent + 1, 0);
        match marker {
            Marker::Bullet => "- ".to_owned(),
            Marker::Task { done: true } => "- [x] ".to_owned(),
            Marker::Task { done: false } => "- [ ] ".to_owned(),
            Marker::Ordered => {
                self.0[indent] += 1;
                format!("{}. ", self.0[indent])
            }
        }
    }
}

/// Accumulates output lines, collapsing runs of blank lines to one.
#[derive(Default)]
struct Emitter {
    out: String,
    owed: Owed,
}

#[derive(Default, PartialEq, Eq)]
enum Owed {
    #[default]
    Nothing,
    Break,
    Blank,
}

impl Emitter {
    fn push(&mut self, line: &str) {
        match self.owed {
            Owed::Nothing => {}
            Owed::Break => self.out.push('\n'),
            Owed::Blank => self.out.push_str("\n\n"),
        }
        self.out.push_str(line);
        self.owed = Owed::Break;
    }

    fn separate(&mut self) {
        if self.owed != Owed::Nothing {
            self.owed = Owed::Blank;
        }
    }

    fn finish(mut self) -> String {
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        self.out
    }
}

mod inline {
    use super::{Attributes, flag};
    use serde_json::Value;

    /// Wraps a run of text in the markup its attributes call for, keeping the
    /// markers against non-blank characters so that emphasis actually closes.
    pub fn style(text: &str, attributes: Option<&Attributes>) -> String {
        let Some(attributes) = attributes else {
            return escape(text);
        };
        let core = text.trim_matches(char::is_whitespace);
        if core.is_empty() {
            return escape(text);
        }
        let lead = &text[..text.len() - text.trim_start_matches(char::is_whitespace).len()];
        let tail = &text[text.trim_end_matches(char::is_whitespace).len()..];

        let mut styled = if flag(attributes, "code") {
            code(core)
        } else {
            escape(core)
        };
        for (attribute, marker) in [("italic", "*"), ("bold", "**"), ("strike", "~~")] {
            if flag(attributes, attribute) {
                styled = format!("{marker}{styled}{marker}");
            }
        }
        if let Some(url) = attributes.get("link").and_then(Value::as_str) {
            styled = format!("[{styled}]({url})");
        }
        format!("{lead}{styled}{tail}")
    }

    pub fn escape(text: &str) -> String {
        let mut escaped = String::with_capacity(text.len());
        for character in text.chars() {
            if matches!(character, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '~') {
                escaped.push('\\');
            }
            escaped.push(character);
        }
        escaped
    }

    /// A code span, fenced by more backticks than the text itself runs to.
    fn code(text: &str) -> String {
        let longest = text
            .split(|character| character != '`')
            .map(str::len)
            .max()
            .unwrap_or(0);
        let fence = "`".repeat(longest + 1);
        let pad = if text.starts_with('`') || text.ends_with('`') {
            " "
        } else {
            ""
        };
        format!("{fence}{pad}{text}{pad}{fence}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn markdown(delta: &str) -> String {
        to_markdown(delta).unwrap()
    }

    #[test]
    fn styles_inline_runs() {
        assert_eq!(
            markdown(
                r#"{"ops":[{"attributes":{"code":true},"insert":"hello"},{"insert":" "},
                   {"attributes":{"bold":true},"insert":"there"},{"insert":" "},
                   {"attributes":{"italic":true},"insert":"world"}]}"#
            ),
            "`hello` **there** *world*\n"
        );
    }

    #[test]
    fn keeps_emphasis_markers_off_whitespace() {
        assert_eq!(
            markdown(r#"{"ops":[{"attributes":{"bold":true},"insert":"a "},{"insert":"b"}]}"#),
            "**a** b\n"
        );
    }

    #[test]
    fn renders_links() {
        assert_eq!(
            markdown(
                r#"{"ops":[{"attributes":{"link":"https://e.com","bold":true},"insert":"e"}]}"#
            ),
            "[**e**](https://e.com)\n"
        );
    }

    #[test]
    fn fences_code_containing_backticks() {
        assert_eq!(
            markdown(r#"{"ops":[{"attributes":{"code":true},"insert":"a `b` c"}]}"#),
            "``a `b` c``\n"
        );
    }

    #[test]
    fn separates_paragraphs_but_not_line_breaks() {
        assert_eq!(
            markdown(r#"{"ops":[{"insert":"a\nb\n\nc\n"}]}"#),
            "a\nb\n\nc\n"
        );
    }

    #[test]
    fn numbers_ordered_lists() {
        assert_eq!(
            markdown(
                r#"{"ops":[{"insert":"one"},{"attributes":{"list":"ordered"},"insert":"\n"},
                   {"insert":"two"},{"attributes":{"list":"ordered"},"insert":"\n"},
                   {"insert":"deep"},
                   {"attributes":{"list":"ordered","indent":1},"insert":"\n"}]}"#
            ),
            "1. one\n2. two\n  1. deep\n"
        );
    }

    #[test]
    fn renders_bullets_tasks_and_quotes() {
        assert_eq!(
            markdown(
                r#"{"ops":[{"insert":"b"},{"attributes":{"list":"bullet"},"insert":"\n"},
                   {"insert":"t"},{"attributes":{"list":"checked"},"insert":"\n"},
                   {"insert":"q"},{"attributes":{"blockquote":true},"insert":"\n"}]}"#
            ),
            "- b\n- [x] t\n\n> q\n"
        );
    }

    #[test]
    fn fences_code_blocks_verbatim() {
        assert_eq!(
            markdown(
                r#"{"ops":[{"insert":"let x = *y;"},{"attributes":{"code-block":true},"insert":"\n"},
                   {"insert":""},{"attributes":{"code-block":true},"insert":"\n"},
                   {"insert":"after"},{"insert":"\n"}]}"#
            ),
            "```\nlet x = *y;\n\n```\n\nafter\n"
        );
    }

    #[test]
    fn renders_headings() {
        assert_eq!(
            markdown(r#"{"ops":[{"insert":"Title"},{"attributes":{"header":2},"insert":"\n"}]}"#),
            "## Title\n"
        );
    }

    #[test]
    fn escapes_markup_in_plain_text() {
        assert_eq!(
            markdown(r#"{"ops":[{"insert":"a *b* [c] _d_\n- not a list\n"}]}"#),
            "a \\*b\\* \\[c\\] \\_d\\_\n\\- not a list\n"
        );
    }

    #[test]
    fn takes_display_text_from_embeds() {
        assert_eq!(
            markdown(r#"{"ops":[{"insert":"hi "},{"insert":{"user":{"name":"@jeff"}}}]}"#),
            "hi @jeff\n"
        );
    }

    #[test]
    fn rejects_a_delta_without_ops() {
        assert!(to_markdown("{}").is_err());
    }
}
