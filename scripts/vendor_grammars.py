"""Vendor the grammar sources, the ctype shim, utf8proc and their licenses into one archive.

Development-time only. It needs network access and the zstd command. Run it from the repository
root:

    uv run --no-project python scripts/vendor_grammars.py [--asset parser-sources-1.14.3.tar.zst]

It writes crates/syntax/vendor/grammars-1.14.3.tar.zst and crates/syntax/vendor/MANIFEST.toml.
Set GITHUB_TOKEN to raise the GitHub API rate limit.
"""

import argparse
import base64
import hashlib
import http
import http.client
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import urllib.parse
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / "crates" / "syntax" / "vendor"
PACK = "xberg-io/tree-sitter-language-pack"
VERSION = "1.14.3"
TAG = f"v{VERSION}"
ASSET = f"parser-sources-{VERSION}.tar.zst"
ASSET_SHA256 = "f13bcf0be1b9a99c45ead43671ae311346c2a10807c3fdc7c76407445ea7f2c7"
ASSET_URL = f"https://github.com/{PACK}/releases/download/{TAG}/{ASSET}"
ARCHIVE = f"grammars-{VERSION}.tar.zst"
TIMEOUT = 120
REDIRECTS = 5
REDIRECT_CODES = frozenset({301, 302, 303, 307, 308})
LANGUAGES = (
    "bash",
    "c",
    "cpp",
    "csharp",
    "go",
    "hcl",
    "java",
    "javascript",
    "json",
    "kotlin",
    "lua",
    "php",
    "python",
    "ruby",
    "rust",
    "scala",
    "sql",
    "swift",
    "toml",
    "tsx",
    "typescript",
    "yaml",
)
# The tag can move, so each file fetched at it must match the git blob id that the tag held.
TAG_FILES = (
    ("src/ctype_shim.h", "shim/ctype_shim.h", "90426d84f038c8e4f2e73b400d4a3ded54e721dd"),
    (
        "third_party/utf8proc/LICENSE.md",
        "shim/utf8proc/LICENSE.md",
        "f18b1f3abdcb55e82ecdd84f476e7f366b00183b",
    ),
    (
        "third_party/utf8proc/utf8proc.c",
        "shim/utf8proc/utf8proc.c",
        "29e92ea3f1d0b45b8516894c50f7b67de2e7e7dc",
    ),
    (
        "third_party/utf8proc/utf8proc.h",
        "shim/utf8proc/utf8proc.h",
        "039da76909d8d139155743436a4c5e7c5d4171f5",
    ),
    (
        "third_party/utf8proc/utf8proc_data.c",
        "shim/utf8proc/utf8proc_data.c",
        "39b7b7778054e2b54bbc3c898fbe2c6066b4d156",
    ),
)
PACK_LICENSE = ("LICENSE", "66e173036ce9255a612e6f22d67d2108af6726af")
ABI = re.compile(rb"#define LANGUAGE_VERSION (\d+)")


@dataclass(frozen=True)
class Member:
    path: str
    data: bytes
    origin: str


@dataclass(frozen=True)
class Grammar:
    name: str
    symbol: str
    repo: str
    rev: str
    license: str
    abi: int


def main(argv: list[str] | None = None) -> int:
    """Write the archive and its manifest, and return the exit status."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--asset", type=Path, help=f"a local copy of {ASSET}")
    args = parser.parse_args(argv)
    asset = args.asset.read_bytes() if args.asset else fetch(ASSET_URL)
    if sha256(asset) != ASSET_SHA256:
        print(f"refused: {ASSET} does not have the pinned sha256", file=sys.stderr)
        return 1
    files = asset_files(asset)
    definitions = json.loads(files["sources/language_definitions.json"])
    licenses = json.loads(files["sources/license_cache.json"])
    grammars = [grammar(name, definitions[name], licenses, files) for name in LANGUAGES]
    members = grammar_members(files) + tag_members() + license_members(grammars)
    members.sort(key=lambda member: member.path)
    archive = compress(tar_bytes(members))
    VENDOR.mkdir(parents=True, exist_ok=True)
    (VENDOR / ARCHIVE).write_bytes(archive)
    (VENDOR / "MANIFEST.toml").write_text(manifest(archive, grammars, members), encoding="utf-8")
    print(f"wrote {VENDOR / ARCHIVE} ({len(archive)} bytes, {len(members)} files)")
    return 0


def fetch(url: str) -> bytes:
    for _ in range(REDIRECTS):
        parts = urllib.parse.urlsplit(url)
        if parts.scheme != "https":
            message = f"{url}: only https is fetched"
            raise ValueError(message)
        headers = {"User-Agent": "agentless-mcp-vendor-grammars"}
        token = os.environ.get("GITHUB_TOKEN")
        if token and parts.netloc == "api.github.com":
            headers["Authorization"] = f"Bearer {token}"
        connection = http.client.HTTPSConnection(parts.netloc, timeout=TIMEOUT)
        try:
            target = urllib.parse.urlunsplit(("", "", *parts[2:]))
            connection.request("GET", target, headers=headers)
            response = connection.getresponse()
            location = response.getheader("Location")
            data = response.read()
        finally:
            connection.close()
        if response.status in REDIRECT_CODES and location:
            url = urllib.parse.urljoin(url, location)
            continue
        if response.status != http.HTTPStatus.OK:
            message = f"{url}: HTTP {response.status}"
            raise ValueError(message)
        return data
    message = f"{url}: more than {REDIRECTS} redirects"
    raise ValueError(message)


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def git_blob(data: bytes) -> str:
    return hashlib.sha1(b"blob %d\0" % len(data) + data, usedforsecurity=False).hexdigest()


def wanted(name: str) -> bool:
    if name.startswith("sources/"):
        return name in ("sources/language_definitions.json", "sources/license_cache.json")
    top, language, *rest = [*name.split("/"), "", ""]
    if top != "parsers" or language not in LANGUAGES:
        return False
    return rest[0] == "src" or name.endswith("/common/scanner.h")


def asset_files(asset: bytes) -> dict[str, bytes]:
    tar = subprocess.run(
        ["zstd", "-dc"], input=asset, capture_output=True, check=True, timeout=TIMEOUT
    ).stdout
    files: dict[str, bytes] = {}
    with tarfile.open(fileobj=io.BytesIO(tar), mode="r:") as archive:
        for info in archive:
            if info.isfile() and wanted(info.name):
                extracted = archive.extractfile(info)
                if extracted is None:
                    message = f"{info.name}: no data"
                    raise ValueError(message)
                files[info.name] = extracted.read()
    return files


def grammar(
    name: str, definition: dict[str, str], licenses: dict[str, str], files: dict[str, bytes]
) -> Grammar:
    repo = definition["repo"]
    parser = files[f"parsers/{name}/src/parser.c"]
    found = ABI.search(parser)
    if found is None:
        message = f"{name}: parser.c has no LANGUAGE_VERSION"
        raise ValueError(message)
    # The pack names the symbol only when it differs from the name. A renamed key would otherwise
    # record the name, so the archived parser must define the symbol.
    symbol = definition.get("c_symbol", name)
    if f"tree_sitter_{symbol}(".encode() not in parser:
        message = f"{name}: parser.c defines no tree_sitter_{symbol}"
        raise ValueError(message)
    return Grammar(
        name=name,
        symbol=symbol,
        repo=repo,
        rev=definition["rev"],
        license=licenses[repo.removeprefix("https://github.com/")],
        abi=int(found.group(1)),
    )


def grammar_members(files: dict[str, bytes]) -> list[Member]:
    return [
        Member(path=name, data=data, origin=f"asset:{name}")
        for name, data in files.items()
        if name.startswith("parsers/")
    ]


def tag_members() -> list[Member]:
    base = f"https://raw.githubusercontent.com/{PACK}/{TAG}/crates/ts-pack-core"
    members = [pinned(f"{base}/{source}", path, blob) for source, path, blob in TAG_FILES]
    source, blob = PACK_LICENSE
    url = f"https://raw.githubusercontent.com/{PACK}/{TAG}/{source}"
    members.append(pinned(url, "licenses/tree-sitter-language-pack/LICENSE", blob))
    return members


def pinned(url: str, path: str, blob: str) -> Member:
    data = fetch(url)
    if git_blob(data) != blob:
        message = f"{url}: git blob {git_blob(data)} is not the pinned {blob}"
        raise ValueError(message)
    return Member(path=path, data=data, origin=url)


def license_members(grammars: list[Grammar]) -> list[Member]:
    return [license_member(grammar) for grammar in grammars]


def license_member(grammar: Grammar) -> Member:
    repo = grammar.repo.removeprefix("https://github.com/")
    url = f"https://api.github.com/repos/{repo}/license?ref={grammar.rev}"
    answer = json.loads(fetch(url))
    data = base64.b64decode(answer["content"])
    if git_blob(data) != answer["sha"]:
        message = f"{url}: the content does not match its git blob id"
        raise ValueError(message)
    origin = f"https://github.com/{repo}/blob/{grammar.rev}/{answer['path']}"
    return Member(path=f"licenses/{grammar.name}/{answer['path']}", data=data, origin=origin)


def tar_bytes(members: list[Member]) -> bytes:
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for member in members:
            info = tarfile.TarInfo(member.path)
            info.size = len(member.data)
            info.mode = 0o644
            info.mtime = 0
            archive.addfile(info, io.BytesIO(member.data))
    return buffer.getvalue()


def compress(tar: bytes) -> bytes:
    return subprocess.run(
        ["zstd", "-19", "-q", "-c"], input=tar, capture_output=True, check=True, timeout=600
    ).stdout


def manifest(archive: bytes, grammars: list[Grammar], members: list[Member]) -> str:
    lines = [
        "# Written by scripts/vendor_grammars.py. crates/syntax/tests/vendor.rs checks it.",
        f"pack = {json.dumps(PACK)}",
        f"version = {json.dumps(VERSION)}",
        f"asset = {json.dumps(ASSET)}",
        f"asset_sha256 = {json.dumps(ASSET_SHA256)}",
        f"archive = {json.dumps(ARCHIVE)}",
        f"archive_sha256 = {json.dumps(sha256(archive))}",
    ]
    for grammar in grammars:
        lines += [
            "",
            "[[language]]",
            f"name = {json.dumps(grammar.name)}",
            f"symbol = {json.dumps(grammar.symbol)}",
            f"repo = {json.dumps(grammar.repo)}",
            f"rev = {json.dumps(grammar.rev)}",
            f"license = {json.dumps(grammar.license)}",
            f"abi = {grammar.abi}",
        ]
    for member in members:
        lines += [
            "",
            "[[file]]",
            f"path = {json.dumps(member.path)}",
            f"size = {len(member.data)}",
            f"sha256 = {json.dumps(sha256(member.data))}",
            f"origin = {json.dumps(member.origin)}",
        ]
    return "\n".join(lines) + "\n"


if __name__ == "__main__":
    sys.exit(main())
