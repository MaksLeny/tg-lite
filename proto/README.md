# Прототипы окна для Tg-Lite 2.0 / Window prototypes for Tg-Lite 2.0

Два одинаковых по задаче окна, чтобы выбрать фреймворк по цифрам на слабом ПК. Это не настоящий клиент: данные случайные.
Two equal-task windows to pick the GUI framework by measurement on a weak PC. Not a real client: random data.

- `slint/` — Slint, рисование на процессоре (без видеокарты). 5000 чатов, 200 сообщений.
- `iced/` — Iced, рисование на процессоре (tiny-skia). Список в нём не «ленивый», поэтому запускайте с `--chats 200`.

## Как замерить на ПК (Windows)
1. Скачайте `proto-slint.exe` и `proto-iced.exe` из Actions → Artifacts (запуск «Prototypes (v2)»).
2. Запустите окно. Для Iced: `proto-iced.exe --chats 200 --msgs 200`.
3. Минуту покрутите колесом список чатов слева и переписку справа.
4. В другом окне PowerShell: `powershell -ExecutionPolicy Bypass -File tools\measure.ps1 -Minutes 2 -Name proto-slint` (или `-Name proto-iced`).
5. Пришлите строку с результатом и скажите, насколько плавно крутится (да/нет, рывки, задержки).
