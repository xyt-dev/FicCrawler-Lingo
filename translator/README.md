# translator

Batch-translates EnglishReading HTML chapters using Claude API.

## Setup

```bash
python3 -m venv .venv
.venv/bin/pip install anthropic beautifulsoup4 lxml
```

## Usage

```bash
export ANTHROPIC_API_KEY=sk-ant-...

# Translate all chapters of a book
.venv/bin/python translate.py "../books/A Ruinous Gift"

# Single chapter only
.venv/bin/python translate.py "../books/A Ruinous Gift" --chapter 1

# Dry-run (no API calls, just show batches)
.venv/bin/python translate.py "../books/A Ruinous Gift" --dry-run

# Custom batch size
.venv/bin/python translate.py "../books/A Ruinous Gift" --batch-size 10
```

## How it works

1. Reads `chapter{N}.md` to extract paragraphs (mirrors the Rust `md_block_to_html` logic — only plain text blocks get a `para-block` in the HTML)
2. Splits into batches of 15 paragraphs
3. Calls Claude with `prompt.txt` + the batch text
4. Parses `### p[N]` sections from the response
5. Fills `<p class="trans-text">`, `<p class="vocab-item">`, `<p class="chunks">` in `chapter{N}.html`
6. Saves progress to `chapter{N}.progress.json` — interrupted runs resume where they left off

## Resume / idempotency

Progress is saved after each batch. If the run is interrupted, just re-run the same command — already-translated paragraphs are skipped.

If a batch fails to parse, the raw API response is saved to `chapter{N}.debug.{start_para}.txt` for inspection.
