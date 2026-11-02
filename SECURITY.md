# Security

## How data is stored
- `api_id`, `api_hash` and the Telegram session live in `tglite.vault`, encrypted with your password
  (Argon2id for the key, ChaCha20-Poly1305 for the data). Without the password the file cannot be read, and tampering is detected.
- Conversations are never written to disk. Exceptions: `/export` and `/save`, which you run yourself.
- `tglite.log` never contains message text.
- Optional: overwrite and delete `tglite.vault` after N wrong passwords on the lock screen (off by default).
  Other files stay, and the Telegram session remains valid on the server: end it in Telegram → Settings → Devices.

## What the program does not protect
- A computer that already runs malware (keyloggers, memory readers).
- A forgotten password: it cannot be recovered.
- Do not bake your `api_id` / `api_hash` into an exe built from a public repository.

## Reporting a vulnerability
Please do not open a public issue. Write to the author in Telegram: **@maks1m_salov**, or use a private
report via the repository's Security tab. Include the version and steps to reproduce.

---

# Безопасность

## Как хранятся данные
- `api_id`, `api_hash` и сессия Telegram лежат в файле `tglite.vault`, зашифрованном вашим паролем
  (Argon2id для ключа, ChaCha20-Poly1305 для данных). Без пароля файл не читается, подмена данных обнаруживается.
- Переписка на диск не сохраняется. Исключения: `/export` и `/save`, которые вы запускаете сами.
- Журнал `tglite.log` не содержит текстов сообщений.
- Опционально: затереть и удалить `tglite.vault` после N неверных паролей на экране блокировки (по умолчанию выключено).
  Остальные файлы остаются, а сессия Telegram на сервере остаётся действительной: завершите её в Telegram → Настройки → Устройства.

## Чего программа не защищает
- Компьютер, на котором уже работает вредоносная программа (кейлоггер, чтение памяти).
- Забытый пароль: восстановить нельзя.
- Не вшивайте свои `api_id`/`api_hash` в exe, собранный из публичного репозитория.

## Как сообщить об уязвимости
Не публикуйте её в открытом Issue: напишите автору в Telegram **@maks1m_salov** или отправьте приватное
сообщение через вкладку Security репозитория. Укажите версию и шаги воспроизведения.
