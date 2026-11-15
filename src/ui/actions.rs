//! Что можно сделать с выбранным сообщением (`F7`).
//!
//! Одна функция решает и подсказку в строке состояния, и то, что делают клавиши, поэтому подсказка
//! не предлагает ничего, что не сработает: «слушать» только у голосовых и музыки, «читать» только у
//! текстовых файлов, «открыть» только когда тип безопасен, файл не слишком большой и в системе есть
//! программа для него.

use crate::tg::{ChatMsg, MediaKind};
use crate::util;
use crate::{tr, trf};

/// Как назвать открытие вложения: от этого зависит подпись клавиши `O`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenVerb {
    Open,
    Listen,
    Watch,
}

/// Почему вложение не открывается системной программой.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenBlock {
    NoFile,
    /// Тип файла не из безопасного списка (программы, скрипты, архивы, документы Office).
    NotOpenable,
    TooBig,
    /// В системе нет программы для этого расширения.
    NoProgram(String),
}

impl OpenBlock {
    /// Объяснение для строки состояния.
    pub fn note(&self) -> String {
        match self {
            Self::NoFile => tr!("В этом сообщении нет файла, который можно открыть", "This message has no file to open").into(),
            Self::NotOpenable => tr!(
                "Файлы этого типа не открываются из чата (только картинки, PDF, текст, звук, видео): сохраните через S",
                "Files of this type are not opened from the chat (only images, PDF, text, audio, video): save with S"
            )
            .into(),
            Self::TooBig => tr!("Файл больше 50 МБ: сохраните его через S", "The file is larger than 50 MB: save it with S").into(),
            Self::NoProgram(ext) => trf!(
                "В Windows нет программы для файлов .{ext}: сохраните файл через S и откройте плеером или установите программу",
                "Windows has no program for .{ext} files: save the file with S and open it with a player, or install one"
            ),
        }
    }
}

/// Можно ли открыть вложение и каким словом это назвать. `has_handler` отвечает, есть ли в системе
/// программа для расширения (в тестах подставляется своя).
pub fn open_check(m: &ChatMsg, has_handler: &dyn Fn(&str) -> bool) -> Result<OpenVerb, OpenBlock> {
    let Some(media) = &m.media else { return Err(OpenBlock::NoFile) };
    if !util::is_openable(&media.name) {
        return Err(OpenBlock::NotOpenable);
    }
    if media.size.is_some_and(|s| s > util::MAX_OPEN_BYTES) {
        return Err(OpenBlock::TooBig);
    }
    let ext = util::file_ext(&media.name);
    if !has_handler(&ext) {
        return Err(OpenBlock::NoProgram(ext));
    }
    Ok(match media.kind {
        MediaKind::Voice | MediaKind::Audio => OpenVerb::Listen,
        MediaKind::Video | MediaKind::VideoNote | MediaKind::Gif => OpenVerb::Watch,
        MediaKind::Photo | MediaKind::Sticker | MediaKind::File => OpenVerb::Open,
    })
}

/// Действия, доступные для сообщения.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub reply: bool,
    pub edit: bool,
    pub delete: bool,
    pub copy: bool,
    pub save: bool,
    pub open: Option<OpenVerb>,
    pub photo: bool,
    pub read: bool,
    pub links: bool,
    pub vote: bool,
    pub buttons: bool,
    pub goto_reply: bool,
}

/// Правится ли сообщение: своё, не опрос, и либо обычный текст, либо вложение, у которого есть подпись
/// (у стикера, круглого видео, геопозиции, контакта и кубика подписи нет).
pub fn is_editable(m: &ChatMsg) -> bool {
    if !m.outgoing || m.poll.is_some() {
        return false;
    }
    match &m.media {
        Some(media) => !matches!(media.kind, MediaKind::Sticker | MediaKind::VideoNote),
        None => m.caption.is_none() && !m.raw.trim().is_empty(),
    }
}

/// Считает доступные действия. `can_write` — можно ли писать в чат.
pub fn actions_for(m: &ChatMsg, can_write: bool, has_handler: &dyn Fn(&str) -> bool) -> Actions {
    let media = m.media.as_ref();
    Actions {
        reply: can_write,
        edit: is_editable(m),
        delete: m.outgoing,
        copy: !m.raw.trim().is_empty(),
        save: media.is_some(),
        open: open_check(m, has_handler).ok(),
        photo: media.is_some_and(|x| x.kind == MediaKind::Photo),
        read: media.is_some_and(|x| util::is_text_file(&x.name) && x.size.is_none_or(|s| s <= util::MAX_TEXT_BYTES as u64)),
        links: !util::extract_links(&m.text).is_empty(),
        vote: m.poll.as_ref().is_some_and(|p| !p.closed),
        buttons: !m.buttons.is_empty(),
        goto_reply: m.reply_to.is_some(),
    }
}

/// Подсказка для строки состояния: только то, что работает. Редкое, зависящее от сообщения, стоит в начале,
/// потому что хвост подсказки обрезается первым.
pub fn pick_hint(a: &Actions, jump_back: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if a.goto_reply {
        parts.push(tr!("G к оригиналу", "G original"));
    }
    if jump_back {
        parts.push(tr!("B назад", "B back"));
    }
    if a.vote {
        parts.push(tr!("V голос", "V vote"));
    }
    if a.buttons {
        parts.push(tr!("K кнопки", "K buttons"));
    }
    if a.photo {
        parts.push(tr!("P фото", "P photo"));
    }
    if a.read {
        parts.push(tr!("T читать", "T read"));
    }
    match a.open {
        Some(OpenVerb::Listen) => parts.push(tr!("O слушать", "O listen")),
        Some(OpenVerb::Watch) => parts.push(tr!("O смотреть", "O watch")),
        Some(OpenVerb::Open) => parts.push(tr!("O открыть", "O open")),
        None => {}
    }
    if a.save {
        parts.push(tr!("S скачать", "S save"));
    }
    if a.links {
        parts.push(tr!("L ссылки", "L links"));
    }
    if a.reply {
        parts.push(tr!("R ответ", "R reply"));
    }
    parts.push(tr!("F переслать", "F forward"));
    if a.edit {
        parts.push(tr!("E правка", "E edit"));
    }
    if a.delete {
        parts.push(tr!("D удалить", "D delete"));
    }
    if a.copy {
        parts.push(tr!("C копия", "C copy"));
    }
    parts.push(tr!("1-6 реакция", "1-6 react"));
    parts.push("Esc");
    format!(" {}", parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tg::{MediaInfo, PollView};

    fn with_media(kind: MediaKind, name: &str, size: Option<u64>) -> ChatMsg {
        ChatMsg { media: Some(MediaInfo { kind, name: name.into(), size }), caption: Some(String::new()), ..Default::default() }
    }

    const ALL: &dyn Fn(&str) -> bool = &|_| true;
    const NONE: &dyn Fn(&str) -> bool = &|_| false;

    #[test]
    fn each_kind_of_file_gets_its_own_verb_and_nothing_extra() {
        let voice = with_media(MediaKind::Voice, "voice_1.ogg", Some(20_000));
        let a = actions_for(&voice, true, ALL);
        assert_eq!(a.open, Some(OpenVerb::Listen));
        assert!(a.save && !a.photo && !a.read && !a.copy && !a.links);
        let hint = pick_hint(&a, false);
        assert!(hint.contains("O слушать") || hint.contains("O listen"), "{hint}");
        assert_eq!(hint.matches("O ").count(), 1, "ровно один пункт про O: {hint}");

        assert_eq!(actions_for(&with_media(MediaKind::Video, "a.mp4", Some(1)), true, ALL).open, Some(OpenVerb::Watch));
        assert_eq!(actions_for(&with_media(MediaKind::VideoNote, "videonote_1.mp4", None), true, ALL).open, Some(OpenVerb::Watch));
        assert_eq!(actions_for(&with_media(MediaKind::Gif, "animation_1.mp4", None), true, ALL).open, Some(OpenVerb::Watch));
        assert_eq!(actions_for(&with_media(MediaKind::Audio, "песня.mp3", None), true, ALL).open, Some(OpenVerb::Listen));
        let photo = actions_for(&with_media(MediaKind::Photo, "photo_1.jpg", None), true, ALL);
        assert!(photo.photo && photo.save && photo.open == Some(OpenVerb::Open) && !photo.read);
        let doc = actions_for(&with_media(MediaKind::File, "отчёт.pdf", Some(1000)), true, ALL);
        assert_eq!(doc.open, Some(OpenVerb::Open));
        assert!(!doc.photo && !doc.read, "PDF не читается в окне");
    }

    #[test]
    fn text_files_can_be_read_but_only_small_ones() {
        let t = actions_for(&with_media(MediaKind::File, "notes.TXT", Some(5000)), true, ALL);
        assert!(t.read && t.open == Some(OpenVerb::Open));
        let big = actions_for(&with_media(MediaKind::File, "log.txt", Some(util::MAX_TEXT_BYTES as u64 + 1)), true, ALL);
        assert!(!big.read && big.open.is_some(), "читать в окне нельзя, но системной программой можно");
        assert!(actions_for(&with_media(MediaKind::File, "data.json", None), true, ALL).read, "размер неизвестен: пробуем");
        // xml, yml, toml читаются в окне, но системной программой не открываются (не из безопасного списка)
        let x = actions_for(&with_media(MediaKind::File, "config.toml", Some(10)), true, ALL);
        assert!(x.read && x.open.is_none() && x.save);
    }

    #[test]
    fn risky_missing_or_oversized_files_offer_only_save() {
        for name in ["setup.exe", "run.bat", "archive.zip", "report.docx", "tgs.tgs", "noext"] {
            let a = actions_for(&with_media(MediaKind::File, name, Some(1)), true, ALL);
            assert!(a.save && a.open.is_none() && !a.read && !a.photo, "{name}: {a:?}");
            assert_eq!(open_check(&with_media(MediaKind::File, name, Some(1)), ALL), Err(OpenBlock::NotOpenable), "{name}");
        }
        let huge = with_media(MediaKind::Video, "movie.mp4", Some(util::MAX_OPEN_BYTES + 1));
        assert_eq!(open_check(&huge, ALL), Err(OpenBlock::TooBig));
        assert!(actions_for(&huge, true, ALL).save);
    }

    #[test]
    fn open_is_not_offered_when_windows_has_no_program_for_it() {
        let voice = with_media(MediaKind::Voice, "voice_1.ogg", None);
        assert_eq!(open_check(&voice, NONE), Err(OpenBlock::NoProgram("ogg".into())));
        let a = actions_for(&voice, true, NONE);
        assert!(a.open.is_none() && a.save, "скачать можно, открыть нечем");
        let hint = pick_hint(&a, false);
        assert!(!hint.contains("O слуш") && !hint.contains("O listen") && !hint.contains("O откр"), "{hint}");
        assert!(OpenBlock::NoProgram("ogg".into()).note().contains("ogg"));
        // без вопроса к системе мы спрашиваем по расширению в нижнем регистре
        let seen = std::cell::RefCell::new(Vec::new());
        let spy = |e: &str| {
            seen.borrow_mut().push(e.to_string());
            true
        };
        let _ = open_check(&with_media(MediaKind::File, "Photo.JPG", None), &spy);
        assert_eq!(*seen.borrow(), vec!["jpg".to_string()]);
    }

    #[test]
    fn plain_and_special_messages_get_only_fitting_actions() {
        let mut m = ChatMsg {
            raw: "привет https://example.com".into(), text: "привет https://example.com".into(), ..Default::default()
        };
        let a = actions_for(&m, true, ALL);
        assert!(a.copy && a.links && a.reply && !a.save && a.open.is_none() && !a.edit && !a.delete);
        m.outgoing = true;
        let a = actions_for(&m, true, ALL);
        assert!(a.edit && a.delete);
        // в канале без права писать нет «ответить»
        assert!(!actions_for(&m, false, ALL).reply);
        // опрос: голосовать можно, пока он открыт; править нельзя
        let mut poll = ChatMsg { outgoing: true, caption: Some(String::new()), poll: Some(PollView::default()), ..Default::default() };
        let a = actions_for(&poll, true, ALL);
        assert!(a.vote && !a.edit && !a.save && !a.copy);
        poll.poll.as_mut().unwrap().closed = true;
        assert!(!actions_for(&poll, true, ALL).vote);
        // стикер и круглое видео подписи не имеют: править нельзя
        let mut st = with_media(MediaKind::Sticker, "sticker_1.webp", Some(1));
        st.outgoing = true;
        assert!(!actions_for(&st, true, ALL).edit);
        let mut photo = with_media(MediaKind::Photo, "photo_1.jpg", None);
        photo.outgoing = true;
        photo.raw = "подпись".into();
        assert!(actions_for(&photo, true, ALL).edit, "подпись у фото править можно");
        // геопозиция и контакт: подпись есть, а скачивать и править нечего
        let geo = ChatMsg { outgoing: true, caption: Some(String::new()), text: "[геопозиция]".into(), ..Default::default() };
        let a = actions_for(&geo, true, ALL);
        assert!(!a.save && !a.edit && a.open.is_none());
        // ответ и кнопки
        let r = ChatMsg { reply_to: Some(5), buttons: Vec::new(), ..Default::default() };
        assert!(actions_for(&r, true, ALL).goto_reply);
    }

    #[test]
    fn the_hint_lists_each_key_at_most_once_and_always_ends_with_esc() {
        let mut every = Actions {
            reply: true,
            edit: true,
            delete: true,
            copy: true,
            save: true,
            open: Some(OpenVerb::Open),
            photo: true,
            read: true,
            links: true,
            vote: true,
            buttons: true,
            goto_reply: true,
        };
        for jump in [false, true] {
            let hint = pick_hint(&every, jump);
            assert!(hint.ends_with("Esc"), "{hint}");
            for key in ["G ", "B ", "V ", "K ", "P ", "T ", "O ", "S ", "L ", "R ", "F ", "E ", "D ", "C ", "1-6 "] {
                assert!(hint.matches(&format!(" {key}")).count() <= 1, "{key} повторяется: {hint}");
            }
        }
        every.open = None;
        assert!(!pick_hint(&every, false).contains("O "));
        let bare = pick_hint(&Actions::default(), false);
        assert!(bare.contains('F') && bare.contains("1-6") && !bare.contains("S "), "{bare}");
    }
}
