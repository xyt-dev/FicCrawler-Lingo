use clap::Parser;
use regex::Regex;
use reqwest::blocking::Client;
use scraper::{ElementRef, Html, Selector};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "ao3-scraper", about = "Scrape AO3 works into markdown files")]
struct Args {
    /// AO3 work URL (e.g. https://archiveofourown.org/works/12345)
    url: String,

    /// Output directory (default: current directory)
    #[arg(short, long, default_value = ".")]
    output: String,

    /// Delay between requests in milliseconds
    #[arg(short, long, default_value_t = 1500)]
    delay: u64,
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
    println!("Work ID: {}", work_id);

    let client = Client::builder()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0")
        .cookie_store(true)
        .build()
        .expect("Failed to build HTTP client");

    // Fetch the full work page (all chapters in one page)
    let full_url = format!(
        "https://archiveofourown.org/works/{}?view_full_work=true&view_adult=true",
        work_id
    );
    println!("Fetching: {}", full_url);

    let resp = client
        .get(&full_url)
        .send()
        .expect("Failed to fetch work page");

    if !resp.status().is_success() {
        eprintln!("HTTP error: {}", resp.status());
        std::process::exit(1);
    }

    let html_text = resp.text().expect("Failed to read response body");
    let document = Html::parse_document(&html_text);

    // Extract metadata
    let meta = extract_metadata(&document);
    println!("Title: {}", meta.title);
    println!("Authors: {}", meta.authors.join(", "));

    // Sanitize title for directory name
    let dir_name = sanitize_filename::sanitize(&meta.title);
    let out_dir = PathBuf::from(&args.output).join(&dir_name);
    fs::create_dir_all(&out_dir).expect("Failed to create output directory");

    // Write metadata file
    let meta_md = format_metadata_md(&meta);
    let meta_path = out_dir.join("metadata.md");
    fs::write(&meta_path, &meta_md).expect("Failed to write metadata.md");
    println!("Wrote: {}", meta_path.display());

    // Extract chapters
    let chapters = extract_chapters(&document);
    println!("Found {} chapter(s)", chapters.len());

    if chapters.is_empty() {
        // Single-chapter work without chapter dividers
        let content = extract_single_chapter_content(&document);
        let chapter_path = out_dir.join("chapter1.md");
        fs::write(&chapter_path, &content).expect("Failed to write chapter");
        println!("Wrote: {}", chapter_path.display());
    } else {
        for (i, (title, content)) in chapters.iter().enumerate() {
            let filename = format!("chapter{}.md", i + 1);
            let chapter_path = out_dir.join(&filename);

            let mut md = String::new();
            if !title.is_empty() {
                md.push_str(&format!("# {}\n\n", title));
            }
            md.push_str(content);

            fs::write(&chapter_path, &md).expect("Failed to write chapter file");
            println!("Wrote: {}", chapter_path.display());

            if i < chapters.len() - 1 {
                thread::sleep(Duration::from_millis(args.delay));
            }
        }
    }

    println!("\nDone! Files saved to: {}", out_dir.display());
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
        // Fallback: try any link inside byline
        authors = doc
            .select(&sel("h3.byline a"))
            .map(|e| e.text().collect::<String>().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    if authors.is_empty() {
        // Fallback: use the byline text directly
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

    // Stats
    let mut stats = HashMap::new();
    let stats_sel = sel("dl.stats");
    if let Some(stats_dl) = doc.select(&stats_sel).next() {
        let dt_sel = sel("dt");
        let dd_sel = sel("dd");
        let dts: Vec<String> = stats_dl
            .select(&dt_sel)
            .map(|e| e.text().collect::<String>().trim().trim_end_matches(':').to_string())
            .collect();
        let dds: Vec<String> = stats_dl
            .select(&dd_sel)
            .map(|e| e.text().collect::<String>().trim().to_string())
            .collect();
        for (k, v) in dts.into_iter().zip(dds.into_iter()) {
            stats.insert(k, v);
        }
    }

    // Summary
    let summary = doc
        .select(&sel("div.preface .summary blockquote"))
        .next()
        .map(|e| html_node_to_md(e))
        .unwrap_or_default();

    // Notes (beginning)
    let notes_begin = doc
        .select(&sel("div.preface .notes blockquote"))
        .next()
        .map(|e| html_node_to_md(e))
        .unwrap_or_default();

    // Notes (end)
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

    // Stats
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
    // Include any stats not in the standard list
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

    // Tags
    if !meta.additional_tags.is_empty() {
        md.push_str("## Tags\n\n");
        for tag in &meta.additional_tags {
            md.push_str(&format!("- {}\n", tag));
        }
        md.push('\n');
    }

    // Series
    if !meta.series.is_empty() {
        md.push_str("## Series\n\n");
        for s in &meta.series {
            md.push_str(&format!("- {}\n", s));
        }
        md.push('\n');
    }

    // Summary
    if !meta.summary.is_empty() {
        md.push_str("## Summary\n\n");
        md.push_str(&meta.summary);
        md.push_str("\n\n");
    }

    // Notes
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

fn extract_chapters(doc: &Html) -> Vec<(String, String)> {
    // Use id pattern to only match top-level chapter divs (div#chapter-1, div#chapter-2, ...)
    // NOT the inner div.chapter.preface.group which also has class "chapter"
    let chapter_sel = sel("div[id^='chapter-']");
    let chapters: Vec<ElementRef> = doc.select(&chapter_sel).collect();

    if chapters.is_empty() {
        return vec![];
    }

    let mut result = Vec::new();

    for chapter_el in chapters {
        // Chapter title
        let title = chapter_el
            .select(&sel("h3.title"))
            .next()
            .map(|e| {
                let text = e.text().collect::<String>();
                text.trim().to_string()
            })
            .unwrap_or_default();

        // Chapter content - must be div.userstuff with role="article" (the actual body)
        // NOT blockquote.userstuff (which appears in chapter notes)
        let content = chapter_el
            .select(&sel("div.userstuff[role='article']"))
            .next()
            .map(|e| html_node_to_md(e))
            .unwrap_or_default();

        // Chapter-level notes (beginning) - inside the preface group's notes module
        let chapter_notes_begin = chapter_el
            .select(&sel("div.preface div.notes blockquote.userstuff"))
            .next()
            .map(|e| html_node_to_md(e))
            .unwrap_or_default();

        // Chapter-level notes (end)
        let chapter_notes_end = chapter_el
            .select(&sel("div.end.notes blockquote"))
            .next()
            .map(|e| html_node_to_md(e))
            .unwrap_or_default();

        let mut full_content = String::new();
        if !chapter_notes_begin.is_empty() {
            full_content.push_str(&format!(
                "> **Chapter Notes:**\n>\n{}\n\n---\n\n",
                prefix_lines(&chapter_notes_begin, "> ")
            ));
        }
        full_content.push_str(&content);
        if !chapter_notes_end.is_empty() {
            full_content.push_str(&format!(
                "\n\n---\n\n> **End Notes:**\n>\n{}",
                prefix_lines(&chapter_notes_end, "> ")
            ));
        }

        result.push((title, full_content));
    }

    result
}

fn extract_single_chapter_content(doc: &Html) -> String {
    doc.select(&sel("div.userstuff"))
        .next()
        .map(|e| html_node_to_md(e))
        .unwrap_or_else(|| "No content found.".to_string())
}

/// Convert an HTML element tree into Markdown, preserving formatting.
fn html_node_to_md(el: ElementRef) -> String {
    let mut output = String::new();
    process_children(el, &mut output, &InlineCtx::default());
    // Clean up excessive blank lines
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
                                let prefix = "#".repeat(level.parse::<usize>().unwrap_or(1));
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
                            // Skip AO3 landmark headings like "Chapter Text"
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

fn prefix_lines(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{}{}", prefix, line))
        .collect::<Vec<_>>()
        .join("\n")
}
