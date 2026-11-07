#!/usr/bin/env python3
"""Делает из описания релиза GitHub пост для Telegram-канала.

Берётся только русская часть описания (после строки «Русский»), текст приводится к стилю канала:
строчные буквы, «—» вместо дефисов, без эмодзи и без разметки Markdown.

Использование: release_post.py имя_релиза.txt описание.md > пост.txt
"""
import re
import sys

LIMIT = 4000  # у Telegram предел 4096 знаков на сообщение

MARKER = re.compile(r"^\s*(?:#+\s*)?(?:\*\*)?\s*(?:русский|russian|ru)\s*(?:\*\*)?\s*:?\s*$", re.I)
EMOJI = re.compile("[\U0001F000-\U0001FAFF☀-➿⬀-⯿️‍]")
CYRILLIC = re.compile("[а-яё]", re.I)


def drop_foreign_tail(text: str) -> str:
    """Отбрасывает хвост после «---», если в нём нет русских букв (например, «Full changelog: …»)."""
    parts = re.split(r"\n\s*-{3,}\s*\n", "\n" + text + "\n")
    while len(parts) > 1 and not CYRILLIC.search(parts[-1]):
        parts.pop()
    return "\n---\n".join(parts).strip()


def russian_part(body: str) -> str:
    return drop_foreign_tail(find_russian(body))


def find_russian(body: str) -> str:
    lines = body.replace("\r\n", "\n").split("\n")
    for i, line in enumerate(lines):
        if MARKER.match(line):
            return "\n".join(lines[i + 1 :]).strip()
    # Нет заголовка «Русский»: берём то, что после последнего разделителя «---», иначе только строки с кириллицей.
    for i in range(len(lines) - 1, -1, -1):
        if re.fullmatch(r"\s*-{3,}\s*", lines[i]) and CYRILLIC.search("\n".join(lines[i + 1 :])):
            return "\n".join(lines[i + 1 :]).strip()
    return "\n".join(l for l in lines if not l.strip() or CYRILLIC.search(l)).strip()


def channel_style(text: str) -> str:
    text = EMOJI.sub("", text)
    text = re.sub(r"\[([^\]]+)\]\(([^)]+)\)", r"\1", text)  # [текст](ссылка) -> текст
    out = []
    for line in text.split("\n"):
        line = line.rstrip()
        line = re.sub(r"^\s*#+\s*", "", line)  # заголовки
        line = re.sub(r"^\s*[-*+]\s+", "— ", line)  # маркеры списка
        line = re.sub(r"^\s*\d+[.)]\s+", "— ", line)
        line = line.replace("**", "").replace("__", "").replace("`", "")
        line = re.sub(r"\s[-–]\s", " — ", line)
        line = re.sub(r"[ \t]{2,}", " ", line)
        if re.fullmatch(r"\s*[-=_*]{3,}\s*", line):
            line = ""
        out.append(line)
    text = "\n".join(out).lower()
    return re.sub(r"\n{3,}", "\n\n", text).strip()


def cut(text: str, limit: int = LIMIT) -> str:
    if len(text) <= limit:
        return text
    head = text[:limit]
    head = head[: head.rfind("\n")] if "\n" in head else head
    return head.rstrip() + "\n…"


def build(name: str, body: str) -> str:
    ru = channel_style(russian_part(body))
    if not ru:
        raise SystemExit("в описании релиза нет русской части: пост не создан")
    title = channel_style(name).replace("\n", " ").strip()
    return cut((title + "\n\n" + ru).strip() if title else ru)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    name = open(sys.argv[1], encoding="utf-8").read()
    body = open(sys.argv[2], encoding="utf-8").read()
    sys.stdout.write(build(name, body) + "\n")
