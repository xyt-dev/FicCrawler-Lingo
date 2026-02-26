#!/usr/bin/env python3
"""
Batch-translate EnglishReading HTML chapters using Claude API.

For each chapter:
  1. Extract paragraphs directly from the .html file (id="pN", p.original)
  2. Split into batches of BATCH_SIZE paragraphs
  3. Call Claude API with prompt.txt + batch text
  4. Parse ### p[N] blocks from response
  5. Fill translation/vocab/chunks into the .html file in-place
  6. Save progress to a .json sidecar so interrupted runs can resume

Usage:
  python translate.py <book_dir> [--chapter N] [--batch-size N] [--dry-run] [--repatch]

Example:
  python translate.py "../books/A Ruinous Gift"
  python translate.py "../books/A Ruinous Gift" --chapter 1
  python translate.py "../books/A Ruinous Gift" --chapter 1 --repatch
"""

import argparse
import json
import os
import re
import sys
import time
from pathlib import Path

import anthropic
from bs4 import BeautifulSoup
import markdown as md_lib

# ── config ────────────────────────────────────────────────────────────────────
MODEL = "claude-opus-4-6"
BATCH_SIZE = 15          # paragraphs per API call
MAX_RETRIES = 3
RETRY_DELAY = 5          # seconds between retries


# ── paragraph extraction ──────────────────────────────────────────────────────

def extract_paragraphs_from_html(html_path: Path) -> list[tuple[int, str]]:
    """
    Return [(para_id, text), ...] by reading para-blocks directly from the HTML.
    ID is taken from the element's id attribute (p1 -> 1).
    Only blocks with non-empty original text are included.
    """
    soup = BeautifulSoup(html_path.read_text(encoding="utf-8"), "lxml")
    result = []
    for block in soup.select("div.para-block[id]"):
        pid_str = block.get("id", "")
        if not pid_str.startswith("p"):
            continue
        try:
            pid = int(pid_str[1:])
        except ValueError:
            continue
        original = block.select_one("p.original")
        if original is None:
            continue
        text = original.get_text(separator="\n", strip=True)
        if text:
            result.append((pid, text))
    result.sort(key=lambda x: x[0])
    return result


# ── progress sidecar ──────────────────────────────────────────────────────────

def load_progress(progress_path: Path) -> dict:
    if progress_path.exists():
        return json.loads(progress_path.read_text(encoding="utf-8"))
    return {}   # {str(para_id): {"translation": ..., "vocab": ..., "chunks": ...}}


def save_progress(progress_path: Path, progress: dict):
    progress_path.write_text(
        json.dumps(progress, ensure_ascii=False, indent=2), encoding="utf-8"
    )


# ── Claude API call ───────────────────────────────────────────────────────────

def build_user_message(prompt_txt: str, batch: list[tuple[int, str]]) -> str:
    lines = [prompt_txt.strip(), "", "## Chapter Text", ""]
    for pid, text in batch:
        lines.append(f"### p{pid}")
        lines.append("")
        lines.append(text)
        lines.append("")
    return "\n".join(lines)


def call_claude(client: anthropic.Anthropic, prompt_txt: str,
                batch: list[tuple[int, str]]) -> str:
    user_msg = build_user_message(prompt_txt, batch)
    for attempt in range(1, MAX_RETRIES + 1):
        try:
            resp = client.messages.create(
                model=MODEL,
                max_tokens=8000,
                messages=[{"role": "user", "content": user_msg}],
            )
            return resp.content[0].text
        except anthropic.RateLimitError:
            if attempt == MAX_RETRIES:
                raise
            wait = RETRY_DELAY * attempt
            print(f"  rate limit, waiting {wait}s …", flush=True)
            time.sleep(wait)
        except anthropic.APIError as e:
            if attempt == MAX_RETRIES:
                raise
            print(f"  API error ({e}), retrying in {RETRY_DELAY}s …", flush=True)
            time.sleep(RETRY_DELAY)
    return ""


# ── response parsing ──────────────────────────────────────────────────────────

def parse_response(response: str) -> dict[int, dict]:
    """
    Parse Claude's output into {para_id: {translation, vocab, chunks}}.
    Expects sections like:
        ### p3
        **Translation:**
        ...
        **Vocabulary ...**
        ...
        **Chunks ...**
        ...
    """
    result = {}
    parts = re.split(r"###\s+p(\d+)", response)
    # parts = [preamble, id, body, id, body, ...]
    it = iter(parts)
    next(it)  # skip preamble
    for pid_str, body in zip(it, it):
        pid = int(pid_str)
        translation = _extract_section(body, r"\*\*Translation[^*]*\*\*", r"\*\*Vocabulary")
        vocab = _extract_section(body, r"\*\*Vocabulary[^*]*\*\*", r"\*\*Chunks")
        chunks = _extract_section(body, r"\*\*Chunks[^*]*\*\*", None)
        result[pid] = {
            "translation": translation.strip(),
            "vocab": vocab.strip(),
            "chunks": chunks.strip(),
        }
    return result


def _extract_section(text: str, start_pat: str, end_pat: str | None) -> str:
    m = re.search(start_pat, text)
    if not m:
        return ""
    start = m.end()
    if end_pat:
        m2 = re.search(end_pat, text[start:])
        if m2:
            return text[start: start + m2.start()]
    return text[start:]


# ── HTML patching ─────────────────────────────────────────────────────────────

def _md_to_soup_contents(text: str):
    """Convert markdown text to a list of BeautifulSoup nodes."""
    html = md_lib.markdown(text, extensions=["nl2br"])
    inner = BeautifulSoup(html, "lxml").body
    return list(inner.children) if inner else []


def patch_html(html_path: Path, translations: dict[int, dict]):
    """Fill translation/vocab/chunks into the HTML para-blocks in-place."""
    soup = BeautifulSoup(html_path.read_text(encoding="utf-8"), "lxml")
    changed = 0
    for pid, data in translations.items():
        block = soup.find(id=f"p{pid}")
        if not block:
            continue
        trans_p = block.select_one("p.trans-text")
        vocab_p = block.select_one("p.vocab-item")
        chunks_p = block.select_one("p.chunks")
        if trans_p is not None:
            trans_p.clear()
            for node in _md_to_soup_contents(data["translation"]):
                trans_p.append(node)
        if vocab_p is not None:
            vocab_p.clear()
            for node in _md_to_soup_contents(data["vocab"]):
                vocab_p.append(node)
        if chunks_p is not None:
            chunks_p.clear()
            for node in _md_to_soup_contents(data["chunks"]):
                chunks_p.append(node)
        changed += 1
    html_path.write_text(str(soup), encoding="utf-8")
    return changed


# ── chapter pipeline ──────────────────────────────────────────────────────────

def translate_chapter(client: anthropic.Anthropic, prompt_txt: str,
                      chapter_num: int, book_dir: Path,
                      batch_size: int, dry_run: bool, repatch: bool = False):
    html_path = book_dir / f"chapter{chapter_num}.html"
    prog_path = book_dir / f"chapter{chapter_num}.progress.json"

    if not html_path.exists():
        print(f"  [skip] {html_path.name} not found")
        return

    paragraphs = extract_paragraphs_from_html(html_path)
    if not paragraphs:
        print(f"  [skip] chapter{chapter_num}: no paragraphs")
        return

    progress = load_progress(prog_path)
    todo = [(pid, text) for pid, text in paragraphs
            if str(pid) not in progress]

    print(f"  chapter{chapter_num}: {len(paragraphs)} paras total, "
          f"{len(todo)} remaining", flush=True)

    if repatch or not todo:
        print("  patching html from progress …", flush=True)
        patch_html(html_path, {int(k): v for k, v in progress.items()})
        return

    # Process in batches
    for batch_start in range(0, len(todo), batch_size):
        batch = todo[batch_start: batch_start + batch_size]
        ids_str = f"p{batch[0][0]}–p{batch[-1][0]}"
        print(f"    batch {ids_str} ({len(batch)} paras) …", end=" ", flush=True)

        if dry_run:
            print("[dry-run, skip]")
            continue

        response = call_claude(client, prompt_txt, batch)
        parsed = parse_response(response)

        if not parsed:
            print(f"WARNING: no paragraphs parsed from response for {ids_str}")
            debug_path = book_dir / f"chapter{chapter_num}.debug.{batch[0][0]}.txt"
            debug_path.write_text(response, encoding="utf-8")
            print(f"    raw response saved to {debug_path.name}")
            continue

        for pid, data in parsed.items():
            progress[str(pid)] = data
        save_progress(prog_path, progress)
        print(f"parsed {len(parsed)} ✓", flush=True)

        time.sleep(1)

    # Patch HTML with all collected translations
    all_translations = {int(k): v for k, v in progress.items()}
    n = patch_html(html_path, all_translations)
    print(f"  patched {n} paragraphs into {html_path.name}", flush=True)


# ── main ──────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(description="Translate EnglishReading HTML chapters")
    parser.add_argument("book_dir", help="Path to the book directory")
    parser.add_argument("--chapter", type=int, default=None,
                        help="Only translate this chapter number")
    parser.add_argument("--batch-size", type=int, default=BATCH_SIZE,
                        help=f"Paragraphs per API call (default {BATCH_SIZE})")
    parser.add_argument("--dry-run", action="store_true",
                        help="Parse paragraphs but don't call the API")
    parser.add_argument("--repatch", action="store_true",
                        help="Skip API calls; re-patch HTML from existing progress.json")
    args = parser.parse_args()

    book_dir = Path(args.book_dir).expanduser().resolve()
    if not book_dir.is_dir():
        sys.exit(f"Error: {book_dir} is not a directory")

    prompt_path = book_dir / "prompt.txt"
    if not prompt_path.exists():
        sys.exit(f"Error: prompt.txt not found in {book_dir}")
    prompt_txt = prompt_path.read_text(encoding="utf-8")

    api_key = os.environ.get("ANTHROPIC_API_KEY")
    if not api_key and not args.repatch:
        sys.exit("Error: ANTHROPIC_API_KEY environment variable not set")
    client = anthropic.Anthropic(api_key=api_key) if api_key else None

    # Discover chapters from HTML files
    if args.chapter:
        chapters = [args.chapter]
    else:
        chapter_files = sorted(
            book_dir.glob("chapter*.html"),
            key=lambda p: int(re.search(r"\d+", p.stem).group())
        )
        chapters = [int(re.search(r"\d+", p.stem).group()) for p in chapter_files]

    print(f"Book: {book_dir.name}")
    print(f"Chapters: {chapters}")
    print(f"Batch size: {args.batch_size}")
    print(f"Model: {MODEL}")
    print()

    for ch in chapters:
        print(f"── Chapter {ch} ──────────────────────────────")
        translate_chapter(client, prompt_txt, ch, book_dir,
                          args.batch_size, args.dry_run, args.repatch)
        print()

    print("Done.")


if __name__ == "__main__":
    main()
