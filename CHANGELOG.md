# Changelog / История версий

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
