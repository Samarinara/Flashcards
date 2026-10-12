# Flashcards deck format, version 1

A deck is a UTF-8 JSON document. Writers should use two-space indentation and a
trailing newline. Unknown fields are rejected so misspellings do not silently
lose information.

Every deck and card has a UUID. A deck ID identifies the logical deck even if
the file is renamed. Card IDs allow review events from an active session to be
merged after an external edit.

## Minimal deck

```json
{
  "schema_version": 1,
  "id": "9fb0c901-6118-4a68-a5c5-68381d2ca6f8",
  "title": "Capitals",
  "cards": [
    {
      "id": "91dbdba2-33ad-424a-8c4d-314fe77369fe",
      "front": "Portugal",
      "back": "Lisbon",
      "stats": {
        "again_count": 0,
        "good_count": 0,
        "current_good_streak": 0
      }
    }
  ]
}
```

`description`, `tags`, rating `history`, and `sessions` are omitted when empty.
Tags are lowercase, sorted, unique strings. Fronts and backs are Unicode text;
line breaks are preserved. They may include LaTeX equations for the study view.

## Equations

Use `$...$` or `\(...\)` for inline equations, and `$$...$$` or `\[...\]`
for equations on their own line. Both the front and back support mixed text
and math, including fractions, roots, superscripts, subscripts, Greek letters,
sums, and integrals. For example, type this directly in the card editor:

```text
Solve $ax^2 + bx + c = 0$.

$$x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}$$
```

JSON requires backslashes to be escaped, so the same answer in a deck file is:

```json
"back": "$$x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$$"
```

Rendering works offline with a bundled math font. The editor and JSON retain
the original source. Invalid or unsupported LaTeX and unclosed delimiters
display as text rather than preventing a study session. Hover over study
content to see its source. Wide equations can be scrolled horizontally.

Single-dollar math must not start or end with whitespace or span multiple
lines; ordinary prices such as `Costs $5 and $10` remain text. Escape a dollar
sign with `\$` to prevent it from being interpreted as a delimiter.

## Review data

Each Again or Good response appends a rating event to its card. The event
contains a UTC timestamp, the local calendar date, and UTC offset at review
time. This preserves day-streak meaning if the deck later moves between time
zones.

The aggregate counts and `last_reviewed` must agree with `history`. A mismatch
is a validation error instead of being silently repaired. A Good response
increments `current_good_streak`; Again resets it to zero.

Only completed sessions are added to `sessions`. Ratings from interrupted
sessions remain valid history and retain their session UUID even without a
matching summary.

## Compatibility

`schema_version` is required. Flashcards 0.1 supports version 1 and refuses
unknown versions. A future migration must create a backup before changing the
source document.
