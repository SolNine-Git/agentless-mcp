# Supply-chain audit: tree-sitter-language-pack

Phase 0 go/no-go note. Date: 2026-08-18. Verdict: **GO** for direct use of
`tree-sitter-language-pack` as the grammar source, under exact version pinning.

## Why this note exists

The pack fetches prebuilt native parser libraries at first use and `dlopen`s
them. Remote-fetched native code loaded into our process is a supply-chain
surface, and this tool is marketed on its security posture, so adoption was
gated on reading the download path rather than trusting the README. The
alternative if the audit had failed was our own pinned-manifest mirror or a
fallback to per-grammar PyPI packages.

## Method

Read `crates/ts-pack-core/src/download.rs` on `xberg-io/tree-sitter-language-pack`
`main` (the canonical upstream; **not** the unmaintained `tree-sitter-languages`).
Findings below are from that source, not from documentation.

## Findings

**Bundles are SHA-256 verified.** `load_or_download_bundle` computes
`sha256_hex` over the downloaded bundle and compares it against the
`bundle.sha256` field in the manifest. A mismatch raises `ChecksumMismatch` and
hard-fails; there is no "continue anyway" path. `validate_sha256_hex` guards the
digest strings themselves before they are used, so a hostile digest cannot
escape the cache directory via path traversal.

**Cache location and permissions.** Grammars land in
`~/.cache/tree-sitter-language-pack/v{version}/libs/`, created `0o700` on Unix.
The directory is overridable via `TREE_SITTER_LANGUAGE_PACK_CACHE_DIR`.

**Fetch granularity.** First use downloads ONE whole platform bundle
(`.tar.zst`), then extracts per-language libraries from it. It is not a
per-parser fetch: the first grammar request pays for the whole platform bundle,
which is exactly why fetching belongs in an explicit warmup step and not in the
middle of a tool call.

**Manifest.** The default manifest URL is
`https://github.com/xberg-io/tree-sitter-language-pack/releases/download/v{version}/parsers.json`,
overridable via `TREE_SITTER_LANGUAGE_PACK_MANIFEST_URL`. That override is our
mirror and air-gap path: point it at an internally hosted `parsers.json` and the
whole fetch stays inside the perimeter.

**Offline behavior.** There is no native offline mode. A cached manifest plus a
cached bundle serve without network access; a network failure with a cold cache
surfaces as an error rather than a silent degradation. Our warmup command design
covers this — the failure happens at install time, loudly, not mid-tool-call.

## Residual risk (accepted)

`parsers.json` is HTTPS-trusted but **not cryptographically signed**. Bundle
integrity is anchored to whatever digest that manifest carries, so an attacker
who could serve a substituted manifest for a given release tag could also serve
matching digests. The mitigation we accept is exact release-version pinning: the
pack version in `pyproject.toml` selects the release whose manifest we trust, and
that version moves only through a reviewed dependency bump. This is recorded in
the README security section rather than left implicit.

## Implications for the build

- **Warmup-only fetching.** Grammar downloads happen in an explicit `warmup`
  command run at install time, where a failure is visible and actionable. No
  tool call ever triggers a first fetch.
- **`AGENTLESS_MCP_NO_DOWNLOAD` air-gap gate.** When set, any attempted fetch is
  an error rather than a network call. Combined with
  `TREE_SITTER_LANGUAGE_PACK_CACHE_DIR` and
  `TREE_SITTER_LANGUAGE_PACK_MANIFEST_URL`, this gives a fully offline install
  path from a pre-seeded cache or an internal mirror.
- **Unit-pinning.** `tree-sitter` and `tree-sitter-language-pack` are pinned as
  one unit (`>=0.25,<0.27` and `==1.14.3`). The prebuilt grammars are compiled
  against a specific ABI — 16 of them, `cpp` and `csharp` among them, are ABI 15
  and fail silently below tree-sitter 0.25 — and the pack version is also what
  selects the trusted manifest. Bump both together or neither.
- **Per-language version stamps in `capabilities`.** The resolved grammar
  version for each loaded language is recorded in the cache and surfaced by the
  `capabilities` tool, so a grammar revision that shifts node types is
  attributable instead of showing up as unexplained extraction drift.
