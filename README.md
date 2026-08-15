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

Copy from a web page or an email, then run it to get Markdown back.

Slack is a special case. Copying a message there leaves no HTML on the
pasteboard: Slack's editor state travels instead as a Quill delta inside
Chromium's `org.chromium.web-custom-data` flavor, which only another Chromium
app can read. `--paste` unpacks that flavor and renders the delta, so
formatting survives the trip. Failing both, the pasteboard's plain text is
emitted unchanged.
