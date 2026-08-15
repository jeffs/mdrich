#![doc = include_str!("../README.md")]

use std::io::{self, Read, Write};
use std::process;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options, markdown_to_html, parse_document};
use htmd::HtmlToMarkdown;
use htmd::options::{BulletListMarker, Options as HtmlOptions};
use objc2_app_kit::NSPasteboard;
use objc2_foundation::NSString;

mod chromium;
mod quill;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The pasteboard flavors we read and write.
mod flavor {
    pub const HTML: &str = "public.html";
    pub const PLAIN: &str = "public.utf8-plain-text";
    pub const WEB_CUSTOM: &str = "org.chromium.web-custom-data";
}

/// The custom `DataTransfer` type under which Slack's editor offers the Quill
/// delta behind a copied message.
const SLACK_DELTA: &str = "slack/texty";

/// Which direction to convert.
enum Mode {
    /// Markdown on stdin becomes rich text on the pasteboard.
    Copy,
    /// Rich text on the pasteboard becomes Markdown on stdout.
    Paste,
}

impl Mode {
    fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self> {
        match args.next().as_deref() {
            None => Ok(Self::Copy),
            Some("--paste") => Ok(Self::Paste),
            Some(other) => Err(format!("unrecognized argument: {other}").into()),
        }
    }
}

fn slurp_stdin() -> Result<String> {
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;
    Ok(String::from_utf8(bytes)?)
}

fn append_inline_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    for child in node.children() {
        match &child.data.borrow().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::SoftBreak => out.push(' '),
            NodeValue::LineBreak => out.push('\n'),
            NodeValue::HtmlInline(_) => {}
            _ => append_inline_text(child, out),
        }
    }
}

fn append_block_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    let mut first = true;
    for child in node.children() {
        let separator = if first { "" } else { "\n\n" };
        match &child.data.borrow().value {
            NodeValue::Paragraph | NodeValue::Heading(_) => {
                out.push_str(separator);
                append_inline_text(child, out);
                first = false;
            }
            NodeValue::BlockQuote | NodeValue::List(_) | NodeValue::Item(_) => {
                out.push_str(separator);
                append_block_text(child, out);
                first = false;
            }
            NodeValue::CodeBlock(cb) => {
                out.push_str(separator);
                out.push_str(cb.literal.trim_end_matches('\n'));
                first = false;
            }
            NodeValue::ThematicBreak => {
                out.push_str(separator);
                first = false;
            }
            _ => {}
        }
    }
}

fn render_plain(markdown: &str) -> String {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &Options::default());
    let mut out = String::new();
    append_block_text(root, &mut out);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn render_html(markdown: &str) -> String {
    let mut options = Options::default();
    options.extension.autolink = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    markdown_to_html(markdown, &options)
}

fn read_clipboard(uti: &str) -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    let ns_type = NSString::from_str(uti);
    pb.stringForType(&ns_type).map(|s| s.to_string())
}

fn read_clipboard_data(uti: &str) -> Option<Vec<u8>> {
    let pb = NSPasteboard::generalPasteboard();
    let ns_type = NSString::from_str(uti);
    pb.dataForType(&ns_type).map(|data| data.to_vec())
}

fn write_clipboard(html: &str, plain: &str) {
    let html_ns = NSString::from_str(html);
    let plain_ns = NSString::from_str(plain);
    let html_type = NSString::from_str(flavor::HTML);
    let plain_type = NSString::from_str(flavor::PLAIN);

    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    pb.setString_forType(&html_ns, &html_type);
    pb.setString_forType(&plain_ns, &plain_type);
}

fn copy() -> Result<()> {
    let markdown = slurp_stdin()?;
    let html = render_html(&markdown);
    let plain = render_plain(&markdown);
    write_clipboard(&html, &plain);
    Ok(())
}

fn html_to_markdown(html: &str) -> Result<String> {
    Ok(HtmlToMarkdown::builder()
        .options(HtmlOptions {
            bullet_list_marker: BulletListMarker::Dash,
            ..HtmlOptions::default()
        })
        .build()
        .convert(html)?)
}

fn slack_delta() -> Option<String> {
    let custom = chromium::parse(&read_clipboard_data(flavor::WEB_CUSTOM)?)?;
    custom.get(SLACK_DELTA).cloned()
}

/// Reads the richest flavor the pasteboard offers as Markdown: HTML first,
/// then the Quill delta Slack hides in Chromium's custom-data flavor, and
/// finally the plain text, emitted unchanged.
fn pasteboard_markdown() -> Result<String> {
    if let Some(html) = read_clipboard(flavor::HTML) {
        return html_to_markdown(&html);
    }
    if let Some(delta) = slack_delta() {
        return quill::to_markdown(&delta);
    }
    Ok(read_clipboard(flavor::PLAIN).ok_or("pasteboard holds no text")?)
}

fn paste() -> Result<()> {
    let markdown = pasteboard_markdown()?;
    let mut stdout = io::stdout();
    stdout.write_all(markdown.as_bytes())?;
    if !markdown.ends_with('\n') {
        stdout.write_all(b"\n")?;
    }
    Ok(())
}

fn run() -> Result<()> {
    match Mode::from_args(std::env::args().skip(1))? {
        Mode::Copy => copy(),
        Mode::Paste => paste(),
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("mdrich: {err}");
        process::exit(1);
    }
}
