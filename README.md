# mdrich

Converts between Markdown and macOS pasteboard rich text, in both directions.

Reads Markdown on stdin, renders it, and puts it on the pasteboard as rich text:

```sh
echo '**bold**' | mdrich
```

Then paste into any rich-text destination: Slack, Gmail, Notion, etc..

With `--paste`, goes the other way: reads the pasteboard's rich text and writes
Markdown to stdout.

```sh
mdrich --paste
```

Copy from a web page or an email, then run it to get Markdown back. When the
pasteboard carries no HTML flavor, its plain text is emitted unchanged.
