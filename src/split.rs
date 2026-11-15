//! Деление длинного текста на сообщения: Telegram принимает не больше 4096 символов за раз.
//!
//! Режем по границам: пустая строка, перевод строки, конец предложения, пробел. Только если
//! в последних двух третях окна ничего подходящего нет, режем где придётся. Лимит считаем в единицах
//! UTF-16: часть не будет отвергнута, как бы Telegram ни считал символы. Блок кода, разрезанный
//! посередине (режим Markdown), закрывается в конце части и открывается снова в начале следующей.

/// Предел Telegram на одно сообщение.
pub const MESSAGE_LIMIT: usize = 4096;
/// Из одного текста делаем не больше стольких сообщений: дальше это уже файл (`/send`), а не переписка.
pub const MAX_PARTS: usize = 12;
/// Сколько символов принимает поле ввода (чуть меньше десяти полных сообщений).
pub const MAX_INPUT_CHARS: usize = 40_000;
/// Запас под закрывающие и открывающие ограждения кода.
const FENCE_ROOM: usize = 24;
/// Сколько знаков строки с ограждением повторяется в начале следующей части.
const FENCE_LINE_MAX: usize = 24;

/// Одно сообщение из длинного текста.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// Что отправить.
    pub text: String,
    /// С какого байта исходного текста эта часть начинается: с него берётся остаток,
    /// если эту часть отправить не удалось.
    pub start: usize,
}

/// Длина в единицах UTF-16 (так считают лимиты Telegram).
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Делит `text` на части не длиннее [`MESSAGE_LIMIT`]. Короткий текст остаётся одной частью без изменений.
pub fn split_message(text: &str, markdown: bool) -> Vec<Part> {
    split_with_limit(text, markdown, MESSAGE_LIMIT)
}

fn split_with_limit(text: &str, markdown: bool, limit: usize) -> Vec<Part> {
    if utf16_len(text) <= limit {
        return vec![Part { text: text.to_string(), start: 0 }];
    }
    let fences = markdown && text.contains("```");
    let room = if fences { limit.saturating_sub(FENCE_ROOM) } else { limit };
    let mut parts: Vec<Part> = Vec::new();
    let mut start = 0usize;
    // Открывающая строка блока кода, оставшегося незакрытым в конце предыдущей части.
    let mut open: Option<String> = None;
    while start < text.len() {
        let rest = &text[start..];
        let prefix = open.as_deref().map(|o| format!("{o}\n")).unwrap_or_default();
        let avail = room.saturating_sub(utf16_len(&prefix)).max(1);
        let (end, next) = if utf16_len(rest) <= avail { (rest.len(), rest.len()) } else { cut_point(rest, avail) };
        let slice = rest[..end].trim_end();
        if !slice.trim().is_empty() || parts.is_empty() {
            let mut body = format!("{prefix}{slice}");
            if fences {
                open = open_fence(&body);
                if open.is_some() {
                    body.push_str("\n```");
                }
            }
            parts.push(Part { text: body, start });
        }
        start += next;
    }
    parts
}

/// Где резать `rest`, чтобы первая часть вошла в `avail` единиц UTF-16: `(конец части, начало следующей)`.
/// Пробелы и переводы строк на стыке не попадают ни в одну часть; отступ следующей строки сохраняется.
fn cut_point(rest: &str, avail: usize) -> (usize, usize) {
    let mut used = 0;
    let mut cap = rest.len();
    for (i, c) in rest.char_indices() {
        used += c.len_utf16();
        if used > avail {
            cap = i;
            break;
        }
    }
    let window = &rest[..cap];
    // Слишком рано резать не стоит: иначе получатся крошечные части.
    let min = window.char_indices().nth(window.chars().count() * 2 / 3).map_or(0, |(i, _)| i).min(window.len());
    let after_newlines = |from: usize| from + rest[from..].chars().take_while(|c| *c == '\n').map(char::len_utf8).sum::<usize>();
    let after_spaces = |from: usize| from + rest[from..].chars().take_while(|c| *c == ' ').map(char::len_utf8).sum::<usize>();
    // Чем крупнее граница, тем она предпочтительнее; в обоих первых вариантах подойдёт любая в последней трети окна.
    if let Some(i) = window.rfind("\n\n").filter(|i| *i >= min / 2) {
        return (i, after_newlines(i));
    }
    if let Some(i) = window.rfind('\n').filter(|i| *i >= min / 2) {
        return (i, i + 1);
    }
    let sentence = [". ", "! ", "? ", "… "].iter().filter_map(|p| window.rfind(p).map(|i| i + p.len() - 1)).filter(|end| *end > min).max();
    if let Some(end) = sentence {
        return (end, after_spaces(end));
    }
    if let Some(i) = window.rfind(' ').filter(|i| *i > min) {
        return (i, after_spaces(i));
    }
    // Ни границы, ни пробела (длинная ссылка, текст без пробелов): режем по окну.
    (cap.max(rest.chars().next().map_or(1, char::len_utf8)), cap.max(rest.chars().next().map_or(1, char::len_utf8)))
}

/// Если `text` кончается внутри блока кода, возвращает строку, которой этот блок открыт (с названием языка).
fn open_fence(text: &str) -> Option<String> {
    let mut open: Option<String> = None;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("```") {
            // Открывать блок заново достаточно короткой строкой («```rust»): иначе после очень длинной
            // строки с ограждением в каждую часть попал бы почти весь предел и по одному знаку текста.
            open = if open.is_some() { None } else { Some(t.trim_end().chars().take(FENCE_LINE_MAX).collect()) };
        }
    }
    open
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(parts: &[Part]) -> String {
        parts.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("")
    }

    #[test]
    fn short_text_is_one_untouched_part() {
        for t in ["", "привет", "  пробелы сохраняются  ", &"а".repeat(MESSAGE_LIMIT)] {
            let p = split_message(t, false);
            assert_eq!(p, vec![Part { text: t.to_string(), start: 0 }]);
        }
    }

    #[test]
    fn long_text_is_cut_at_paragraphs_then_lines_then_sentences_then_spaces() {
        // Абзацы по 40 символов, лимит 100: по два абзаца в часть.
        let para = "а".repeat(38);
        let text = vec![para.clone(); 6].join("\n\n");
        let parts = split_with_limit(&text, false, 100);
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert!(parts.iter().all(|p| p.text == format!("{para}\n\n{para}")), "{parts:?}");
        // Строки.
        let line = "б".repeat(30);
        let text = vec![line.clone(); 8].join("\n");
        let parts = split_with_limit(&text, false, 100);
        assert!(parts.iter().all(|p| utf16_len(&p.text) <= 100 && p.text.lines().all(|l| l == line)), "{parts:?}");
        // Предложения.
        let text = "Первое предложение тут. Второе предложение тут. Третье предложение тут. Четвёртое предложение тут.";
        let parts = split_with_limit(text, false, 60);
        assert!(parts.iter().all(|p| p.text.ends_with('.') && utf16_len(&p.text) <= 60), "{parts:?}");
        assert_eq!(parts.len(), 2);
        // Слова.
        let text = "слово ".repeat(40);
        let parts = split_with_limit(&text, false, 50);
        assert!(parts.iter().all(|p| p.text.split(' ').all(|w| w == "слово") && utf16_len(&p.text) <= 50), "{parts:?}");
    }

    #[test]
    fn nothing_is_lost_or_reordered() {
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!("Строка номер {i}, и ещё немного слов для длины.\n"));
            if i % 7 == 0 {
                text.push('\n');
            }
        }
        let parts = split_message(&text, false);
        assert!(parts.len() > 1);
        for p in &parts {
            assert!(utf16_len(&p.text) <= MESSAGE_LIMIT, "{}", utf16_len(&p.text));
            assert!(!p.text.is_empty());
        }
        // Все слова на месте и по порядку.
        let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(words(&text), words(&parts.iter().map(|p| p.text.clone()).collect::<Vec<_>>().join("\n")));
        // start каждой части указывает на её начало в исходном тексте.
        for p in &parts {
            assert!(text.is_char_boundary(p.start));
            assert!(text[p.start..].starts_with(p.text.lines().next().unwrap()), "{:?}", p);
        }
        assert!(parts.windows(2).all(|w| w[0].start < w[1].start));
    }

    #[test]
    fn text_without_any_boundary_is_cut_by_the_window_without_splitting_characters() {
        let text = "ж".repeat(10_000);
        let parts = split_message(&text, false);
        assert_eq!(parts.len(), 3);
        assert_eq!(joined(&parts), text);
        // Эмодзи занимают две единицы UTF-16: часть не должна выходить за лимит.
        let text = "😀".repeat(3000); // 6000 единиц UTF-16, хотя символов всего 3000
        let parts = split_message(&text, false);
        assert_eq!(parts.len(), 2);
        assert!(parts.iter().all(|p| utf16_len(&p.text) <= MESSAGE_LIMIT));
        assert_eq!(joined(&parts), text);
        // Длинная ссылка не режется на мелкие части, пока есть место.
        let url = format!("https://example.com/{}", "a".repeat(5000));
        let text = format!("посмотри {url} и скажи");
        let parts = split_message(&text, false);
        assert!(parts.iter().all(|p| utf16_len(&p.text) <= MESSAGE_LIMIT));
        assert_eq!(joined(&parts).replace(' ', ""), text.replace(' ', ""));
    }

    #[test]
    fn indentation_of_the_next_line_survives_a_cut() {
        let code = "    отступ\n".repeat(30);
        let parts = split_with_limit(&code, false, 100);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.text.lines().all(|l| l == "    отступ")), "{parts:?}");
    }

    #[test]
    fn code_blocks_are_closed_and_reopened_in_markdown() {
        let body = "let x = 1;\n".repeat(40);
        let text = format!("вот код\n```rust\n{body}```\nготово");
        let parts = split_with_limit(&text, true, 200);
        assert!(parts.len() >= 3, "{parts:?}");
        for p in &parts {
            assert!(utf16_len(&p.text) <= 200, "{}", utf16_len(&p.text));
            assert_eq!(p.text.matches("```").count() % 2, 0, "ограждения парные в каждой части: {p:?}");
        }
        assert!(parts[1].text.starts_with("```rust\n"), "{:?}", parts[1].text);
        assert!(parts.last().unwrap().text.ends_with("готово"));
        // Без режима Markdown ограждения — обычный текст и ничего не дописывается.
        let plain = split_with_limit(&text, false, 200);
        assert!(plain.iter().all(|p| !p.text.contains("```rust\n```")));
        assert_eq!(plain.iter().map(|p| p.text.matches("```").count()).sum::<usize>(), 2);
    }

    #[test]
    fn utf16_length_counts_astral_characters_twice() {
        assert_eq!(utf16_len("ab"), 2);
        assert_eq!(utf16_len("я"), 1);
        assert_eq!(utf16_len("😀"), 2);
    }

    #[test]
    fn whitespace_only_tails_do_not_make_empty_messages() {
        let text = format!("{}\n\n\n   \n", "а".repeat(MESSAGE_LIMIT));
        let parts = split_message(&text, false);
        assert!(parts.iter().all(|p| !p.text.trim().is_empty()));
        let text = format!("{}{}", "а".repeat(MESSAGE_LIMIT - 1), "\n".repeat(50));
        let parts = split_message(&text, false);
        assert_eq!(parts.len(), 1);
        assert!(parts.iter().all(|p| !p.text.trim().is_empty()));
    }

    #[test]
    fn a_maximal_input_makes_a_bounded_number_of_parts() {
        let line = "Обычная строка текста для проверки деления. ";
        let text = line.repeat(MAX_INPUT_CHARS / line.chars().count());
        let parts = split_message(&text, false);
        assert!(parts.len() <= MAX_PARTS, "{}", parts.len());
    }

    /// Случайные тексты (кириллица, эмодзи из двух единиц UTF-16, переводы строк, ограждения кода) и малые
    /// пределы: деление всегда заканчивается, части не длиннее предела, ничего не теряется и не
    /// переставляется, а `start` каждой части лежит на границе знака и растёт.
    #[test]
    fn random_texts_are_always_cut_safely() {
        let alphabet = [
            "а",
            "Я",
            "ё",
            "x",
            "Q",
            "7",
            " ",
            " ",
            "\n",
            "\n\n",
            ".",
            "! ",
            "? ",
            "`",
            "```",
            "```rust\n",
            "😀",
            "🧑\u{200d}💻",
            "—",
            "\t",
            "ab",
            "слово",
        ];
        let mut seed = 7u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as usize
        };
        for case in 0..800 {
            let n = 1 + next() % 400;
            let text: String = (0..n).map(|_| alphabet[next() % alphabet.len()]).collect();
            let limit = 40 + next() % 120;
            let markdown = next() % 2 == 0;
            let parts = split_with_limit(&text, markdown, limit);
            assert!(!parts.is_empty(), "случай {case}: нет частей");
            let mut last_start = None;
            for p in &parts {
                assert!(utf16_len(&p.text) <= limit, "случай {case}: часть длиннее предела {limit}: {:?}", p.text);
                assert!(text.is_char_boundary(p.start), "случай {case}: start внутри знака");
                assert!(last_start.is_none_or(|l| p.start > l), "случай {case}: start не растёт");
                last_start = Some(p.start);
            }
            if !markdown {
                let squeeze = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                assert_eq!(squeeze(&joined(&parts)), squeeze(&text), "случай {case}: текст потерян или переставлен");
            }
        }
    }

    #[test]
    fn a_huge_fence_line_does_not_turn_into_endless_parts() {
        let text = format!("```{}\n{}", "a".repeat(5000), "b".repeat(30_000));
        let parts = split_message(&text, true);
        assert!(parts.len() <= 20, "частей: {}", parts.len());
        assert!(parts.iter().all(|p| utf16_len(&p.text) <= MESSAGE_LIMIT));
        // Обычная строка с языком повторяется целиком.
        let code = format!("```rust\n{}", "let x = 1;\n".repeat(900));
        let parts = split_message(&code, true);
        assert!(parts.len() >= 2);
        assert!(parts[1].text.starts_with("```rust\n"));
    }
}
