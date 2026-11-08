# Changelog / История версий

## 1.4.0
- EN: the status line shows the connection (`● 42 ms` / `○ no connection`); messages sent without network wait in a queue (up to 30) and go out by themselves, in order, when the connection returns; marking several messages with `Space`, `Shift+↑/↓` or by dragging the mouse (the conversation scrolls by itself when the pointer is held at the top or bottom edge, `Ctrl`+click marks one) and forwarding, deleting or copying them all at once; polls are shown with percentages and `V` votes; `/topics` lists forum topics and sends your messages into the chosen one; `P` shows a photo in the console with colored half-blocks; `Ctrl+V` with a screenshot on the clipboard sends it as a photo; fixed: the exe was not attached to the Telegram channel post (the `@` in the channel name was read by curl as a file).
- RU: в строке состояния видна связь (`● 42 мс` / `○ нет связи`); сообщения, отправленные без сети, ждут в очереди (до 30) и уходят сами, по порядку, когда связь вернётся; отметка нескольких сообщений `Пробелом`, `Shift+↑/↓` или протягиванием мыши (переписка листается сама, когда указатель у верхнего или нижнего края, `Ctrl`+щелчок отмечает одно) и пересылка, удаление или копирование всех сразу; опросы показываются с процентами, `V` — голосовать; `/topics` показывает темы форума и отправляет ваши сообщения в выбранную; `P` показывает фото в консоли цветными полублоками; `Ctrl+V` со скриншотом в буфере обмена отправляет его как фото; исправлено: exe не прикреплялся к посту в Telegram-канале (`@` в имени канала curl читал как файл).

## 1.3.1
- EN: one new icon (a chat bubble with a command prompt) for the exe, the README and the repository; the notification is now a short soft click played through the system sound (with the old beep as a fallback); in Telegram → Settings → Devices the session is named `Tg-Lite v1.3.1 (Windows)` instead of `Windows 64-bit`, and the app version is the real one instead of the library's.
- RU: новая иконка (пузырь чата с командной строкой) для exe, README и репозитория; уведомление теперь короткий мягкий щелчок через системный звук (старый писк остаётся запасным вариантом); в Telegram → Настройки → Устройства сеанс называется `Tg-Lite v1.3.1 (Windows)` вместо `Windows 64-bit`, а версия приложения настоящая, а не версия библиотеки.

## 1.3.0
- EN: reactions under messages (short words such as `+1 3`, yours in brackets; `1`–`6` in `F7` set one, the same digit or `0` removes it); `G` in `F7` jumps to the original of a reply and `B` goes back; buttons of bot messages are shown and pressed from the `K` list (Enter or a digit; link buttons open in the browser); after three wrong passwords the lock screen asks you to wait 5, 10, 20 … seconds (up to 5 minutes); day separators say "Yesterday"; new setting "Theme" (dark, light, high contrast, Solarized).
- RU: реакции под сообщениями (короткие слова вроде `+1 3`, ваша в скобках; `1`–`6` в `F7` ставят реакцию, та же цифра или `0` снимает); `G` в `F7` переходит к оригиналу ответа, `B` возвращает; кнопки сообщений ботов показываются и нажимаются из списка `K` (Enter или цифра; кнопки-ссылки открываются в браузере); после трёх неверных паролей экран блокировки просит подождать 5, 10, 20 … секунд (до 5 минут); в разделителях дней есть «Вчера»; новая настройка «Тема» (тёмная, светлая, высокий контраст, Solarized).

## 1.2.0
- EN: read marks in private chats (`√` sent, `√√` read); `/pinned` lists all pinned messages; `/chat @name` opens a chat by public name (also people you have never written to); `/join` joins by invite link or public name; `/leave` leaves a group or channel after a repeated command; cloud drafts (unsent text is saved in Telegram when you switch chats or stop typing, drafts typed on the phone appear here; new setting "Cloud drafts", on by default; commands are never uploaded); view counts next to channel posts.
- RU: отметки прочтения в личных чатах (`√` отправлено, `√√` прочитано); `/pinned` — список всех закреплённых сообщений; `/chat @имя` открывает чат по публичному имени (в том числе с тем, кому вы не писали); `/join` — вступление по ссылке-приглашению или публичному имени; `/leave` — выход из группы или канала после повторной команды; облачные черновики (неотправленный текст сохраняется в Telegram при смене чата или паузе в наборе, черновики с телефона появляются здесь; новая настройка «Облачные черновики», по умолчанию включена; команды не отправляются); просмотры рядом с постами каналов.

## 1.1.0
- EN: mouse support (click a tab or a chat, the wheel scrolls the conversation or moves the selection over the chat list; new setting "Mouse", on by default); short demo GIFs in the README; README measurements now include memory after long heavy use (12.3 MB on 1.0.2).
- RU: поддержка мыши (клик по вкладке или чату, колесо прокручивает переписку или двигает выбор над списком чатов; новая настройка «Мышь», по умолчанию включена); короткие GIF-демо в README; в замерах README теперь есть память после долгой активной работы (12,3 МБ на 1.0.2).

## 1.0.2
- EN: all chats are loaded (up to 3000) instead of the first 120, the first page appears at once; older history on PgUp at the top of a chat (up to 1500 messages per chat); `/findmsg text` searches the current chat, `/findall text` searches all chats, Enter on a result opens it in its context (`/latest`, PgDn or Esc returns to the newest messages).
- RU: загружаются все чаты (до 3000) вместо первых 120, первая страница появляется сразу; старая история по PgUp в верху чата (до 1500 сообщений на чат); `/findmsg текст` ищет в текущем чате, `/findall текст` во всех чатах, Enter на результате открывает его в контексте (`/latest`, PgDn или Esc возвращают к последним сообщениям).

## 1.0.1
- EN: `/open` (O in F7) opens an attachment with the default Windows program (pictures, PDF, text, audio, video up to 50 MB; temporary `tglite-view` folder is emptied on lock, exit and start); `/copy` (C in F7) copies message text; input history on Alt+Up / Alt+Down (and Up / Down when the input has text); HTTP proxies with a login (`/proxy user:password@host:port`), the password is hidden on screen and in the log; more tests; CI: actions updated to Node 24 versions, dependency `pulldown-cmark` updated (security advisory RUSTSEC-2026-0433).
- RU: `/open` (O в F7) открывает вложение обычной программой Windows (картинки, PDF, текст, звук, видео до 50 МБ; временная папка `tglite-view` очищается при блокировке, выходе и запуске); `/copy` (C в F7) копирует текст сообщения; история ввода на Alt+↑ / Alt+↓ (и ↑ / ↓, когда в поле есть текст); HTTP-прокси с логином (`/proxy логин:пароль@адрес:порт`), пароль скрыт на экране и в журнале; больше тестов; CI: действия обновлены до версий на Node 24, обновлена зависимость `pulldown-cmark` (уведомление безопасности RUSTSEC-2026-0433).

## 1.0.0
- EN: first complete release: English and Russian interface, first-run wizard (language, keys, password, sign-in), `/faq`, `/paste` and clipboard paste, safer file names for saved attachments, safer vault saving (fsync, no lost writes), proxy credentials stripped from probes, ~140 tests, CI with lint/audit/build and release workflow with SHA256.
- RU: первый полный релиз: интерфейс на русском и английском, мастер первого запуска (язык, ключи, пароль, вход), `/faq`, `/paste` и вставка из буфера, безопасные имена сохраняемых файлов, более надёжная запись хранилища (fsync, без потерянных записей), логины прокси не попадают в проверки, около 140 тестов, CI (линтер, аудит, сборка) и выпуск релизов с SHA256.

## 0.0.9
- EN/RU: fixes and polish after 0.0.8 (see the release page / см. страницу релиза).

## 0.0.8
- EN/RU: `/faq` overview, new README with screenshots, measurements (see the release page / см. страницу релиза).

## Earlier / Ранее
- 0.0.1 – 0.0.7: first versions, see [Releases](https://github.com/MaksLeny/tg-lite/releases).
