# translator

Batch-translates EnglishReading HTML chapters using the Claude API.

## Setup

```bash
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
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

# Custom batch size (default: 15)
.venv/bin/python translate.py "../books/A Ruinous Gift" --batch-size 10
```

## How it works

1. Reads `chapter{N}.md` to extract paragraphs — mirrors the Rust `md_block_to_html` logic, so only plain-text blocks that have a `para-block` in the HTML are included
2. Splits into batches of 15 paragraphs per API call
3. Sends `prompt.txt` + batch text to Claude
4. Parses `### p[N]` sections from the response
5. Saves parsed results to `chapter{N}.progress.json` immediately after each batch
6. After all batches complete, patches `<p class="trans-text">`, `<p class="vocab-item">`, `<p class="chunks">` in `chapter{N}.html`

## Resume on interruption

Progress is saved to `chapter{N}.progress.json` after every batch.
If the run is interrupted (Ctrl+C, crash, etc.), re-run the same command —
already-translated paragraphs are skipped and the run continues from where it left off.

If a batch fails to parse, the raw API response is saved to
`chapter{N}.debug.{start_para}.txt` for inspection.
