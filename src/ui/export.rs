//! Сохранение переписки в файл: обычный текст, Markdown, HTML или JSON.
//!
//! Сохраняется то, что сейчас загружено в чате (последние сообщения и всё, что подгружено прокруткой вверх).
//! HTML получается одним файлом без скриптов и внешних ресурсов: открывается двойным щелчком, ничего не запрашивает.

use std::path::Path;

use serde_json::{Value, json};

use super::App;
use super::text::{local_date, months};
use crate::tg::{ChatMsg, MediaKind};
use crate::{tr, trf};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Формат файла.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Txt,
    Md,
    Html,
    Json,
}

impl Format {
    /// Расширение файла.
    pub(super) fn ext(self) -> &'static str {
        match self {
            Format::Txt => "txt",
            Format::Md => "md",
            Format::Html => "html",
            Format::Json => "json",
        }
    }

    /// Формат по слову из команды; пустое слово — обычный текст.
    pub(super) fn parse(word: &str) -> Option<Format> {
        match word.trim().trim_start_matches('.').to_lowercase().as_str() {
            "" | "txt" | "text" | "текст" => Some(Format::Txt),
            "md" | "markdown" => Some(Format::Md),
            "html" | "htm" => Some(Format::Html),
            "json" => Some(Format::Json),
            _ => None,
        }
    }
}

impl App {
    /// `/export [txt|md|html|json]`: сохраняет загруженную переписку открытого чата в `папка/<чат>.<формат>`.
    pub(super) fn cmd_export(&mut self, arg: &str, dir: &Path) {
        let Some(c) = self.selected().cloned() else {
            self.status = tr!("Сначала выберите чат", "Select a chat first").into();
            return;
        };
        let Some(format) = Format::parse(arg) else {
            self.status = tr!("Форматы: /export txt, md, html или json", "Formats: /export txt, md, html or json").into();
            return;
        };
        match self.histories.get(&c.id) {
            Some(h) if !h.is_empty() => {
                self.status = match crate::util::write_export(dir, &c.name, format.ext(), &render(format, &c.name, h)) {
                    Ok(p) => trf!("Сохранено: {} ({} сообщ.)", "Saved: {} ({} messages)", p.display(), h.len()),
                    Err(e) => trf!("Ошибка: не удалось сохранить файл: {e}", "Error: could not save the file: {e}"),
                };
            }
            _ => self.status = tr!("В этом чате пока нечего сохранять", "Nothing to save in this chat yet").into(),
        }
    }
}

/// Переписка в нужном формате.
pub(super) fn render(format: Format, chat: &str, msgs: &[ChatMsg]) -> String {
    match format {
        Format::Txt => render_txt(chat, msgs),
        Format::Md => render_md(chat, msgs),
        Format::Html => render_html(chat, msgs),
        Format::Json => render_json(chat, msgs),
    }
}

/// Время сообщения по часам компьютера.
fn stamp(ts: i64, fmt: &str) -> String {
    let tz = jiff::tz::TimeZone::system();
    jiff::Timestamp::from_second(ts).map(|t| t.to_zoned(tz).strftime(fmt).to_string()).unwrap_or_default()
}

/// Заголовок дня: «28 октября 2025».
fn day_title(date: (i16, i8, i8)) -> String {
    let month = months().get((date.1 as usize).wrapping_sub(1)).copied().unwrap_or("");
    let (y, d) = (date.0, date.2);
    trf!("{d} {month} {y}", "{month} {d}, {y}")
}

/// Первые символы цитируемого сообщения для строки «в ответ на».
fn quote_of(msgs: &[ChatMsg], id: i32) -> Option<String> {
    let q = msgs.iter().find(|m| m.id == id)?;
    let flat = q.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = flat.chars().take(80).collect();
    Some(format!("{}: {}{}", q.sender, short, if flat.chars().count() > 80 { "…" } else { "" }))
}

fn reactions_line(m: &ChatMsg) -> String {
    m.reactions.iter().map(|r| format!("{} {}", r.emoji, r.count)).collect::<Vec<_>>().join(" · ")
}

fn forwarded_from(m: &ChatMsg) -> Option<String> {
    m.fwd.as_ref().map(|f| f.name.clone().unwrap_or_else(|| tr!("скрытый автор", "hidden author").to_string()))
}

// ------------------------------------------------------------------ txt

fn render_txt(chat: &str, msgs: &[ChatMsg]) -> String {
    let mut out = trf!("Переписка: {chat}\nСохранено Tg-Lite {VERSION}\n\n", "Chat: {chat}\nSaved by Tg-Lite {VERSION}\n\n");
    for m in msgs {
        out.push_str(&format!("[{}] {}: {}\n", stamp(m.ts, "%d.%m.%Y %H:%M"), m.sender, m.text.replace('\n', "\n    ")));
    }
    out
}

// ------------------------------------------------------------------ markdown

/// Экранирует то, что Markdown принял бы за разметку: сообщение попадает в файл ровно тем текстом, который был в чате.
fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for (n, line) in s.split('\n').enumerate() {
        if n > 0 {
            out.push_str("  \n"); // два пробела — перенос строки внутри абзаца
        }
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        let numbered = digits > 0 && matches!(line[digits..].chars().next(), Some('.' | ')'));
        for (i, c) in line.chars().enumerate() {
            let block_marker = i == 0 && matches!(c, '#' | '-' | '+' | '=');
            let list_dot = numbered && i == digits;
            if block_marker || list_dot || matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '~' | '|') {
                out.push('\\');
            }
            out.push(c);
        }
    }
    out
}

fn render_md(chat: &str, msgs: &[ChatMsg]) -> String {
    let tz = jiff::tz::TimeZone::system();
    let mut out = format!("# {}\n\n*{}*\n", md_escape(chat), trf!("Сохранено Tg-Lite {VERSION}", "Saved by Tg-Lite {VERSION}"));
    let mut day = None;
    for m in msgs {
        let d = local_date(m.ts, &tz);
        if d != day {
            day = d;
            if let Some(d) = d {
                out.push_str(&format!("\n## {}\n", day_title(d)));
            }
        }
        let edited = if m.edited { format!(" *({})*", tr!("изменено", "edited")) } else { String::new() };
        out.push_str(&format!("\n**{}** · {}{edited}\n", md_escape(&m.sender), stamp(m.ts, "%H:%M")));
        if let Some(name) = forwarded_from(m) {
            out.push_str(&format!("> ↪ {} {}\n", tr!("переслано от", "forwarded from"), md_escape(&name)));
        }
        if let Some(q) = m.reply_to.and_then(|r| quote_of(msgs, r)) {
            out.push_str(&format!("> ↩ {}\n", md_escape(&q)));
        }
        if !m.text.trim().is_empty() {
            out.push('\n');
            out.push_str(&md_escape(m.text.trim_end()));
            out.push('\n');
        }
        let reactions = reactions_line(m);
        if !reactions.is_empty() {
            out.push_str(&format!("\n{}\n", md_escape(&reactions)));
        }
    }
    out
}

// ------------------------------------------------------------------ html

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Текст сообщения для HTML: всё экранировано, переводы строк сохранены, ссылки http(s) кликабельны.
fn html_text(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        let next = ["https://", "http://"].iter().filter_map(|p| rest.find(p)).min();
        let Some(i) = next else {
            out.push_str(&html_escape(rest));
            break;
        };
        out.push_str(&html_escape(&rest[..i]));
        let tail = &rest[i..];
        let end = tail.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"')).unwrap_or(tail.len());
        // Знаки препинания после ссылки в неё не входят.
        let url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '»', '”', '\'']);
        let scheme_len = if url.starts_with("https://") { 8 } else { 7 };
        if url.len() > scheme_len {
            let esc = html_escape(url);
            out.push_str(&format!("<a href=\"{esc}\" rel=\"noopener noreferrer\">{esc}</a>"));
            rest = &tail[url.len()..];
        } else {
            out.push_str(&html_escape(&tail[..scheme_len]));
            rest = &tail[scheme_len..];
        }
    }
    out.replace('\n', "<br>\n")
}

const HTML_STYLE: &str = "\
:root{color-scheme:light dark;--bg:#fff;--fg:#1c1c1e;--muted:#7a7a80;--in:#f0f0f3;--out:#d9eefc;--accent:#2a7ab9}\
@media (prefers-color-scheme:dark){:root{--bg:#17171a;--fg:#e8e8ec;--muted:#9a9aa3;--in:#26262b;--out:#1f3b52;--accent:#6bb3e8}}\
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.45 system-ui,-apple-system,\"Segoe UI\",sans-serif}\
main{max-width:46rem;margin:0 auto;padding:1rem}\
h1{font-size:1.3rem;margin:.2rem 0}\
.meta{color:var(--muted);font-size:.85rem;margin:0 0 1rem}\
.day{text-align:center;color:var(--muted);font-size:.85rem;font-weight:500;margin:1.4rem 0 .6rem}\
.m{max-width:85%;width:fit-content;margin:.35rem 0;padding:.45rem .7rem;border-radius:.8rem;background:var(--in);overflow-wrap:anywhere}\
.m.out{margin-left:auto;background:var(--out)}\
.who{font-size:.8rem;color:var(--accent);font-weight:600}\
.t{color:var(--muted);font-weight:400;margin-left:.4rem}\
.q{border-left:3px solid var(--accent);padding-left:.5rem;margin:.2rem 0;color:var(--muted);font-size:.88rem}\
.r{color:var(--muted);font-size:.85rem;margin-top:.2rem}\
a{color:var(--accent)}";

fn render_html(chat: &str, msgs: &[ChatMsg]) -> String {
    let tz = jiff::tz::TimeZone::system();
    let lang = if crate::i18n::en() { "en" } else { "ru" };
    let title = html_escape(chat);
    let mut out = format!(
        "<!doctype html>\n<html lang=\"{lang}\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <meta name=\"generator\" content=\"Tg-Lite {VERSION}\">\n<title>{title} — Tg-Lite</title>\n<style>{HTML_STYLE}</style>\n</head>\n<body>\n<main>\n\
         <h1>{title}</h1>\n<p class=\"meta\">{}</p>\n",
        trf!("Сохранено Tg-Lite {VERSION}", "Saved by Tg-Lite {VERSION}")
    );
    let mut day = None;
    for m in msgs {
        let d = local_date(m.ts, &tz);
        if d != day {
            day = d;
            if let Some(d) = d {
                out.push_str(&format!("<h2 class=\"day\">{}</h2>\n", html_escape(&day_title(d))));
            }
        }
        let class = if m.outgoing { "m out" } else { "m" };
        let edited = if m.edited { format!(" ({})", tr!("изменено", "edited")) } else { String::new() };
        out.push_str(&format!(
            "<div class=\"{class}\">\n<div class=\"who\">{}<span class=\"t\">{}{}</span></div>\n",
            html_escape(&m.sender),
            stamp(m.ts, "%H:%M"),
            html_escape(&edited)
        ));
        if let Some(name) = forwarded_from(m) {
            out.push_str(&format!("<div class=\"q\">↪ {} {}</div>\n", tr!("переслано от", "forwarded from"), html_escape(&name)));
        }
        if let Some(q) = m.reply_to.and_then(|r| quote_of(msgs, r)) {
            out.push_str(&format!("<div class=\"q\">↩ {}</div>\n", html_escape(&q)));
        }
        if !m.text.trim().is_empty() {
            out.push_str(&format!("<div>{}</div>\n", html_text(m.text.trim_end())));
        }
        let reactions = reactions_line(m);
        if !reactions.is_empty() {
            out.push_str(&format!("<div class=\"r\">{}</div>\n", html_escape(&reactions)));
        }
        out.push_str("</div>\n");
    }
    out.push_str("</main>\n</body>\n</html>\n");
    out
}

// ------------------------------------------------------------------ json

fn media_kind(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Photo => "photo",
        MediaKind::Voice => "voice",
        MediaKind::Audio => "audio",
        MediaKind::Video => "video",
        MediaKind::VideoNote => "video_note",
        MediaKind::Gif => "gif",
        MediaKind::Sticker => "sticker",
        MediaKind::File => "file",
    }
}

fn render_json(chat: &str, msgs: &[ChatMsg]) -> String {
    const ISO: &str = "%Y-%m-%dT%H:%M:%S%:z";
    let list: Vec<Value> = msgs
        .iter()
        .map(|m| {
            json!({
                "id": m.id,
                "date": stamp(m.ts, ISO),
                "timestamp": m.ts,
                "sender": m.sender,
                "outgoing": m.outgoing,
                "text": m.text,
                "edited": m.edited,
                "reply_to": m.reply_to,
                "forwarded_from": forwarded_from(m),
                "views": m.views,
                "reactions": m.reactions.iter().map(|r| json!({"emoji": r.emoji, "count": r.count, "mine": r.mine})).collect::<Vec<_>>(),
                "media": m.media.as_ref().map(|x| json!({"kind": media_kind(x.kind), "name": x.name, "size": x.size})),
            })
        })
        .collect();
    let doc = json!({
        "chat": chat,
        "exported_by": format!("Tg-Lite {VERSION}"),
        "exported_at": jiff::Zoned::now().strftime(ISO).to_string(),
        "messages": list,
    });
    let mut s = serde_json::to_string_pretty(&doc).unwrap_or_default();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tg::{Fwd, MediaInfo, Reaction};

    fn m(id: i32, outgoing: bool, sender: &str, text: &str, ts: i64) -> ChatMsg {
        ChatMsg { id, ts, outgoing, sender: sender.into(), text: text.into(), ..Default::default() }
    }

    fn sample() -> Vec<ChatMsg> {
        let mut a = m(1, false, "Мама", "Привет\nкак *дела*? #1", 1_761_650_000);
        a.reactions = vec![Reaction { emoji: "👍".into(), count: 3, mine: true }];
        let mut b = m(2, true, "Я", "хорошо, смотри https://example.com/a?b=1&c=2.", 1_761_650_060);
        b.reply_to = Some(1);
        b.edited = true;
        let mut c = m(3, false, "Мама", "[фото] закат", 1_761_650_000 + 86_400 * 2);
        c.media = Some(MediaInfo { kind: MediaKind::Photo, name: "photo_3.jpg".into(), size: Some(2048) });
        c.fwd = Some(Fwd { name: Some("Иван".into()), from: None });
        vec![a, b, c]
    }

    #[test]
    fn formats_are_recognized_by_name() {
        for (w, f) in [
            ("", Format::Txt),
            ("txt", Format::Txt),
            (".MD", Format::Md),
            ("markdown", Format::Md),
            ("HTML", Format::Html),
            ("json", Format::Json),
        ] {
            assert_eq!(Format::parse(w), Some(f), "{w}");
        }
        assert_eq!(Format::parse("pdf"), None);
        assert_eq!(Format::parse("docx"), None);
        let exts: Vec<_> = [Format::Txt, Format::Md, Format::Html, Format::Json].iter().map(|f| f.ext()).collect();
        assert_eq!(exts, ["txt", "md", "html", "json"]);
    }

    #[test]
    fn text_export_keeps_the_old_layout() {
        let text = render(Format::Txt, "Мама", &[m(1, false, "Мама", "Привет\nкак дела", 0), m(2, true, "Я", "хорошо", 0)]);
        assert!(text.starts_with("Переписка: Мама\nСохранено Tg-Lite "));
        assert!(text.contains("Мама: Привет\n    как дела"));
        assert!(text.contains("Я: хорошо"));
    }

    #[test]
    fn markdown_has_day_headers_quotes_escapes_and_line_breaks() {
        let out = render(Format::Md, "Мама_и_папа", &sample());
        assert!(out.starts_with("# Мама\\_и\\_папа\n"), "{out}");
        assert_eq!(out.matches("\n## ").count(), 2, "два разных дня — два заголовка:\n{out}");
        assert!(out.contains("**Мама** · "), "{out}");
        assert!(out.contains("Привет  \nкак \\*дела\\*? #1"), "переносы и звёздочки экранированы: {out}");
        assert!(out.contains("> ↩ Мама: Привет как \\*дела\\*? #1"), "цитата ответа: {out}");
        assert!(out.contains("*(изменено)*") && out.contains("↪ переслано от Иван"), "{out}");
        assert!(out.contains("👍 3"), "{out}");
        assert!(out.contains("\\[фото\\] закат"), "{out}");
        // Разметка в начале строки не превращается в заголовок, список или цитату.
        let md = render(Format::Md, "x", &[m(1, false, "A", "# не заголовок\n- не список\n1. не нумерация\n> не цитата", 0)]);
        for want in ["\\# не заголовок", "\\- не список", "1\\. не нумерация", "\\> не цитата"] {
            assert!(md.contains(want), "{want}: {md}");
        }
    }

    #[test]
    fn html_is_one_safe_file_with_clickable_web_links_only() {
        let mut msgs = sample();
        msgs.push(m(
            4,
            false,
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(1)> & \"кавычки\" javascript:alert(1) http:// https://",
            0,
        ));
        msgs.push(m(5, false, "Я", "см. http://a.example/x), и https://b.example/y!", 0));
        let out = render(Format::Html, "Мама & <папа>", &msgs);
        assert!(out.starts_with("<!doctype html>\n<html lang=\"ru\">"), "{out}");
        assert!(out.contains("<title>Мама &amp; &lt;папа&gt; — Tg-Lite</title>"), "{out}");
        assert!(!out.contains("<script") && !out.contains("<img"), "разметка из сообщений не проходит: {out}");
        assert!(out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(out.contains("&lt;img src=x onerror=alert(1)&gt; &amp; &quot;кавычки&quot; javascript:alert(1)"), "{out}");
        assert!(
            out.contains(
                "<a href=\"https://example.com/a?b=1&amp;c=2\" rel=\"noopener noreferrer\">https://example.com/a?b=1&amp;c=2</a>."
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "<a href=\"http://a.example/x\" rel=\"noopener noreferrer\">http://a.example/x</a>), и <a href=\"https://b.example/y\""
            ),
            "{out}"
        );
        assert!(!out.contains("href=\"javascript"), "{out}");
        // Ссылки только в самих сообщениях, ресурсов извне страница не просит.
        assert!(!out.contains("src=\"http") && !out.contains("<link") && !out.contains("@import") && !out.contains("url(http"), "{out}");
        assert!(out.contains("class=\"m out\"") && out.contains("<br>\n"), "{out}");
        assert!(out.ends_with("</html>\n"));
        // Английский интерфейс — английский атрибут языка и подписи.
        crate::i18n::set(1);
        let en = render(Format::Html, "Mom", &msgs[..3]);
        crate::i18n::set(0);
        assert!(en.contains("<html lang=\"en\">") && en.contains("Saved by Tg-Lite") && en.contains("forwarded from Иван"), "{en}");
    }

    #[test]
    fn json_is_valid_and_complete() {
        let out = render(Format::Json, "Мама \"и\" папа", &sample());
        let v: Value = serde_json::from_str(&out).expect("корректный JSON");
        assert_eq!(v["chat"], "Мама \"и\" папа");
        assert!(v["exported_by"].as_str().unwrap().starts_with("Tg-Lite "));
        let list = v["messages"].as_array().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0]["text"], "Привет\nкак *дела*? #1");
        assert_eq!(list[0]["reactions"][0]["emoji"], "👍");
        assert_eq!(list[0]["reactions"][0]["count"], 3);
        assert_eq!(list[1]["outgoing"], true);
        assert_eq!(list[1]["edited"], true);
        assert_eq!(list[1]["reply_to"], 1);
        assert_eq!(list[2]["forwarded_from"], "Иван");
        assert_eq!(list[2]["media"]["kind"], "photo");
        assert_eq!(list[2]["media"]["name"], "photo_3.jpg");
        assert_eq!(list[2]["media"]["size"], 2048);
        assert!(list[0]["media"].is_null() && list[0]["reply_to"].is_null());
        let date = list[0]["date"].as_str().unwrap();
        assert!(
            date.starts_with("2025-10-2") && date.contains('T') && (date.contains('+') || date.contains('-') || date.ends_with('Z')),
            "{date}"
        );
        assert_eq!(list[0]["timestamp"], 1_761_650_000);
    }

    #[test]
    fn empty_and_odd_input_never_breaks_any_format() {
        for f in [Format::Txt, Format::Md, Format::Html, Format::Json] {
            let empty = render(f, "Пусто", &[]);
            assert!(empty.contains("Пусто"), "{f:?}");
            let odd = render(f, "", &[m(0, false, "", "", -1), m(1, true, "Я", "\n\n", i64::MAX), m(2, false, "Я", "😀 \u{202e}текст", 0)]);
            assert!(!odd.is_empty(), "{f:?}");
        }
        assert!(serde_json::from_str::<Value>(&render(Format::Json, "", &[m(0, false, "", "", -1)])).is_ok());
    }
}
