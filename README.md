<div align="center">

<img src="docs/logo.png" alt="Tg-Lite" width="128" height="128">

# Tg-Lite

<a href="https://github.com/MaksLeny/tg-lite/releases/latest"><img alt="release" src="https://img.shields.io/github/v/release/MaksLeny/tg-lite?style=flat-square&color=2AABEE"></a>
<a href="https://github.com/MaksLeny/tg-lite/releases"><img alt="downloads" src="https://img.shields.io/github/downloads/MaksLeny/tg-lite/total?style=flat-square&color=2AABEE"></a>
<a href="https://github.com/MaksLeny/tg-lite/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/MaksLeny/tg-lite/ci.yml?style=flat-square&label=CI"></a>
<img alt="platform" src="https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-555?style=flat-square">
<img alt="memory" src="https://img.shields.io/badge/RAM-3%E2%80%9312%20MB-brightgreen?style=flat-square">
<a href="LICENSE"><img alt="license" src="https://img.shields.io/badge/license-MIT-blue?style=flat-square"></a>

**A tiny Telegram client for the terminal. One ~3.8 MB exe, about 3 MB of RAM at start, up to about 12 MB after long heavy use.**

[Download](https://github.com/MaksLeny/tg-lite/releases/latest) • [Quick start](#quick-start) • [Security](SECURITY.md) • [Changelog](CHANGELOG.md) • [Report a bug](https://github.com/MaksLeny/tg-lite/issues/new/choose)

English · [Русский](#tg-lite-на-русском)

![Chat screen](docs/chat-en.png)

![Short demo](docs/demo-en.gif)

</div>

Tg-Lite is an unofficial Telegram client written in Rust for old and weak computers (it was built on a Pentium T4400
with 2 GB of RAM and a hard disk, where Telegram Desktop crawls). There is no Qt and no Electron: the interface is drawn
right in the console, and memory only holds the latest messages of the chats you have opened.

> Unofficial client. It speaks the open Telegram protocol (MTProto) through the
> [grammers](https://github.com/Lonami/grammers) library. You need your own `api_id` and `api_hash` from <https://my.telegram.org>.

## Contents
[Numbers](#numbers) · [Features](#features) · [Requirements](#requirements) · [Quick start](#quick-start) · [Keys and commands](#keys-and-commands) ·
[Security](#security) · [Proxy](#proxy) · [Build](#build) · [FAQ](#faq) · [Limits](#limits) · [Roadmap](#roadmap) · [License](#license)

## Numbers

Measured on a Lenovo B550 (Pentium T4400, 2 GB DDR2, HDD, Windows 10 LTSC):

| | Tg-Lite (measured) | Telegram Desktop |
|---|---|---|
| RAM, chat screen right after start | **2.8 MB** | 800 MB and more¹ |
| RAM after 10–15 minutes of use | **2.8 MB** | up to 2–2.5 GB after long use¹ |
| RAM after 10–15 minutes of browsing many chats and opening files (measured on 1.0.2) | **12.3 MB**, then stays flat | up to 2–2.5 GB after long use¹ |
| RAM on the lock screen with the animated background | **2.9 MB** | – |
| CPU | ~0 % on the chat screen, 1.4–2 % while the animated lock screen is shown | depends on version and number of chats |
| Start to chat list | about 1 second | – |
| Size | one exe, about **3.8–3.9 MB** (3934 KB on 0.0.8; the exact size is on the release page) | tens of MB installed plus cache |
| Installation | none, put the exe in any folder | installer |

How it was measured: Windows 10 LTSC, one account with the usual set of chats, Task Manager → Details, column "Memory (private working set)". The first rows are from 0.0.8 (start and 10–15 minutes of ordinary use); the 12.3 MB row is from 1.0.2 after 10–15 minutes of opening many different chats and files, and the figure did not grow further. Memory grows with the number of chats you have opened, because their latest messages are kept in memory (up to 1500 per chat). It is not a fully like-for-like comparison: Tg-Lite is text-only and does less than Telegram Desktop. Other columns also count shared system libraries and show more. To measure on your own PC: start `tg-lite.exe`, open a chat and run `powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Minutes 10` (it prints a ready table row). A test in the repository also checks that 100 chats with 60 messages each stay under 4 MB of heap (about 2.2 MB now).

¹ Telegram Desktop figures are user reports, not lab measurements: [tdesktop #30151](https://github.com/telegramdesktop/tdesktop/issues/30151)
("starts use RAM up to 2.0-2.5gb", "less than 800mb" after a restart). The real value depends a lot on how many chats
and media you have open, so measure it on your own computer. Tg-Lite shows text only and does not decode photos or video,
which is the main reason it is so small.

## Features

**Chats**
- Sign in by phone number and code (plus your cloud password if you have one).
- Tabs: All, New (unread), Users, Groups, Channels and your own Telegram folders.
- Online / last seen (within the other person's privacy settings), unread counters, the unread total in the window title.
- Live updates: new, edited and deleted messages; "typing…" and "recording voice…" in the chat header; `@` marks mentions.
- A separate draft per chat; if sending fails the text returns to the input box. Cloud drafts: unsent text is saved in Telegram (when you switch chats or stop typing for a few seconds), so it shows up on your phone, and a draft typed on the phone appears here. Can be turned off in the settings; commands starting with `/` are never uploaded.
- Channel posts show their view count next to the time (as loaded; it is not refreshed live).
- A pinned message is shown above the conversation.

**Messages**
- `F7` picks a message: reply with a quote (R), forward (F), edit (E), delete (D), copy the text (C), save the attachment (S), open it (O), links (L).
- `/edit` and `/del` act on your last message in the open chat (`/del` asks you to repeat it to confirm), `/note` (quick note to Saved Messages), `/later 30 text` (schedule in 30 minutes), `/scheduled` (list and cancel).
- Markdown (`F3` or `/md`): `**bold**`, `_italic_`, `` `code` ``, `[link](https://…)`.
- Reply quotes, forwarded-from notes and "(edited)" marks. Photos, voice and files appear as labels such as `[photo]`, `[file report.pdf, 2 MB]`.

**Files and links**
- `/send path [caption]` uploads a file up to 2000 MB (drag a file into the window to paste its path). jpg, jpeg and png images under 10 MB go as photos, everything else as a file.
- `/save` downloads an attachment to the `downloads` folder next to the exe, with progress in the status bar.
- `/open` (or `O` in `F7`) downloads an attachment to the temporary `tglite-view` folder and opens it with the default Windows program. Only pictures, PDF, plain text, JSON, audio and video up to 50 MB are opened this way (never programs, scripts or spreadsheets); the `tglite-view` folder is emptied on lock, on exit and on the next start.
- `/copy` (or `C` in `F7`) puts the text of a message on the clipboard.
- `/links` lists links of the chat, Enter opens one in the browser.

**Managing chats**
- `/findmsg text` searches messages in the open chat, `/findall text` in all chats (up to 40 results); `Enter` on a result opens it in its surroundings (`PgDn` or `Esc` goes back to the latest messages, also `/latest`). `PgUp` at the top of a conversation loads older messages (up to 1500 per chat; use search beyond that).
- Read marks in private chats: `√` next to your message means sent, `√√` means the other person has read it (not shown in groups, channels and Saved Messages).
- `/pinned` lists all pinned messages of the open chat (`Enter` opens one in its surroundings).
- `/chat @name` finds a person or a channel by its public name and opens the chat, even if you have never written to them. `/join t.me/+code` or `/join @name` joins a group or channel by an invite link or a public name; `/leave` leaves the open group or channel (repeat it to confirm; not available in private chats).
- `/find text` (`F6`, `Ctrl+F`) filters chats by name, `/pin` pins a chat, `/readall` marks everything read, `/export` saves a conversation to `export/<chat>.txt`.

**Comfort**
- Paste works: `Ctrl+V`, `Shift+Insert`, the terminal's own paste, or `/paste`.
- English and Russian interface (setting "Interface language", chosen at first start).
- Animated sign-in screen (always Aurora) and lock screen (Aurora, Matrix, Stars or none, your choice in settings).
- Mouse: click a tab or a chat, the wheel scrolls the conversation (over the chat list it moves the selection). Can be turned off in the settings. While the mouse is on, selecting text by dragging and right-click paste may not work in the console (in Windows Terminal hold `Shift` to select); use `/copy` and `Ctrl+V`, or turn the mouse off.
- Settings: auto-lock, proxy, sound, status, number of messages, accent color, self-destruct, lock screen background, widths, language, mouse, cloud drafts, password.
- `/faq` shows troubleshooting tips inside the program. `tglite.log` records errors and `tg-lite --debug` makes it detailed (never message text).
- Emoji, invisible characters and scripts other than Latin, Greek and Cyrillic are hidden: the Windows console cannot draw them and the screen would fall apart.

<table>
<tr>
<td><img src="docs/pick-en.png" alt="Message actions"></td>
<td><img src="docs/settings-en.png" alt="Settings"></td>
</tr>
<tr>
<td><img src="docs/lock-aurora.png" alt="Lock screen, Aurora"></td>
<td><img src="docs/faq-en.png" alt="Built-in FAQ"></td>
</tr>
</table>

## Requirements

| | |
|---|---|
| OS | Windows 10 / 11, 64-bit (tested on Windows 10 LTSC). Windows 7/8 are not tested and not promised |
| Terminal | Windows Terminal or cmd (Consolas font recommended) |
| Network | direct connection, or a SOCKS5 / HTTP proxy (see [Proxy](#proxy)) |
| Rust | only if you build it yourself |

## Quick start

1. Download `tg-lite.exe` from [Releases](../../releases) (or the CI artifact), and check it: `Get-FileHash tg-lite.exe -Algorithm SHA256` must match the SHA256 in the release description or in `tg-lite.exe.sha256` if it is attached (letter case does not matter).
2. Run it, best from Windows Terminal or cmd with the Consolas font.
3. The first-run wizard has four steps (the language step is skipped if a language is already chosen, the keys step if the keys are built into the exe):
   1. choose the language;
   2. enter `api_id` and `api_hash` (create an app at <https://my.telegram.org> → API development tools, then copy both values);
   3. choose an app password (any length, even one character);
   4. enter your phone number and the code from Telegram.
4. From then on only the password is asked.

`tglite.vault` is created next to the exe (change the folder with the `TGLITE_DIR` environment variable).
**A forgotten password cannot be recovered**: delete `tglite.vault` and sign in again.

## Keys and commands

Every action has both a key and a command, in case a key does not work in your console.

| Key | Command | Action |
|---|---|---|
| Enter | | send |
| Shift+Enter | | new line (pasted multi-line text is kept together) |
| Ctrl+V / Shift+Insert | `/paste` | paste from the clipboard |
| ↑ / ↓, Tab / Shift+Tab | | choose a chat, switch tabs (after a leading `/`, Tab completes the command) |
| Alt+↑ / Alt+↓ | | earlier messages and commands (↑ ↓ do this too when the input has text) |
| PgUp / PgDn | | scroll the conversation; PgUp at the top loads older messages |
| Mouse | | click a tab or a chat, wheel scrolls (see settings) |
| Esc | | clear the input; then cancel edit or reply; then return to the latest messages after a search jump; then reset the chat filter |
| F1 | `/help` | help |
| F2 | `/lock` | lock |
| F3 | `/md` | Markdown on/off |
| F4 | `/settings` | settings |
| F5 | `/refresh` | refresh chat list |
| F6 | `/find text` | search chats |
| | `/findmsg text` `/findall text` `/latest` | search messages in this chat or everywhere, back to the latest messages |
| | `/pinned` | all pinned messages of the open chat |
| | `/chat @name` | find a person or channel by name and open the chat |
| | `/join link-or-@name` `/leave` | join a group or channel; leave the open one (repeat to confirm) |
| F7 | | pick a message: R reply, F forward, E edit, D delete, C copy, S save, O open, L links |
| F10 | `/quit` | quit |
| | `/pin` `/readall` `/export` | pin chat, read everything, export |
| | `/edit` `/del` `/note` `/later` `/scheduled` | edit or delete your last message, note, schedule, scheduled list |
| | `/send` `/save` `/open` `/copy` `/links` | upload file, download attachment, open attachment, copy message, links |
| | `/passwd` `/proxy` `/faq` | change password, proxy, troubleshooting |

`Ctrl+L`, `Ctrl+Q`, `Ctrl+T`, `Ctrl+R`, `Ctrl+F` also work, in the Latin and Russian keyboard layouts. `Alt+Enter` and `Ctrl+J` also insert a new line (Windows may take Alt+Enter for full screen).

## Security

- `api_id`, `api_hash` and the Telegram session live in `tglite.vault`, encrypted with your password (Argon2id + ChaCha20-Poly1305). Without the password the file cannot be read and tampering is detected.
- The conversation is never written to disk. Exceptions are `/export` and `/save`, which you run yourself, and the short-lived files of `/open` (the `tglite-view` folder, emptied on lock, on exit and on start). The input history lives in memory only; drafts also live in memory here, and with cloud drafts on (default) the unsent text is stored by Telegram itself, like in any official app.
- Auto-lock after 1–60 minutes (or never) and on `F2`; the lock screen shows no messages.
- Optional self-destruct: overwrite and delete `tglite.vault` after 3, 5 or 10 wrong passwords on the lock screen (off by default). Other files stay, and you should end the session in Telegram → Settings → Devices.
- The log never contains message text. The chosen language is stored in `tglite.lang` (not secret).

**Security model in short**

- *What is stored locally?* `tglite.vault` (keys and Telegram session, encrypted) and `tglite.lang` (language) always. Besides those only what you ask for or what the program lists: `tglite.log` (log, passwords hidden), `downloads/`, `export/` and the temporary `tglite-view/`. Messages are not stored.
- *What if I delete `tglite.vault`?* The program forgets the keys and the session; sign in again. End the old session in Telegram → Settings → Devices.
- *Is a third-party exe safe?* Not by default: the exe is unsigned. Download it only from this repository's Releases, compare SHA256 (shown in the CI job summary), or build it yourself.
- *Was the code audited?* It was reviewed and tested by the author and by automated checks (clippy, `cargo audit`). This is not an independent professional security audit.

Details and how to report a vulnerability: [SECURITY.md](SECURITY.md) (English and Russian).

## Proxy

Nothing needs to be typed. In "auto" mode the order is: manually set address, environment variables
(`TGLITE_PROXY`, `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`), the Windows system proxy, then a direct connection. Plain HTTP proxies are handled by a
small local bridge the program starts itself. If nothing works it asks for a proxy address (SOCKS5 or HTTP; Enter retries, `0` means no proxy, `q` quits) and remembers it (for example
`127.0.0.1:10808`). Manual: `/proxy 127.0.0.1:10808` uses only that address; `/proxy auto` restores the order above; `/proxy off` goes direct (applied on the next start). A proxy with a login: `/proxy user:password@127.0.0.1:3128` (for HTTP proxies an `@` or `:` inside the password is written as `%40` or `%3A`; the address must have a port). The password is never shown on screen or written to the log.

## Build

Install Rust (<https://rustup.rs>), then `cargo build --release`; the exe appears in `target/release/tg-lite.exe`.
Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

Without installing Rust: push the project to GitHub. **CI** (`.github/workflows/ci.yml`) runs fmt, clippy and tests on Linux, `cargo audit`,
and builds the Windows exe (Actions → the run → Artifacts; the job summary shows the size and SHA256).
**Releases:** push a tag such as `git tag v1.0.0 && git push --tags`, and `release.yml` publishes `tg-lite.exe` with `tg-lite.exe.sha256`.

**API keys in the build.** In CI (`ci.yml`) the secrets `TGLITE_API_ID` and `TGLITE_API_HASH` bake your keys into the exe so they need not be
typed; the release workflow does not use them, so release exes never contain keys. Do not set these secrets in a public repository:
anyone can download the CI exe with your keys.

## FAQ

The same list is available in the program as `/faq`.

| Problem | What to do |
|---|---|
| Text will not paste | `Ctrl+V`, `Shift+Insert` or `/paste`; in the old Windows console right-click also works, but only with the mouse turned off in the settings |
| Boxes instead of letters | Use the Consolas font or Windows Terminal; the program hides emoji itself |
| No connection | Telegram may be blocked: use a VPN or `/proxy 127.0.0.1:port`, then restart |
| Antivirus complains | The exe is unsigned and built from open source; build it yourself or compare SHA256 with the release |
| Forgot the password | It cannot be recovered: delete `tglite.vault` and sign in again |
| No sound | `F4` → "Sound on new message": on; the beep goes through the sound card, check the volume |
| Anything else | Run `tg-lite --debug` and send `tglite.log` (no message text in it). Author: Telegram **@maks1m_salov** |

## Limits

- Text only: photos, voice, stickers and video are shown as labels (open them with `/open` or download with `/save`). Emoji are hidden.
- No secret chats and no calls.
- Up to 3000 chats are loaded (the first 100 appear at once, the rest follow); all tabs and folders are built from them. At most 1500 messages per chat are kept in memory.
- The proxy mode is applied on the next start; the sign-in screen always uses the "Aurora" background (settings are inside the encrypted vault).
- Self-destruct works on the lock screen, not at the startup password prompt.
- If Telegram is blocked where you live you need a VPN or a proxy.
- HTTP proxies with a login are supported as `/proxy user:password@host:port` (basic authorization only; the password is kept in the encrypted vault).
- Telegram is a trademark of its owners; this project is not affiliated with them.

## Roadmap

**1.2.0 (this version):** read marks in private chats, `/pinned`, `/chat @name`, `/join` and `/leave`, cloud drafts (synced with the phone), view counts under channel posts.

**1.1.0:** mouse (click a tab or a chat, wheel scrolling; can be turned off in the settings), a short demo GIF, measured memory use after long heavy use.

**1.0.2:** all chats instead of the first 120, older history on PgUp, message search (`/findmsg`, `/findall`) with the result opened in its context.

**1.0.1:** `/open` and `/copy`, input history on Alt+↑ / Alt+↓, HTTP proxies with a login.

**1.0.0** is the first complete release: clipboard paste, English and Russian interface, first-run wizard, built-in FAQ,
CI with checksummed releases, and a full code audit whose findings are covered by regression tests. Not planned: a graphical app with
photos, video and voice, tray mode, a reusable core crate.

## License

[MIT](LICENSE) © @maks1m_salov. Issues and ideas: Telegram **@maks1m_salov**.

---

<a id="tg-lite-на-русском"></a>

<div align="center">

<img src="docs/logo.png" alt="Tg-Lite" width="128" height="128">

# Tg-Lite на русском

<a href="https://github.com/MaksLeny/tg-lite/releases/latest"><img alt="release" src="https://img.shields.io/github/v/release/MaksLeny/tg-lite?style=flat-square&color=2AABEE"></a>
<a href="https://github.com/MaksLeny/tg-lite/releases"><img alt="downloads" src="https://img.shields.io/github/downloads/MaksLeny/tg-lite/total?style=flat-square&color=2AABEE"></a>
<a href="https://github.com/MaksLeny/tg-lite/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/MaksLeny/tg-lite/ci.yml?style=flat-square&label=CI"></a>
<img alt="platform" src="https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-555?style=flat-square">
<img alt="memory" src="https://img.shields.io/badge/RAM-3%E2%80%9312%20MB-brightgreen?style=flat-square">
<a href="LICENSE"><img alt="license" src="https://img.shields.io/badge/license-MIT-blue?style=flat-square"></a>

**Крошечный клиент Telegram для терминала. Один exe около 3,8 МБ, около 3 МБ памяти при запуске и до 12 МБ после долгой активной работы.**

[Скачать](https://github.com/MaksLeny/tg-lite/releases/latest) • [Быстрый старт](#быстрый-старт) • [Безопасность](SECURITY.md) • [История версий](CHANGELOG.md) • [Сообщить об ошибке](https://github.com/MaksLeny/tg-lite/issues/new/choose)

[English](#tg-lite) · Русский

![Экран чатов](docs/chat-ru.png)

![Короткое демо](docs/demo-ru.gif)

</div>

Tg-Lite — неофициальный клиент Telegram на Rust для старых и слабых компьютеров (он сделан на Pentium T4400 с 2 ГБ ОЗУ и
жёстким диском, где Telegram Desktop еле ворочается). Никакого Qt и Electron: интерфейс рисуется прямо в консоли, а в памяти
лежат только последние сообщения открытых вами чатов.

> Неофициальный клиент. Использует открытый протокол Telegram (MTProto) через библиотеку
> [grammers](https://github.com/Lonami/grammers). Нужны ваши собственные `api_id` и `api_hash` с <https://my.telegram.org>.

## Содержание
[Цифры](#цифры) · [Возможности](#возможности) · [Требования](#требования) · [Быстрый старт](#быстрый-старт) · [Клавиши и команды](#клавиши-и-команды) ·
[Безопасность](#безопасность) · [Прокси](#прокси) · [Сборка](#сборка) · [Если не работает](#если-не-работает) · [Ограничения](#ограничения) ·
[Планы](#планы) · [Лицензия](#лицензия)

## Цифры

Замеры на Lenovo B550 (Pentium T4400, 2 ГБ DDR2, жёсткий диск, Windows 10 LTSC):

| | Tg-Lite (замер) | Telegram Desktop |
|---|---|---|
| ОЗУ на экране чатов сразу после запуска | **2,8 МБ** | 800 МБ и больше¹ |
| ОЗУ через 10–15 минут работы | **2,8 МБ** | до 2–2,5 ГБ после долгой работы¹ |
| ОЗУ через 10–15 минут просмотра многих чатов и открытия файлов (замер на 1.0.2) | **12,3 МБ**, дальше не растёт | до 2–2,5 ГБ после долгой работы¹ |
| ОЗУ на экране блокировки с анимированным фоном | **2,9 МБ** | – |
| Процессор | ~0 % на экране чатов, 1,4–2 % пока показан анимированный экран блокировки | зависит от версии и числа чатов |
| От запуска до списка чатов | около секунды | – |
| Размер | один exe, около **3,8–3,9 МБ** (3934 КБ на 0.0.8; точный размер указан на странице релиза) | десятки МБ установки плюс кэш |
| Установка | не нужна, exe кладётся в любую папку | установщик |

Как мерили: Windows 10 LTSC, один аккаунт с обычным набором чатов, Диспетчер задач → «Подробности», столбец «Память (закрытый рабочий набор)». Первые строки — на 0.0.8 (запуск и 10–15 минут обычной работы); строка 12,3 МБ — на 1.0.2 после 10–15 минут открытия множества разных чатов и файлов, дальше значение не росло. Память растёт вместе с числом открытых чатов, потому что их последние сообщения хранятся в памяти (до 1500 на чат). Сравнение не полностью равноценное: Tg-Lite показывает только текст и умеет меньше, чем Telegram Desktop. Другие столбцы учитывают и общие системные библиотеки, поэтому показывают больше. Замерить у себя: запустите `tg-lite.exe`, откройте чат и выполните `powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Minutes 10` (скрипт выведет готовую строку для таблицы). Тест в репозитории следит, чтобы 100 чатов по 60 сообщений занимали меньше 4 МБ кучи (сейчас около 2,2 МБ).

¹ Данные по Telegram Desktop — отзывы пользователей, а не лабораторные замеры: [tdesktop #30151](https://github.com/telegramdesktop/tdesktop/issues/30151)
(«starts use RAM up to 2.0-2.5gb», после перезапуска «less than 800mb»). Реальный расход сильно зависит от числа чатов и
открытых медиа, поэтому проверьте у себя. Tg-Lite показывает только текст и не раскодирует фото и видео — отсюда такой малый размер.

## Возможности

**Чаты**
- Вход по номеру телефона и коду (и облачному паролю Telegram, если он включён).
- Вкладки: Все, Новые (с непрочитанными), Личные, Группы, Каналы и ваши папки из Telegram.
- «В сети» / «был(а) в сети» (в пределах приватности собеседника), счётчики непрочитанных, число в заголовке окна.
- Всё приходит само: новые, изменённые и удалённые сообщения; «печатает…» и «записывает голосовое…» в заголовке; значок `@` у упоминаний.
- Свой черновик у каждого чата; если отправка не удалась, текст возвращается в поле ввода. Облачные черновики: неотправленный текст сохраняется в Telegram (при смене чата или когда вы несколько секунд не печатаете) и виден на телефоне, а черновик с телефона появляется здесь. Выключается в настройках; команды, начинающиеся с `/`, не отправляются.
- У постов каналов рядом со временем показано число просмотров (на момент загрузки, вживую не обновляется).
- Закреплённое сообщение показывается над перепиской.

**Сообщения**
- `F7` — выбрать сообщение: ответить с цитатой (R), переслать (F), править (E), удалить (D), копировать текст (C), скачать вложение (S), открыть его (O), ссылки (L).
- `/edit` и `/del` работают с вашим последним сообщением в открытом чате (`/del` просит повторить для подтверждения), `/note` (быстрая заметка в «Избранное»), `/later 30 текст` (отложить на 30 минут), `/scheduled` (список и отмена).
- Markdown (`F3` или `/md`): `**жирный**`, `_курсив_`, `` `код` ``, `[ссылка](https://…)`.
- Цитаты ответов, пометки «переслано» и «(изм.)». Фото, голосовые и файлы показываются подписями: `[фото]`, `[файл отчёт.pdf, 2 МБ]`.

**Файлы и ссылки**
- `/send путь подпись` — отправить файл (подпись необязательна) до 2000 МБ (путь можно просто перетащить в окно). Картинки jpg, jpeg и png меньше 10 МБ идут как фото, остальное как файл.
- `/save` — скачать вложение в папку `downloads` рядом с exe, с прогрессом в строке состояния.
- `/open` (или `O` в `F7`) скачивает вложение во временную папку `tglite-view` и открывает обычной программой Windows. Так открываются только картинки, PDF, простой текст, JSON, звук и видео до 50 МБ (никогда программы, скрипты и таблицы); папка `tglite-view` очищается при блокировке, выходе и следующем запуске.
- `/copy` (или `C` в `F7`) кладёт текст сообщения в буфер обмена.
- `/links` — ссылки чата, Enter открывает ссылку в браузере.

**Управление чатами**
- `/findmsg текст` — поиск сообщений в открытом чате, `/findall текст` — во всех чатах (до 40 результатов); `Enter` на результате открывает его среди соседних сообщений (`PgDn` или `Esc` возвращает к последним, также `/latest`). `PgUp` у верха переписки подгружает старые сообщения (до 1500 на чат; дальше помогает поиск).
- Отметки прочтения в личных чатах: `√` у вашего сообщения — отправлено, `√√` — собеседник прочитал (в группах, каналах и «Избранном» не показываются).
- `/pinned` — список всех закреплённых сообщений открытого чата (`Enter` открывает сообщение среди соседних).
- `/chat @имя` находит человека или канал по публичному имени и открывает чат, даже если вы ему ещё не писали. `/join t.me/+код` или `/join @имя` — вступить в группу или канал по ссылке-приглашению или публичному имени; `/leave` — выйти из открытой группы или канала (повторите для подтверждения; в личных чатах недоступно).
- `/find текст` (`F6`, `Ctrl+F`) — поиск по названиям, `/pin` — закрепить, `/readall` — всё прочитано, `/export` — сохранить переписку в `export/<чат>.txt`.

**Удобство**
- Вставка работает: `Ctrl+V`, `Shift+Insert`, вставка самого терминала или команда `/paste`.
- Интерфейс на русском и английском (настройка «Язык интерфейса», выбирается при первом запуске).
- Анимированный экран входа (всегда «Аврора») и экран блокировки («Аврора», «Матрица», «Звёзды» или без анимации, выбирается в настройках).
- Мышь: клик по вкладке или чату, колесо прокручивает переписку (над списком чатов двигает выбор). Выключается в настройках. Пока мышь включена, выделение текста протягиванием и вставка правой кнопкой в консоли могут не работать (в Windows Terminal выделять можно с зажатым `Shift`); используйте `/copy` и `Ctrl+V` или выключите мышь.
- Настройки: автоблокировка, прокси, звук, статусы, число сообщений, цвет, самоуничтожение, фон блокировки, ширина, язык, мышь, облачные черновики, пароль.
- `/faq` — подсказки «Если не работает» прямо в программе. В `tglite.log` пишутся ошибки, а `tg-lite --debug` делает журнал подробным (тексты переписки не пишутся никогда).
- Эмодзи, невидимые символы и письменности кроме латиницы, греческого и кириллицы скрываются: консоль Windows не умеет их рисовать, и экран «рассыпался» бы.

<table>
<tr>
<td><img src="docs/pick-ru.png" alt="Действия с сообщением"></td>
<td><img src="docs/settings-ru.png" alt="Настройки"></td>
</tr>
<tr>
<td><img src="docs/lock-matrix.png" alt="Экран блокировки, Матрица"></td>
<td><img src="docs/faq-ru.png" alt="Подсказки в программе"></td>
</tr>
</table>

## Требования

| | |
|---|---|
| ОС | Windows 10 / 11, 64 бита (проверено на Windows 10 LTSC). Windows 7/8 не проверялись и не обещаются |
| Терминал | Windows Terminal или cmd (лучше шрифт Consolas) |
| Сеть | прямое соединение или прокси SOCKS5 / HTTP (см. [Прокси](#прокси)) |
| Rust | нужен только для самостоятельной сборки |

## Быстрый старт

1. Скачайте `tg-lite.exe` из [Releases](../../releases) (или артефакт CI) и проверьте: `Get-FileHash tg-lite.exe -Algorithm SHA256` должен совпасть с SHA256 из описания релиза или с `tg-lite.exe.sha256`, если он приложен (регистр букв не важен).
2. Запустите, лучше из Windows Terminal или cmd со шрифтом Consolas.
3. Мастер первого запуска — четыре шага (шаг языка пропускается, если язык уже выбран, шаг ключей — если ключи вшиты в exe):
   1. выбор языка;
   2. ввод `api_id` и `api_hash` (создайте приложение на <https://my.telegram.org> → API development tools и скопируйте оба значения);
   3. пароль приложения (любой длины, даже один символ);
   4. номер телефона и код из Telegram.
4. Дальше при каждом запуске спрашивается только пароль.

`tglite.vault` создаётся рядом с exe (папку можно сменить переменной окружения `TGLITE_DIR`).
**Забытый пароль восстановить нельзя**: удалите `tglite.vault` и войдите заново.

## Клавиши и команды

Каждое действие есть и клавишей, и командой — на случай, если клавиша не работает в вашей консоли.

| Клавиша | Команда | Действие |
|---|---|---|
| Enter | | отправить |
| Shift+Enter | | новая строка (вставленный многострочный текст не рвётся) |
| Ctrl+V / Shift+Insert | `/paste` | вставить из буфера обмена |
| ↑ / ↓, Tab / Shift+Tab | | выбор чата, вкладки (после начального `/` Tab дополняет команду) |
| Alt+↑ / Alt+↓ | | прошлые сообщения и команды (↑ ↓ делают то же, когда в поле есть текст) |
| PgUp / PgDn | | прокрутка переписки; PgUp у верха подгружает старые сообщения |
| Мышь | | клик по вкладке или чату, колесо — прокрутка (см. настройки) |
| Esc | | очистить ввод; затем отменить правку или ответ; затем вернуться к последним после перехода из поиска; затем сбросить фильтр чатов |
| F1 | `/help` | справка |
| F2 | `/lock` | заблокировать |
| F3 | `/md` | Markdown вкл/выкл |
| F4 | `/settings` | настройки |
| F5 | `/refresh` | обновить список чатов |
| F6 | `/find текст` | поиск по чатам |
| | `/findmsg текст` `/findall текст` `/latest` | поиск сообщений в этом чате или везде, назад к последним |
| | `/pinned` | все закреплённые сообщения открытого чата |
| | `/chat @имя` | найти человека или канал по имени и открыть чат |
| | `/join ссылка-или-@имя` `/leave` | вступить в группу или канал; выйти из открытого (повторите для подтверждения) |
| F7 | | выбор сообщения: R ответить, F переслать, E править, D удалить, C копировать, S скачать, O открыть, L ссылки |
| F10 | `/quit` | выход |
| | `/pin` `/readall` `/export` | закрепить чат, прочитать всё, сохранить переписку |
| | `/edit` `/del` `/note` `/later` `/scheduled` | править или удалить своё последнее сообщение, заметка, отложить, список отложенных |
| | `/send` `/save` `/open` `/copy` `/links` | отправить файл, скачать вложение, открыть вложение, копировать сообщение, ссылки |
| | `/passwd` `/proxy` `/faq` | сменить пароль, прокси, подсказки |

`Ctrl+L`, `Ctrl+Q`, `Ctrl+T`, `Ctrl+R`, `Ctrl+F` тоже работают, в латинской и русской раскладках. `Alt+Enter` и `Ctrl+J` тоже вставляют новую строку (Windows может забрать Alt+Enter для полноэкранного режима).

## Безопасность

- `api_id`, `api_hash` и сессия Telegram лежат в `tglite.vault`, зашифрованном вашим паролем (Argon2id + ChaCha20-Poly1305). Без пароля файл не прочитать, подмена данных обнаруживается.
- Переписка на диск не пишется. Исключения — `/export` и `/save`, которые вы запускаете сами, и недолгие файлы `/open` (папка `tglite-view`, очищается при блокировке, выходе и запуске). История ввода живёт только в памяти; черновики здесь тоже в памяти, а при включённых облачных черновиках (по умолчанию) неотправленный текст хранит сам Telegram, как в любом официальном приложении.
- Автоблокировка через 1–60 минут (или никогда) и по `F2`; на экране блокировки переписки нет.
- Самоуничтожение по желанию: затереть и удалить `tglite.vault` после 3, 5 или 10 неверных паролей на экране блокировки (по умолчанию выключено). Остальные файлы остаются; завершите сессию в Telegram → Настройки → Устройства.
- В журнале нет текстов сообщений. Выбранный язык хранится в `tglite.lang` (не секрет).

**Модель безопасности кратко**

- *Что хранится на диске?* Всегда `tglite.vault` (ключи и сессия Telegram, зашифрованы) и `tglite.lang` (язык). Кроме них только то, что вы запросили или что перечислено здесь: `tglite.log` (журнал, пароли скрыты), `downloads/`, `export/` и временная `tglite-view/`. Сообщения не сохраняются.
- *Что будет, если удалить `tglite.vault`?* Программа забудет ключи и сессию, войти нужно заново. Старую сессию завершите в Telegram → Настройки → Устройства.
- *Безопасен ли чужой exe?* По умолчанию нет: exe не подписан. Скачивайте только из Releases этого репозитория, сверяйте SHA256 (виден в сводке CI) или собирайте сами.
- *Был ли аудит кода?* Код проверен и протестирован автором и автоматикой (clippy, `cargo audit`). Это не независимый профессиональный аудит безопасности.

Подробности и как сообщить об уязвимости: [SECURITY.md](SECURITY.md) (на русском и английском).

## Прокси

Вводить ничего не нужно. Порядок в режиме «авто»: адрес, заданный вручную, переменные окружения
(`TGLITE_PROXY`, `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`), системный прокси Windows, затем напрямую. Для обычных HTTP-прокси программа сама поднимает
небольшой локальный мост. Если ничего не сработало, спросит адрес прокси (SOCKS5 или HTTP; Enter повторяет, `0` — без прокси, `q` — выход) и запомнит его (например `127.0.0.1:10808`).
Вручную: `/proxy 127.0.0.1:10808` использует только этот адрес, `/proxy auto` возвращает порядок выше, `/proxy off` — напрямую (применяется при следующем запуске). Прокси с логином: `/proxy логин:пароль@127.0.0.1:3128` (для HTTP-прокси `@` или `:` внутри пароля пишутся как `%40` и `%3A`; в адресе обязателен порт). Пароль не показывается на экране и не попадает в журнал.

## Сборка

Нужен Rust (<https://rustup.rs>): `cargo build --release`, файл появится в `target/release/tg-lite.exe`.
Проверки: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.

Без установки Rust: залейте проект на GitHub. **CI** (`.github/workflows/ci.yml`) гоняет fmt, clippy и тесты на Linux, `cargo audit`
и собирает exe под Windows (Actions → запуск → Artifacts; в сводке задания видны размер и SHA256).
**Релизы:** создайте тег, например `git tag v1.0.0 && git push --tags`, и `release.yml` опубликует `tg-lite.exe` вместе с `tg-lite.exe.sha256`.

**Ключи API в сборке.** В CI (`ci.yml`) секреты `TGLITE_API_ID` и `TGLITE_API_HASH` вшивают ваши ключи в exe, и вводить их не придётся;
workflow релиза их не использует, поэтому exe из релизов ключей не содержат. Не задавайте эти секреты в публичном репозитории:
любой сможет скачать exe из CI с вашими ключами.

## Если не работает

Этот же список показывает команда `/faq` в программе.

| Проблема | Что делать |
|---|---|
| Не вставляется текст | `Ctrl+V`, `Shift+Insert` или `/paste`; в старой консоли Windows работает и правая кнопка мыши, но только при выключенной в настройках мыши |
| Вместо букв квадраты | Шрифт Consolas или Windows Terminal; эмодзи программа скрывает сама |
| Нет подключения | Telegram может быть заблокирован: VPN или `/proxy 127.0.0.1:порт`, затем перезапуск |
| Антивирус ругается | exe не подписан и собран из открытого кода; соберите сами или сверьте SHA256 с релизом |
| Забыл пароль | Восстановить нельзя: удалите `tglite.vault` и войдите заново |
| Нет звука | `F4` → «Звук при новом сообщении»: включить; сигнал идёт через звуковую карту, проверьте громкость |
| Другое | `tg-lite --debug`, затем пришлите `tglite.log` (без текстов переписки). Автор: Telegram **@maks1m_salov** |

## Ограничения

- Только текст: фото, голосовые, стикеры и видео показываются подписями (откройте через `/open` или скачайте через `/save`). Эмодзи скрыты.
- Нет секретных чатов и звонков.
- Загружается до 3000 чатов (первые 100 появляются сразу, остальные следом); все вкладки и папки строятся из них. В памяти хранится не больше 1500 сообщений на чат.
- Режим прокси применяется при следующем запуске; экран входа всегда с фоном «Аврора» (настройки лежат внутри зашифрованного файла).
- Самоуничтожение работает на экране блокировки, но не при первом запросе пароля при запуске.
- Если Telegram у вас заблокирован, нужен VPN или прокси.
- HTTP-прокси с логином поддерживаются в виде `/proxy логин:пароль@адрес:порт` (только базовая авторизация; пароль хранится в зашифрованном хранилище).
- Telegram — торговая марка своих владельцев; проект с ними не связан.

## Планы

**1.2.0 (эта версия):** отметки прочтения в личных чатах, `/pinned`, `/chat @имя`, `/join` и `/leave`, облачные черновики (синхронизация с телефоном), просмотры под постами каналов.

**1.1.0:** мышь (клик по вкладке или чату, прокрутка колесом; выключается в настройках), короткое GIF-демо, замер памяти после долгой активной работы.

**1.0.2:** все чаты вместо первых 120, старая история по PgUp, поиск сообщений (`/findmsg`, `/findall`) с открытием найденного в контексте.

**1.0.1:** `/open` и `/copy`, история ввода на Alt+↑ / Alt+↓, HTTP-прокси с логином.

**1.0.0** — первый законченный выпуск: вставка из буфера, интерфейс на русском и английском, мастер первого запуска,
подсказки внутри программы, CI с релизами и контрольными суммами, полный аудит кода, все находки которого закрыты регрессионными
тестами. Не планируется: графическое приложение с фото, видео и голосовыми, режим в трее, общий core-крейт.

## Лицензия

[MIT](LICENSE) © @maks1m_salov. Вопросы и идеи: Telegram **@maks1m_salov**.
