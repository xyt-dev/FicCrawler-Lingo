use clap::Parser;
use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "ao3-scraper", about = "Scrape AO3 works into markdown files")]
struct Args {
    /// AO3 work URL (e.g. https://archiveofourown.org/works/12345)
    url: String,

    /// Output directory (default: ./books)
    #[arg(short, long, default_value = "books")]
    output: String,

    /// Delay between requests in milliseconds
    #[arg(short, long, default_value_t = 1500)]
    delay: u64,

    /// Max retry attempts per request
    #[arg(short, long, default_value_t = 5)]
    retries: u32,

    /// Request timeout in seconds
    #[arg(short, long, default_value_t = 60)]
    timeout: u64,

    /// Path to a Netscape-format cookies.txt file (export from browser extension)
    #[arg(long)]
    cookies: Option<String>,
}

#[derive(Debug)]
struct WorkMeta {
    title: String,
    authors: Vec<String>,
    rating: String,
    warnings: Vec<String>,
    categories: Vec<String>,
    fandoms: Vec<String>,
    relationships: Vec<String>,
    characters: Vec<String>,
    additional_tags: Vec<String>,
    language: String,
    series: Vec<String>,
    stats: HashMap<String, String>,
    summary: String,
    notes_begin: String,
    notes_end: String,
    published: String,
    updated: String,
    status: String,
}

fn main() {
    let args = Args::parse();

    let work_id = extract_work_id(&args.url).expect("Could not extract work ID from URL");

    // -- Phase 1: Fetch navigate page to get chapter list & metadata --
    let phase1_pb = ProgressBar::new_spinner();
    phase1_pb.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner:.cyan.bold} {msg}")
            .unwrap(),
    );
    phase1_pb.set_message(format!(
        "Fetching work {} navigate page...",
        style(&work_id).bold()
    ));
    phase1_pb.enable_steady_tick(Duration::from_millis(80));

    let navigate_url = format!(
        "https://archiveofourown.org/works/{}/navigate?view_adult=true",
        work_id
    );
    let navigate_html = fetch_with_retry(&navigate_url, args.retries, args.timeout, args.cookies.as_deref(), &phase1_pb);
    let navigate_doc = Html::parse_document(&navigate_html);
    let chapter_links = extract_chapter_links(&navigate_doc);

    let first_url = format!(
        "https://archiveofourown.org/works/{}?view_adult=true",
        work_id
    );
    thread::sleep(Duration::from_millis(args.delay));
    phase1_pb.set_message("Fetching metadata...");
    let first_html = fetch_with_retry(&first_url, args.retries, args.timeout, args.cookies.as_deref(), &phase1_pb);
    let first_doc = Html::parse_document(&first_html);

    let meta = extract_metadata(&first_doc);
    phase1_pb.finish_with_message(format!(
        "{} {} by {}",
        style("Found:").green().bold(),
        style(&meta.title).bold(),
        style(if meta.authors.is_empty() {
            "Unknown".to_string()
        } else {
            meta.authors.join(", ")
        })
        .dim()
    ));

    // Create output directory
    let dir_name = sanitize_filename::sanitize(&meta.title);
    let out_dir = PathBuf::from(&args.output).join(&dir_name);
    fs::create_dir_all(&out_dir).expect("Failed to create output directory");

    // Write metadata
    let meta_md = format_metadata_md(&meta);
    fs::write(out_dir.join("metadata.md"), &meta_md).expect("Failed to write metadata.md");
    fs::write(
        out_dir.join("metadata.html"),
        &generate_metadata_html(&meta),
    )
    .expect("Failed to write metadata.html");
    eprintln!("  {} metadata.md + metadata.html", style("wrote").green());

    // Write prompt file
    let prompt = generate_prompt(&meta.title);
    fs::write(out_dir.join("prompt.txt"), &prompt).expect("Failed to write prompt.txt");
    eprintln!("  {} prompt.txt", style("wrote").green());

    // -- Phase 2: Fetch chapters with progress bar --
    if chapter_links.is_empty() {
        let pb = ProgressBar::new(1);
        pb.set_style(chapter_progress_style());
        pb.set_message("chapter1");

        let content_md = extract_single_chapter_content(&first_doc);
        fs::write(out_dir.join("chapter1.md"), &content_md).expect("Failed to write chapter");

        let study_html = generate_chapter_html("Chapter 1", &content_md, "", "", 1);
        fs::write(out_dir.join("chapter1.html"), &study_html).expect("Failed to write html");

        pb.inc(1);
        pb.finish_with_message(format!("{} 1 chapter", style("Done!").green().bold()));
    } else {
        let total = chapter_links.len() as u64;
        let pb = ProgressBar::new(total);
        pb.set_style(chapter_progress_style());

        for (i, chapter_url) in chapter_links.iter().enumerate() {
            let chapter_num = i + 1;
            let filename = format!("chapter{}", chapter_num);
            pb.set_message(format!("{}  (fetching)", filename));

            if i > 0 {
                thread::sleep(Duration::from_millis(args.delay));
            }

            let full_chapter_url = format!(
                "https://archiveofourown.org{}?view_adult=true",
                chapter_url
            );
            let chapter_html =
                fetch_with_retry(&full_chapter_url, args.retries, args.timeout, args.cookies.as_deref(), &pb);

            pb.set_message(format!("{}  (parsing)", filename));
            let chapter_doc = Html::parse_document(&chapter_html);

            let title = chapter_doc
                .select(&sel("h3.title"))
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            // -- Markdown --
            let content = chapter_doc
                .select(&sel("div.userstuff[role='article']"))
                .next()
                .or_else(|| {
                    chapter_doc
                        .select(&sel("div#chapters div.userstuff"))
                        .next()
                })
                .map(|e| html_node_to_md(e))
                .unwrap_or_default();

            let chapter_notes_begin = chapter_doc
                .select(&sel("div#chapters div.preface div.notes blockquote.userstuff"))
                .next()
                .map(|e| html_node_to_md(e))
                .unwrap_or_default();
            let chapter_notes_end = chapter_doc
                .select(&sel("div#chapters div.end.notes blockquote"))
                .next()
                .map(|e| html_node_to_md(e))
                .unwrap_or_default();

            let mut md = String::new();
            if !title.is_empty() {
                md.push_str(&format!("# {}\n\n", title));
            }
            if !chapter_notes_begin.is_empty() {
                md.push_str(&format!(
                    "> **Chapter Notes:**\n>\n{}\n\n---\n\n",
                    prefix_lines(&chapter_notes_begin, "> ")
                ));
            }
            md.push_str(&content);
            if !chapter_notes_end.is_empty() {
                md.push_str(&format!(
                    "\n\n---\n\n> **End Notes:**\n>\n{}",
                    prefix_lines(&chapter_notes_end, "> ")
                ));
            }

            fs::write(out_dir.join(format!("{}.md", filename)), &md)
                .expect("Failed to write chapter md");

            // -- HTML with translation slots --
            let display_title = if title.is_empty() {
                format!("Chapter {}", chapter_num)
            } else {
                title.clone()
            };
            let study_html = generate_chapter_html(
                &display_title,
                &content,
                &chapter_notes_begin,
                &chapter_notes_end,
                chapter_num,
            );
            fs::write(out_dir.join(format!("{}.html", filename)), &study_html)
                .expect("Failed to write chapter html");

            pb.inc(1);
        }

        pb.finish_with_message(format!(
            "{} {} chapters (md + html)",
            style("Done!").green().bold(),
            total
        ));
    }

    eprintln!(
        "\n{}  {}",
        style("Saved to:").green().bold(),
        style(out_dir.display()).underlined()
    );
}

fn chapter_progress_style() -> ProgressStyle {
    ProgressStyle::default_bar()
        .template(
            " {spinner:.cyan} {msg:30!} [{bar:38.green/dim}] {pos}/{len}  {elapsed_precise} / ETA {eta}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ ")
}

fn fetch_with_retry(
    url: &str,
    max_retries: u32,
    timeout_secs: u64,
    cookies_file: Option<&str>,
    pb: &ProgressBar,
) -> String {
    let mut attempt = 0;
    loop {
        attempt += 1;

        let mut cmd = Command::new("curl");
        let timeout_str = timeout_secs.to_string();
        cmd.args([
            "--silent",
            "--compressed",
            "--location",
            "--max-time", &timeout_str,
            "--connect-timeout", "30",
            "-H", "User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:147.0) Gecko/20100101 Firefox/147.0",
            "-H", "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            "-H", "Accept-Language: zh-CN,zh;q=0.9,en-US;q=0.6,en;q=0.5",
            "-H", "Accept-Encoding: gzip, deflate, br, zstd",
            "-H", "Connection: keep-alive",
            "-H", "Upgrade-Insecure-Requests: 1",
            "-H", "Sec-Fetch-Dest: document",
            "-H", "Sec-Fetch-Mode: navigate",
            "-H", "Sec-Fetch-Site: none",
            "-H", "Sec-Fetch-User: ?1",
            "--write-out", "\n__STATUS__%{http_code}",
            url,
        ]);
        if let Some(path) = cookies_file {
            cmd.args(["--cookie", path]);
        }

        match cmd.output() {
            Ok(out) => {
                let raw = String::from_utf8_lossy(&out.stdout);
                if let Some(pos) = raw.rfind("\n__STATUS__") {
                    let body = &raw[..pos];
                    let status: u16 = raw[pos + 11..].trim().parse().unwrap_or(0);

                    if (200..300).contains(&status) {
                        return body.to_string();
                    }

                    let is_rate_limit = status == 429;
                    if (status >= 500 || is_rate_limit) && attempt <= max_retries {
                        let wait = retry_delay(attempt, is_rate_limit);
                        sleep_with_countdown(
                            pb,
                            wait,
                            &format!("HTTP {} · retry {}/{}", status, attempt, max_retries),
                        );
                        continue;
                    }
                    eprintln!("HTTP error {}: {}", status, url);
                    std::process::exit(1);
                } else {
                    // curl exit non-zero or no status marker → network error
                    if attempt <= max_retries {
                        let wait = retry_delay(attempt, false);
                        sleep_with_countdown(
                            pb,
                            wait,
                            &format!("curl error · retry {}/{}", attempt, max_retries),
                        );
                        continue;
                    }
                    eprintln!("curl failed after {} attempts: {}", max_retries, url);
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("Failed to run curl: {}", e);
                std::process::exit(1);
            }
        }
    }
}

/// Sleep for `duration`, updating the progress bar every second with a countdown.
fn sleep_with_countdown(pb: &ProgressBar, duration: Duration, reason: &str) {
    let total_secs = duration.as_secs().max(1);
    for remaining in (1..=total_secs).rev() {
        pb.set_message(format!("{} ({}s)", reason, remaining));
        thread::sleep(Duration::from_secs(1));
    }
}

fn retry_delay(attempt: u32, is_rate_limit: bool) -> Duration {
    let base = if is_rate_limit { 10 } else { 3 };
    let secs = base * 2u64.pow(attempt.saturating_sub(1));
    Duration::from_secs(secs.min(120))
}

/// Extract chapter URLs from the /navigate page.
fn extract_chapter_links(doc: &Html) -> Vec<String> {
    doc.select(&sel("ol.chapter.index.group li a"))
        .filter_map(|e| e.value().attr("href").map(|s| s.to_string()))
        .collect()
}

fn extract_work_id(url: &str) -> Option<String> {
    let re = Regex::new(r"archiveofourown\.org/works/(\d+)").unwrap();
    re.captures(url).map(|c| c[1].to_string())
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).unwrap()
}

fn extract_metadata(doc: &Html) -> WorkMeta {
    let title = doc
        .select(&sel("h2.title"))
        .next()
        .map(|e| e.text().collect::<String>().trim().to_string())
        .unwrap_or_default();

    let mut authors: Vec<String> = doc
        .select(&sel("h3.byline a[rel='author']"))
        .map(|e| e.text().collect::<String>().trim().to_string())
        .collect();
    if authors.is_empty() {
        authors = doc
            .select(&sel("h3.byline a"))
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if authors.is_empty() {
        if let Some(byline) = doc.select(&sel("h3.byline")).next() {
            let text = byline.text().collect::<String>().trim().to_string();
            if !text.is_empty() {
                authors.push(text);
            }
        }
    }

    let rating = extract_tag_list(doc, "dd.rating");
    let warnings = extract_tag_list_vec(doc, "dd.warning");
    let categories = extract_tag_list_vec(doc, "dd.category");
    let fandoms = extract_tag_list_vec(doc, "dd.fandom");
    let relationships = extract_tag_list_vec(doc, "dd.relationship");
    let characters = extract_tag_list_vec(doc, "dd.character");
    let additional_tags = extract_tag_list_vec(doc, "dd.freeform");
    let language = extract_tag_list(doc, "dd.language");
    let series = extract_tag_list_vec(doc, "dd.series");

    let mut stats = HashMap::new();
    let stats_sel = sel("dl.stats");
    if let Some(stats_dl) = doc.select(&stats_sel).next() {
        let dt_sel = sel("dt");
        let dd_sel = sel("dd");
        let dts: Vec<String> = stats_dl
            .select(&dt_sel)
            .map(|e| {
                e.text()
                    .collect::<String>()
                    .trim()
                    .trim_end_matches(':')
                    .to_string()
            })
            .collect();
        let dds: Vec<String> = stats_dl
            .select(&dd_sel)
            .map(|e| e.text().collect::<String>().trim().to_string())
            .collect();
        for (k, v) in dts.into_iter().zip(dds.into_iter()) {
            stats.insert(k, v);
        }
    }

    let summary = doc
        .select(&sel("div.preface .summary blockquote"))
        .next()
        .map(|e| html_node_to_md(e))
        .unwrap_or_default();

    let notes_begin = doc
        .select(&sel("div.preface .notes blockquote"))
        .next()
        .map(|e| html_node_to_md(e))
        .unwrap_or_default();

    let notes_end = doc
        .select(&sel("div.afterword .end.notes blockquote"))
        .next()
        .map(|e| html_node_to_md(e))
        .unwrap_or_default();

    let published = stats.get("Published").cloned().unwrap_or_default();
    let updated = stats.get("Updated").cloned().unwrap_or_default();
    let status = stats
        .get("Completed")
        .cloned()
        .unwrap_or_else(|| stats.get("Status").cloned().unwrap_or_default());

    WorkMeta {
        title,
        authors,
        rating,
        warnings,
        categories,
        fandoms,
        relationships,
        characters,
        additional_tags,
        language,
        series,
        stats,
        summary,
        notes_begin,
        notes_end,
        published,
        updated,
        status,
    }
}

fn extract_tag_list(doc: &Html, selector: &str) -> String {
    doc.select(&sel(selector))
        .next()
        .map(|e| {
            let tags: Vec<String> = e
                .select(&sel("a.tag"))
                .map(|a| a.text().collect::<String>().trim().to_string())
                .collect();
            if tags.is_empty() {
                e.text().collect::<String>().trim().to_string()
            } else {
                tags.join(", ")
            }
        })
        .unwrap_or_default()
}

fn extract_tag_list_vec(doc: &Html, selector: &str) -> Vec<String> {
    doc.select(&sel(selector))
        .next()
        .map(|e| {
            let tags: Vec<String> = e
                .select(&sel("a.tag"))
                .map(|a| a.text().collect::<String>().trim().to_string())
                .collect();
            if tags.is_empty() {
                let text = e.text().collect::<String>().trim().to_string();
                if text.is_empty() {
                    vec![]
                } else {
                    vec![text]
                }
            } else {
                tags
            }
        })
        .unwrap_or_default()
}

fn format_metadata_md(meta: &WorkMeta) -> String {
    let mut md = String::new();

    md.push_str(&format!("# {}\n\n", meta.title));
    md.push_str(&format!("**Author(s):** {}\n\n", meta.authors.join(", ")));

    md.push_str("---\n\n");
    md.push_str("## Work Information\n\n");

    md.push_str("| Field | Value |\n");
    md.push_str("| --- | --- |\n");
    md.push_str(&format!("| **Rating** | {} |\n", meta.rating));
    md.push_str(&format!(
        "| **Archive Warnings** | {} |\n",
        meta.warnings.join(", ")
    ));
    if !meta.categories.is_empty() {
        md.push_str(&format!(
            "| **Categories** | {} |\n",
            meta.categories.join(", ")
        ));
    }
    md.push_str(&format!(
        "| **Fandoms** | {} |\n",
        meta.fandoms.join(", ")
    ));
    if !meta.relationships.is_empty() {
        md.push_str(&format!(
            "| **Relationships** | {} |\n",
            meta.relationships.join(", ")
        ));
    }
    if !meta.characters.is_empty() {
        md.push_str(&format!(
            "| **Characters** | {} |\n",
            meta.characters.join(", ")
        ));
    }
    md.push_str(&format!("| **Language** | {} |\n", meta.language));

    if !meta.published.is_empty() {
        md.push_str(&format!("| **Published** | {} |\n", meta.published));
    }
    if !meta.updated.is_empty() {
        md.push_str(&format!("| **Updated** | {} |\n", meta.updated));
    }
    if !meta.status.is_empty() {
        md.push_str(&format!("| **Status** | {} |\n", meta.status));
    }

    md.push('\n');

    md.push_str("## Stats\n\n");
    md.push_str("| Stat | Value |\n");
    md.push_str("| --- | --- |\n");
    let stat_keys = [
        "Words", "Chapters", "Comments", "Kudos", "Bookmarks", "Hits",
    ];
    for key in &stat_keys {
        if let Some(val) = meta.stats.get(*key) {
            md.push_str(&format!("| **{}** | {} |\n", key, val));
        }
    }
    for (k, v) in &meta.stats {
        if !stat_keys.contains(&k.as_str())
            && k != "Published"
            && k != "Updated"
            && k != "Completed"
            && k != "Status"
        {
            md.push_str(&format!("| **{}** | {} |\n", k, v));
        }
    }
    md.push('\n');

    if !meta.additional_tags.is_empty() {
        md.push_str("## Tags\n\n");
        for tag in &meta.additional_tags {
            md.push_str(&format!("- {}\n", tag));
        }
        md.push('\n');
    }

    if !meta.series.is_empty() {
        md.push_str("## Series\n\n");
        for s in &meta.series {
            md.push_str(&format!("- {}\n", s));
        }
        md.push('\n');
    }

    if !meta.summary.is_empty() {
        md.push_str("## Summary\n\n");
        md.push_str(&meta.summary);
        md.push_str("\n\n");
    }

    if !meta.notes_begin.is_empty() {
        md.push_str("## Notes (Beginning)\n\n");
        md.push_str(&meta.notes_begin);
        md.push_str("\n\n");
    }

    if !meta.notes_end.is_empty() {
        md.push_str("## Notes (End)\n\n");
        md.push_str(&meta.notes_end);
        md.push_str("\n\n");
    }

    md
}

fn extract_single_chapter_content(doc: &Html) -> String {
    doc.select(&sel("div.userstuff[role='article']"))
        .next()
        .or_else(|| doc.select(&sel("div.userstuff")).next())
        .map(|e| html_node_to_md(e))
        .unwrap_or_else(|| "No content found.".to_string())
}

/// Convert an HTML element tree into Markdown, preserving formatting.
fn html_node_to_md(el: ElementRef) -> String {
    let mut output = String::new();
    process_children(el, &mut output, &InlineCtx::default());
    let re = Regex::new(r"\n{3,}").unwrap();
    let output = re.replace_all(&output, "\n\n").to_string();
    output.trim().to_string()
}

#[derive(Default, Clone)]
struct InlineCtx {
    bold: bool,
    italic: bool,
    in_blockquote: bool,
    list_depth: usize,
    ordered: bool,
    list_counter: usize,
}

fn process_children(el: ElementRef, out: &mut String, ctx: &InlineCtx) {
    for child in el.children() {
        match child.value() {
            scraper::node::Node::Text(text) => {
                let t = text.text.to_string();
                if !t.trim().is_empty() || t.contains(' ') {
                    out.push_str(&t);
                }
            }
            scraper::node::Node::Element(elem) => {
                let tag = elem.name();
                if let Some(child_ref) = ElementRef::wrap(child) {
                    match tag {
                        "p" => {
                            out.push_str("\n\n");
                            if ctx.in_blockquote {
                                out.push_str("> ");
                            }
                            process_children(child_ref, out, ctx);
                            out.push('\n');
                        }
                        "br" => {
                            out.push('\n');
                            if ctx.in_blockquote {
                                out.push_str("> ");
                            }
                        }
                        "strong" | "b" => {
                            out.push_str("**");
                            let mut new_ctx = ctx.clone();
                            new_ctx.bold = true;
                            process_children(child_ref, out, &new_ctx);
                            out.push_str("**");
                        }
                        "em" | "i" => {
                            out.push('*');
                            let mut new_ctx = ctx.clone();
                            new_ctx.italic = true;
                            process_children(child_ref, out, &new_ctx);
                            out.push('*');
                        }
                        "s" | "strike" | "del" => {
                            out.push_str("~~");
                            process_children(child_ref, out, ctx);
                            out.push_str("~~");
                        }
                        "u" => {
                            out.push_str("<u>");
                            process_children(child_ref, out, ctx);
                            out.push_str("</u>");
                        }
                        "sup" => {
                            out.push_str("<sup>");
                            process_children(child_ref, out, ctx);
                            out.push_str("</sup>");
                        }
                        "sub" => {
                            out.push_str("<sub>");
                            process_children(child_ref, out, ctx);
                            out.push_str("</sub>");
                        }
                        "a" => {
                            let href = elem.attr("href").unwrap_or("#");
                            let href = if href.starts_with('/') {
                                format!("https://archiveofourown.org{}", href)
                            } else {
                                href.to_string()
                            };
                            out.push('[');
                            process_children(child_ref, out, ctx);
                            out.push_str(&format!("]({})", href));
                        }
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            let class = elem.attr("class").unwrap_or("");
                            if class.contains("landmark") || class.contains("heading") {
                                // Skip AO3 landmark/accessibility headings
                            } else {
                                let level = &tag[1..2];
                                let prefix =
                                    "#".repeat(level.parse::<usize>().unwrap_or(1));
                                out.push_str(&format!("\n\n{} ", prefix));
                                process_children(child_ref, out, ctx);
                                out.push_str("\n\n");
                            }
                        }
                        "hr" => {
                            out.push_str("\n\n---\n\n");
                        }
                        "blockquote" => {
                            out.push_str("\n\n");
                            let mut new_ctx = ctx.clone();
                            new_ctx.in_blockquote = true;
                            out.push_str("> ");
                            process_children(child_ref, out, &new_ctx);
                            out.push_str("\n\n");
                        }
                        "ul" => {
                            out.push('\n');
                            let mut new_ctx = ctx.clone();
                            new_ctx.list_depth = ctx.list_depth + 1;
                            new_ctx.ordered = false;
                            new_ctx.list_counter = 0;
                            process_children(child_ref, out, &new_ctx);
                            out.push('\n');
                        }
                        "ol" => {
                            out.push('\n');
                            let mut new_ctx = ctx.clone();
                            new_ctx.list_depth = ctx.list_depth + 1;
                            new_ctx.ordered = true;
                            new_ctx.list_counter = 0;
                            process_children(child_ref, out, &new_ctx);
                            out.push('\n');
                        }
                        "li" => {
                            let indent = "  ".repeat(ctx.list_depth.saturating_sub(1));
                            let mut new_ctx = ctx.clone();
                            if ctx.ordered {
                                new_ctx.list_counter += 1;
                                out.push_str(&format!(
                                    "\n{}{}. ",
                                    indent, new_ctx.list_counter
                                ));
                            } else {
                                out.push_str(&format!("\n{}- ", indent));
                            }
                            process_children(child_ref, out, &new_ctx);
                        }
                        "pre" => {
                            out.push_str("\n\n```\n");
                            process_children(child_ref, out, ctx);
                            out.push_str("\n```\n\n");
                        }
                        "code" => {
                            out.push('`');
                            process_children(child_ref, out, ctx);
                            out.push('`');
                        }
                        "img" => {
                            let alt = elem.attr("alt").unwrap_or("");
                            let src = elem.attr("src").unwrap_or("");
                            let src = if src.starts_with('/') {
                                format!("https://archiveofourown.org{}", src)
                            } else {
                                src.to_string()
                            };
                            out.push_str(&format!("![{}]({})", alt, src));
                        }
                        "span" => {
                            let class = elem.attr("class").unwrap_or("");
                            if class.contains("landmark") {
                                // skip
                            } else {
                                process_children(child_ref, out, ctx);
                            }
                        }
                        "div" => {
                            let class = elem.attr("class").unwrap_or("");
                            if class.contains("landmark") {
                                // skip
                            } else {
                                process_children(child_ref, out, ctx);
                            }
                        }
                        "center" => {
                            out.push_str("\n\n<center>\n\n");
                            process_children(child_ref, out, ctx);
                            out.push_str("\n\n</center>\n\n");
                        }
                        "table" => {
                            out.push_str("\n\n");
                            process_table(child_ref, out);
                            out.push_str("\n\n");
                        }
                        _ => {
                            process_children(child_ref, out, ctx);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn process_table(table: ElementRef, out: &mut String) {
    let tr_sel = sel("tr");
    let th_sel = sel("th");
    let td_sel = sel("td");

    let rows: Vec<ElementRef> = table.select(&tr_sel).collect();
    if rows.is_empty() {
        return;
    }

    let mut table_data: Vec<Vec<String>> = Vec::new();

    for (i, row) in rows.iter().enumerate() {
        let headers: Vec<String> = row
            .select(&th_sel)
            .map(|c| html_node_to_md(c))
            .collect();
        let cells: Vec<String> = row
            .select(&td_sel)
            .map(|c| html_node_to_md(c))
            .collect();

        if !headers.is_empty() && i == 0 {
            table_data.push(headers);
        } else {
            table_data.push(cells);
        }
    }

    if table_data.is_empty() {
        return;
    }

    let cols = table_data.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return;
    }

    for (i, row) in table_data.iter().enumerate() {
        out.push('|');
        for j in 0..cols {
            let cell = row.get(j).map(|s| s.as_str()).unwrap_or("");
            out.push_str(&format!(" {} |", cell));
        }
        out.push('\n');

        if i == 0 {
            out.push('|');
            for _ in 0..cols {
                out.push_str(" --- |");
            }
            out.push('\n');
        }
    }
}

/// Generate a study HTML page for a chapter with translation slots under each paragraph.
/// Content is derived from the already-generated markdown strings, so empty paragraphs are
/// naturally excluded.
fn generate_chapter_html(
    title: &str,
    content: &str,
    notes_begin: &str,
    notes_end: &str,
    chapter_num: usize,
) -> String {
    let mut body = String::new();

    // Chapter notes (beginning)
    if !notes_begin.is_empty() {
        body.push_str("<div class=\"chapter-notes\">\n<p class=\"notes-label\">Chapter Notes</p>\n");
        body.push_str(&format!("<blockquote>{}</blockquote>", md_inline_to_html(notes_begin)));
        body.push_str("\n</div>\n<hr>\n");
    }

    // Main content: split md by double newline, generate para-block for each non-empty paragraph
    let mut para_id = 0usize;
    for block in content.split("\n\n") {
        let trimmed = block.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (html, is_para) = md_block_to_html(trimmed);
        if is_para {
            para_id += 1;
            body.push_str(&format!(
                "<div class=\"para-block\" id=\"p{pid}\">\n\
                 <p class=\"original\">{inner}</p>\n\
                 <div class=\"translation\">\n\
                 <p class=\"trans-text\"></p>\n\
                 <details class=\"vocab\"><summary>Vocabulary &amp; Chunks</summary>\n\
                 <div class=\"vocab-content\">\n\
                 <p class=\"vocab-item\"></p>\n\
                 <p class=\"chunks\"></p>\n\
                 </div>\n\
                 </details>\n\
                 </div>\n\
                 </div>\n\n",
                pid = para_id,
                inner = html
            ));
        } else {
            body.push_str(&html);
            body.push('\n');
        }
    }

    // Chapter notes (end)
    if !notes_end.is_empty() {
        body.push_str("<hr>\n<div class=\"chapter-notes\">\n<p class=\"notes-label\">End Notes</p>\n");
        body.push_str(&format!("<blockquote>{}</blockquote>", md_inline_to_html(notes_end)));
        body.push_str("\n</div>\n");
    }

    // Navigation
    let prev = if chapter_num > 1 {
        format!(
            "<a href=\"chapter{}.html\">&laquo; Previous</a>",
            chapter_num - 1
        )
    } else {
        String::new()
    };
    let next = format!(
        "<a href=\"chapter{}.html\">Next &raquo;</a>",
        chapter_num + 1
    );

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
{CSS}
</head>
<body>
<nav class="chapter-nav">
  <span>{prev}</span>
  <a href="metadata.html">Index</a>
  <span>{next}</span>
</nav>
<article>
<h1>{title}</h1>
{body}
</article>
<nav class="chapter-nav">
  <span>{prev}</span>
  <a href="metadata.html">Index</a>
  <span>{next}</span>
</nav>
</body>
</html>"#,
        title = html_escape(title),
        CSS = STUDY_CSS,
        prev = prev,
        next = next,
        body = body
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Convert markdown inline formatting back to HTML.
/// Handles: **bold**, *italic*, ~~strikethrough~~, [text](url), and passthrough <u>/<sup>/<sub>.
fn md_inline_to_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        // ~~strikethrough~~
        if i + 1 < len && chars[i] == '~' && chars[i + 1] == '~' {
            if let Some(end) = (i + 2..len - 1).find(|&j| chars[j] == '~' && chars[j + 1] == '~') {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str("<s>");
                out.push_str(&md_inline_to_html(&inner));
                out.push_str("</s>");
                i = end + 2;
                continue;
            }
        }
        // **bold**
        if i + 1 < len && chars[i] == '*' && chars[i + 1] == '*' {
            if let Some(end) = (i + 2..len - 1).find(|&j| chars[j] == '*' && chars[j + 1] == '*') {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str("<strong>");
                out.push_str(&md_inline_to_html(&inner));
                out.push_str("</strong>");
                i = end + 2;
                continue;
            }
        }
        // *italic*
        if chars[i] == '*' {
            if let Some(end) = (i + 1..len).find(|&j| chars[j] == '*') {
                let inner: String = chars[i + 1..end].iter().collect();
                out.push_str("<em>");
                out.push_str(&md_inline_to_html(&inner));
                out.push_str("</em>");
                i = end + 1;
                continue;
            }
        }
        // [text](url)
        if chars[i] == '[' {
            if let Some(bracket_end) = (i + 1..len).find(|&j| chars[j] == ']') {
                if bracket_end + 1 < len && chars[bracket_end + 1] == '(' {
                    if let Some(paren_end) = (bracket_end + 2..len).find(|&j| chars[j] == ')') {
                        let text: String = chars[i + 1..bracket_end].iter().collect();
                        let url: String = chars[bracket_end + 2..paren_end].iter().collect();
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            html_escape(&url),
                            md_inline_to_html(&text)
                        ));
                        i = paren_end + 1;
                        continue;
                    }
                }
            }
        }
        // ![alt](url) image
        if chars[i] == '!' && i + 1 < len && chars[i + 1] == '[' {
            if let Some(bracket_end) = (i + 2..len).find(|&j| chars[j] == ']') {
                if bracket_end + 1 < len && chars[bracket_end + 1] == '(' {
                    if let Some(paren_end) = (bracket_end + 2..len).find(|&j| chars[j] == ')') {
                        let alt: String = chars[i + 2..bracket_end].iter().collect();
                        let url: String = chars[bracket_end + 2..paren_end].iter().collect();
                        out.push_str(&format!(
                            "<img alt=\"{}\" src=\"{}\">",
                            html_escape(&alt),
                            html_escape(&url)
                        ));
                        i = paren_end + 1;
                        continue;
                    }
                }
            }
        }
        // Pass through raw HTML tags like <u>, </u>, <sup>, </sup>, <sub>, </sub>
        if chars[i] == '<' {
            if let Some(end) = (i + 1..len).find(|&j| chars[j] == '>') {
                let tag: String = chars[i..=end].iter().collect();
                out.push_str(&tag);
                i = end + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Convert a single md paragraph/block to HTML body content.
/// Returns (html_string, is_para) where is_para=true means it should get a translation slot.
fn md_block_to_html(block: &str) -> (String, bool) {
    let trimmed = block.trim();
    if trimmed == "---" {
        return ("<hr>".to_string(), false);
    }
    // Heading
    if let Some(rest) = trimmed.strip_prefix("# ") {
        return (format!("<h1>{}</h1>", md_inline_to_html(rest)), false);
    }
    if let Some(rest) = trimmed.strip_prefix("## ") {
        return (format!("<h2>{}</h2>", md_inline_to_html(rest)), false);
    }
    if let Some(rest) = trimmed.strip_prefix("### ") {
        return (format!("<h3>{}</h3>", md_inline_to_html(rest)), false);
    }
    if let Some(rest) = trimmed.strip_prefix("#### ") {
        return (format!("<h4>{}</h4>", md_inline_to_html(rest)), false);
    }
    if let Some(rest) = trimmed.strip_prefix("##### ") {
        return (format!("<h5>{}</h5>", md_inline_to_html(rest)), false);
    }
    if let Some(rest) = trimmed.strip_prefix("###### ") {
        return (format!("<h6>{}</h6>", md_inline_to_html(rest)), false);
    }
    // Blockquote block: all lines start with "> "
    if trimmed.lines().all(|l| l.trim_start().starts_with('>')) {
        let inner: String = trimmed
            .lines()
            .map(|l| {
                let s = l.trim_start().trim_start_matches('>');
                let s = s.strip_prefix(' ').unwrap_or(s);
                md_inline_to_html(s)
            })
            .collect::<Vec<_>>()
            .join("<br>");
        return (format!("<blockquote><p>{}</p></blockquote>", inner), false);
    }
    // List block: all lines start with "- " or digit. "
    let is_ul = trimmed.lines().all(|l| {
        let t = l.trim_start();
        t.starts_with("- ") || t.is_empty()
    });
    let is_ol = !is_ul && trimmed.lines().all(|l| {
        let t = l.trim_start();
        t.is_empty() || t.chars().next().map_or(false, |c| c.is_ascii_digit())
    });
    if is_ul {
        let items: String = trimmed
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let s = l.trim_start().strip_prefix("- ").unwrap_or(l.trim_start());
                format!("<li>{}</li>", md_inline_to_html(s))
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (format!("<ul>\n{}\n</ul>", items), false);
    }
    if is_ol {
        let items: String = trimmed
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let s = l.trim_start();
                // strip "N. " prefix
                let s = if let Some(dot) = s.find(". ") { &s[dot + 2..] } else { s };
                format!("<li>{}</li>", md_inline_to_html(s))
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (format!("<ol>\n{}\n</ol>", items), false);
    }
    // Markdown table
    if trimmed.contains('|') && trimmed.lines().count() >= 2 {
        let lines: Vec<&str> = trimmed.lines().collect();
        // Check for separator row (| --- |)
        let has_sep = lines.iter().any(|l| l.contains("---"));
        if has_sep {
            let mut html = String::from("<table>\n");
            let mut header_done = false;
            for line in &lines {
                if line.trim().contains("---") { continue; }
                let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
                if !header_done {
                    html.push_str("<thead><tr>");
                    for cell in &cells {
                        html.push_str(&format!("<th>{}</th>", md_inline_to_html(cell.trim())));
                    }
                    html.push_str("</tr></thead>\n<tbody>\n");
                    header_done = true;
                } else {
                    html.push_str("<tr>");
                    for cell in &cells {
                        html.push_str(&format!("<td>{}</td>", md_inline_to_html(cell.trim())));
                    }
                    html.push_str("</tr>\n");
                }
            }
            html.push_str("</tbody></table>");
            return (html, false);
        }
    }
    // Regular paragraph — join lines with <br> if multiline
    let html_lines: Vec<String> = trimmed
        .lines()
        .map(|l| md_inline_to_html(l))
        .collect();
    let inner = html_lines.join("<br>");
    (inner, true)
}

fn generate_metadata_html(meta: &WorkMeta) -> String {
    let mut rows = String::new();
    rows.push_str(&format!("<tr><th>Rating</th><td>{}</td></tr>\n", html_escape(&meta.rating)));
    rows.push_str(&format!(
        "<tr><th>Warnings</th><td>{}</td></tr>\n",
        html_escape(&meta.warnings.join(", "))
    ));
    if !meta.categories.is_empty() {
        rows.push_str(&format!(
            "<tr><th>Categories</th><td>{}</td></tr>\n",
            html_escape(&meta.categories.join(", "))
        ));
    }
    rows.push_str(&format!(
        "<tr><th>Fandoms</th><td>{}</td></tr>\n",
        html_escape(&meta.fandoms.join(", "))
    ));
    if !meta.relationships.is_empty() {
        rows.push_str(&format!(
            "<tr><th>Relationships</th><td>{}</td></tr>\n",
            html_escape(&meta.relationships.join(", "))
        ));
    }
    if !meta.characters.is_empty() {
        rows.push_str(&format!(
            "<tr><th>Characters</th><td>{}</td></tr>\n",
            html_escape(&meta.characters.join(", "))
        ));
    }
    rows.push_str(&format!(
        "<tr><th>Language</th><td>{}</td></tr>\n",
        html_escape(&meta.language)
    ));

    let stat_keys = ["Words", "Chapters", "Comments", "Kudos", "Bookmarks", "Hits"];
    let mut stat_rows = String::new();
    for key in &stat_keys {
        if let Some(val) = meta.stats.get(*key) {
            stat_rows.push_str(&format!(
                "<tr><th>{}</th><td>{}</td></tr>\n",
                key,
                html_escape(val)
            ));
        }
    }

    let mut tags_html = String::new();
    for tag in &meta.additional_tags {
        tags_html.push_str(&format!("<span class=\"tag\">{}</span> ", html_escape(tag)));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} - Metadata</title>
{CSS}
</head>
<body>
<article>
<h1>{title}</h1>
<p class="byline">by {authors}</p>
<hr>
<h2>Work Information</h2>
<table class="meta-table">{rows}</table>
<h2>Stats</h2>
<table class="meta-table">{stat_rows}</table>
{tags_section}
{summary_section}
</article>
</body>
</html>"#,
        title = html_escape(&meta.title),
        authors = html_escape(&meta.authors.join(", ")),
        CSS = STUDY_CSS,
        rows = rows,
        stat_rows = stat_rows,
        tags_section = if meta.additional_tags.is_empty() {
            String::new()
        } else {
            format!("<h2>Tags</h2>\n<div class=\"tags\">{}</div>", tags_html)
        },
        summary_section = if meta.summary.is_empty() {
            String::new()
        } else {
            format!(
                "<h2>Summary</h2>\n<div class=\"summary\">{}</div>",
                &meta.summary
            )
        }
    )
}

const STUDY_CSS: &str = r#"<style>
:root {
  --bg: #fafaf9; --fg: #1c1917; --muted: #78716c;
  --border: #d6d3d1; --accent: #2563eb; --accent-bg: #eff6ff;
  --trans-bg: #f0fdf4; --trans-border: #86efac;
  --vocab-bg: #fefce8; --vocab-border: #fde047;
  --note-bg: #f5f3ff; --note-border: #c4b5fd;
  --max-w: 48rem;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #1c1917; --fg: #e7e5e4; --muted: #a8a29e;
    --border: #44403c; --accent: #60a5fa; --accent-bg: #172554;
    --trans-bg: #052e16; --trans-border: #166534;
    --vocab-bg: #422006; --vocab-border: #a16207;
    --note-bg: #2e1065; --note-border: #6d28d9;
  }
}
* { box-sizing: border-box; margin: 0; padding: 0; }
body {
  font-family: 'Georgia', 'Noto Serif', serif;
  background: var(--bg); color: var(--fg);
  line-height: 1.8; max-width: var(--max-w);
  margin: 0 auto; padding: 1.5rem 1rem;
}
article { margin-bottom: 3rem; }
h1 { font-size: 1.6rem; margin-bottom: 0.5rem; border-bottom: 2px solid var(--accent); padding-bottom: 0.3rem; }
h2 { font-size: 1.3rem; margin: 1.5rem 0 0.5rem; color: var(--accent); }
h3, h4, h5, h6 { margin: 1.2rem 0 0.4rem; }
p { margin: 0.6rem 0; }
hr { border: none; border-top: 1px solid var(--border); margin: 1.5rem 0; }
a { color: var(--accent); }
blockquote { border-left: 3px solid var(--border); padding-left: 1rem; color: var(--muted); margin: 0.8rem 0; }
strong, b { font-weight: 700; }
em, i { font-style: italic; }

.byline { color: var(--muted); margin-bottom: 1rem; }
.meta-table { width: 100%; border-collapse: collapse; margin: 0.5rem 0 1rem; }
.meta-table th { text-align: left; padding: 0.4rem 0.8rem; background: var(--accent-bg); width: 30%; border: 1px solid var(--border); }
.meta-table td { padding: 0.4rem 0.8rem; border: 1px solid var(--border); }
.tags { margin: 0.5rem 0; }
.tag { display: inline-block; background: var(--accent-bg); color: var(--accent); padding: 0.15rem 0.5rem; border-radius: 0.8rem; font-size: 0.85rem; margin: 0.2rem; }

.chapter-nav { display: flex; justify-content: space-between; padding: 0.8rem 0; border-bottom: 1px solid var(--border); margin-bottom: 1.5rem; font-size: 0.9rem; }
.chapter-nav:last-child { border-bottom: none; border-top: 1px solid var(--border); margin-top: 1.5rem; margin-bottom: 0; }

.para-block {
  margin: 1.2rem 0; padding: 0.8rem;
  border-left: 3px solid var(--border);
  border-radius: 0 0.4rem 0.4rem 0;
  transition: border-color 0.2s;
}
.para-block:hover { border-left-color: var(--accent); }
.para-block .original { font-size: 1.05rem; line-height: 1.85; margin-bottom: 0.4rem; }
.translation {
  background: var(--trans-bg); border: 1px dashed var(--trans-border);
  border-radius: 0.4rem; padding: 0.6rem 0.8rem; margin-top: 0.5rem;
  min-height: 2.5rem;
}
.trans-text { color: var(--muted); font-size: 0.95rem; }
.trans-text:empty::before { content: "Translation / \7FFB\8BD1"; color: var(--muted); opacity: 0.4; }
.vocab {
  margin-top: 0.5rem; font-size: 0.88rem;
}
.vocab summary {
  cursor: pointer; color: var(--accent); font-weight: 600; font-size: 0.85rem;
  user-select: none; padding: 0.2rem 0;
}
.vocab-content {
  background: var(--vocab-bg); border: 1px solid var(--vocab-border);
  border-radius: 0.3rem; padding: 0.5rem 0.7rem; margin-top: 0.3rem;
}
.vocab-item:empty::before { content: "Word (pos) /phonetic/ — definition  e.g. ..."; color: var(--muted); opacity: 0.4; }
.chunks:empty::before { content: "Chunks: phrase1, phrase2, ..."; color: var(--muted); opacity: 0.4; }

.chapter-notes {
  background: var(--note-bg); border: 1px solid var(--note-border);
  border-radius: 0.5rem; padding: 0.8rem 1rem; margin: 1rem 0;
}
.notes-label { font-weight: 700; color: var(--accent); margin-bottom: 0.4rem; }

.summary { margin: 0.5rem 0; padding: 0.8rem; background: var(--accent-bg); border-radius: 0.4rem; }
</style>"#;

fn generate_prompt(work_title: &str) -> String {
    format!(
        r#"# Translation & Vocabulary Prompt for: {title}

You are a professional English-to-Chinese literary translator and language tutor. You will receive a chapter of the English fanfiction "{title}" from AO3. Your task is to process each paragraph and produce structured output.

## Input Format
The chapter text is divided into numbered paragraphs (marked with IDs like p1, p2, ...). Process each paragraph individually.

## Output Format
For each paragraph, output the following structure (keep the paragraph ID):

---

### p[N]

**Translation:**
[Fluent, natural Chinese translation that preserves the tone, style, and literary quality of the original. Use 「」 for dialogue. Do not translate proper nouns — keep character names, place names, and faction names in English.]

**Vocabulary (>IETLS 6.5):**
- **word** (part_of_speech) /phonetic/ — Chinese definition
  - Example: [a short example sentence from the text or a common usage]
- **word2** (part_of_speech) /phonetic/ — Chinese definition
  - Example: ...
[List 3-8 important or difficult words per paragraph. Skip common words (the, is, a, etc.). Prioritize: domain-specific terms, literary vocabulary, phrasal verbs, idioms.]

**Chunks (>IETLS 6.5):**
- **phrase/collocation** — meaning in Chinese; usage note if needed
- **phrase2** — meaning
[List 2-5 useful multi-word expressions, collocations, or idiomatic phrases from the paragraph.]

---

## Guidelines
1. Translation should read naturally in Chinese — it is literary translation, not word-for-word
2. Keep the author's tone: if humorous, stay humorous; if tense, stay tense
3. For vocabulary, focus on words a B2-C1 English learner might not know
4. Phonetic notation uses IPA (International Phonetic Alphabet)
5. Chunks should be phrases that are reusable in other contexts
6. If a paragraph is very short (< 10 words) or is just a scene break / date header, you may write "N/A" for vocabulary and chunks
7. Process ALL paragraphs — do not skip any

## Example Output

### p1

**Translation:**
阳光透过古老的彩色玻璃窗倾泻而入，在石板地面上投下万花筒般的图案，将整个大厅染上了一层温暖而空灵的光辉。

**Vocabulary:**
- **kaleidoscope** (n) /kəˈlaɪdəskoʊp/ — 万花筒；千变万化的景象
  - Example: The garden was a kaleidoscope of colors in spring.
- **ethereal** (adj) /ɪˈθɪriəl/ — 空灵的，超凡脱俗的
  - Example: The music had an ethereal quality that captivated everyone.

**Chunks:**
- **cast patterns on** — 在……上投下图案
- **bathe in light** — 沐浴在光辉中

---

Now process the chapter text that follows. Output ALL paragraphs.
"#,
        title = work_title
    )
}

fn prefix_lines(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}
