# Translations

MiniToo Studio keeps every text in a catalog, one file per language: `en.lang` (English, also
the fallback for anything missing) and `ru.lang` (Russian) are built into the program.

## Making a translation

1. In **Settings → Language and formats** press **Translations folder**. It opens
   `~/.config/minitoo-studio/locales/` (on Windows `%APPDATA%\minitoo-studio\locales\`) and puts
   `en.lang.template` there — the current English catalog.
2. Copy it to `<code>.lang`, where `<code>` is the language code: `de.lang`, `uk.lang`,
   `pt.lang`…
3. Change `@name` to the name of the language as its speakers write it (`Deutsch`), and
   `@plural` if needed (see below).
4. Translate the text after `=` on each line. Keep `{placeholders}` as they are — the program puts
   values there (`{file}`, `{n}`…) — and keep option names, ids and product names.
5. Pick the language in Settings. To see changes, pick another language and then yours again.

Lines you leave out are shown in English, so a translation can be done bit by bit. A file with
the code of a built-in language (`ru.lang`) changes only the lines it contains.

## Format

```
# a comment
@name = Deutsch
@plural = en
nav.image = Bild
log.saved = {file} gespeichert
modes.sessions[one] = {n} Sitzung
modes.sessions[other] = {n} Sitzungen
settings.cli.prefix = "Befehlszeile: "
```

- `key = text`, one per line; spaces around `=` do not matter.
- Wrap a text in double quotes to keep spaces at its start or end. `\n` is a line break.
- **Plural forms** — a key with `[form]` is chosen by a number `{n}`. Forms: `zero`, `one`, `two`,
  `few`, `many`, `other`; which ones exist depends on `@plural`:

  | `@plural` | forms | languages |
  |---|---|---|
  | `en` | one (1), other | English, German, Spanish, Italian, Dutch, Swedish… (default) |
  | `fr` | one (0, 1), other | French, Portuguese |
  | `ru` | one (1, 21…), few (2–4, 22–24…), many | Russian, Ukrainian, Belarusian |
  | `pl` | one (1), few (2–4, 22–24…), many | Polish |
  | `cs` | one (1), few (2–4), other | Czech, Slovak |
  | `none` | other | Japanese, Chinese, Korean… |

  If a form is missing, `[other]` is used.

## Dates and times

`format.hours` (24 or 12) and `format.date` are the defaults of the language; in Settings they can
be changed. Date patterns: `%Y` 2026, `%y` 26, `%m` 10, `%d` 09, `%-m` / `%-d` without a leading
zero, `%B` month name, `%b` short month, `%A` weekday, `%a` short weekday, `%H` `%I` hours (24 /
12), `%M` minutes, `%S` seconds, `%p` AM/PM. Names come from `date.months` (the form a month takes
inside a date, e.g. Russian genitive «октября»), `date.months.short`, `date.weekdays`,
`date.weekdays.short` — comma-separated lists starting with January and Monday.

## Fonts

The interface and the speaker screen use DejaVu Sans, which covers Latin, Cyrillic, Greek and
more; scripts it lacks (Chinese, Japanese, Arabic…) are not shown yet.
