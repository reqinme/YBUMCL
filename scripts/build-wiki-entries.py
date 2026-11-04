#!/usr/bin/env python3
"""从 PCL 的 WikiEntries.txt 生成「slug → MC 百科条目 id」表。

用法：
    python scripts/build-wiki-entries.py <WikiEntries.txt> \
        [crates/launcher-core/data/mcmod-wiki.json.gz]

格式（见 PCL 的 PCLCS/Resource/WikiEntry.cs）：
- 每行一个条目，行号（从 1 起，不含最后一行）就是 MC 百科的 class id；
- 一行里可以用 `¨` 放多个「平台 slug|中文名」，它们共用同一个 id；
- slug 写法：`@x` 只有 Modrinth；`x@` 两站同 slug；`cf@mr` 两站不同；`x` 只有 CurseForge；
- 最后一行是按 3 字符一组、86 进制编码的浏览量，这里用不上。

输出 {"c": {slug: id}, "m": {slug: id}, "e": [[cf, mr, 中文名, 浏览量], ...]}，
gzip 压缩，mtime 固定保证可复现。`e` 供中文搜索用（PCL 的 ResourceSearcher 就是拿
这三样做本地索引再换成英文关键词）。
"""

import gzip
import io
import json
import sys
from pathlib import Path

# PCL 的 ConvertRadix 用的 86 进制字符集
DIGITS = (
    "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
    "abcdefghijklmnopqrstuvwxyz/+=!?@#$%^&*()[]{}<>;:',"
)


# OpenCC 的繁→简字表，和运行时（zhsearch.rs）用同一份，保证两边转换结果一致
T2S_PATH = Path(__file__).resolve().parent.parent / "crates/launcher-core/data/t2s.txt"


def load_t2s() -> dict:
    table = {}
    with io.open(T2S_PATH, encoding="utf-8") as handle:
        for line in handle:
            line = line.rstrip("\n")
            if not line or "\t" not in line:
                continue
            trad, simp = line.split("\t", 1)
            if len(trad) == 1 and len(simp) == 1:
                table[trad] = simp
    return table


def radix86(text: str) -> int:
    value = 0
    for char in text:
        digit = DIGITS.find(char)
        if digit < 0:
            raise SystemExit(f"浏览量里有非法字符：{char!r}")
        value = value * 86 + digit
    return value


def english_name(slugs: str) -> str:
    first = slugs.split("@")[0] or slugs.strip("@")
    return " ".join(word.capitalize() for word in first.replace("-", " ").split())


def parse(text: str):
    lines = text.splitlines()
    while lines and not lines[-1].strip():
        lines.pop()
    if not lines:
        raise SystemExit("WikiEntries.txt 是空的")
    # 最后一行是每行一个条目的浏览量（3 字符一组、86 进制）
    tail = lines.pop()
    popularities = [radix86(tail[i : i + 3]) for i in range(0, len(tail), 3)]

    curseforge = {}
    modrinth = {}
    entries = []
    popularity_at = 0
    for index, line in enumerate(lines, start=1):
        if not line.strip():
            continue
        popularity = popularities[popularity_at] if popularity_at < len(popularities) else 0
        popularity_at += 1
        for entry in line.split("¨"):
            slugs = entry.split("|")[0].strip()
            if not slugs:
                continue
            if slugs.startswith("@"):
                pairs = [("m", slugs[1:])]
            elif slugs.endswith("@"):
                pairs = [("c", slugs[:-1]), ("m", slugs[:-1])]
            elif "@" in slugs:
                left, _, right = slugs.partition("@")
                pairs = [("c", left), ("m", right)]
            else:
                pairs = [("c", slugs)]
            resolved = {}
            for platform, slug in pairs:
                slug = slug.strip().lower()
                if not slug:
                    continue
                resolved[platform] = slug
                # 同一 slug 出现在多行时，后面的行浏览量更高，保留后者
                (modrinth if platform == "m" else curseforge)[slug] = index
            name = entry.split("|", 1)[1].strip() if "|" in entry else ""
            if "*" in name:
                name = name.replace("*", f" ({english_name(slugs)})")
            # 表里统一存简体，这样用户用繁体搜也能对上
            name = "".join(T2S.get(char, char) for char in name)
            entries.append([resolved.get("c"), resolved.get("m"), name, popularity])
    return {
        "c": dict(sorted(curseforge.items())),
        "m": dict(sorted(modrinth.items())),
        "e": entries,
    }


T2S = {}


def main(argv):
    if len(argv) < 2:
        raise SystemExit(__doc__)
    source = argv[1]
    target = argv[2] if len(argv) > 2 else "crates/launcher-core/data/mcmod-wiki.json.gz"
    global T2S
    T2S = load_t2s()
    with io.open(source, encoding="utf-8", newline="") as handle:
        table = parse(handle.read())
    payload = json.dumps(table, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    with open(target, "wb") as raw:
        # mtime=0：同样的输入永远得到同样的字节，方便进 git
        with gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as out:
            out.write(payload)
    print(
        f"{target}: curseforge {len(table['c'])} 个 slug，modrinth {len(table['m'])} 个 slug，"
        f"{len(table['e'])} 条带名字的条目，{len(payload)} 字节（压缩后 {len(gzip.compress(payload))} 字节）"
    )


if __name__ == "__main__":
    main(sys.argv)
