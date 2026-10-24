# Rust Port: Grammar Pin (D1)

<!-- Copied from the D1 research report of 2026-10-10. A research agent wrote it; the main session
re-checked the py-tree-sitter version (0.26.0), the release asset parser-sources-1.14.3.tar.zst and its
.sha256 file, and the crates.io record of tree-sitter 0.26.9. The scratch scripts it names were not kept. -->

Measured 2026-10-10 on this machine. Scratch scripts are in `scratchpad/scripts/`. Nothing in the repository was changed.

## Summary

- All 22 revisions are reproducible. The manifest pins a full commit SHA in `rev` for each language. No language is pinned to a branch only.
- The upstream file at the manifest commit is not always the file that built the bundle. For 11 languages, the pack's CI regenerated `parser.c` with tree-sitter CLI 0.26.11 at `--abi 14`. Upstream commits an ABI 15 `parser.c` for those grammars. For go, javascript, python and php, the upstream file gives different trees on inputs that contain syntax errors (measured below).
- The exact sources are a release asset of the pack: `parser-sources-1.14.3.tar.zst` (sha256 `f13bcf0be1b9a99c45ead43671ae311346c2a10807c3fdc7c76407445ea7f2c7`). The same CI run built this tarball and the prebuilt bundles from one artifact.
- The bundle has a second input that is not a grammar file. The pack force-includes a wide-ctype shim (`ctype_shim.h`, backed by utf8proc 2.10) into every scanner. Without it, scanners classify non-ASCII characters through libc, and the trees differ (measured).
- The runtime is a third input. py-tree-sitter 0.26.0 bundles tree-sitter core v0.26.9. The Rust `tree-sitter` crate must be pinned to `=0.26.9`. With 0.26.13, 357 of 7,558 error-containing inputs gave different trees.
- **Recommendation: (b).** Vendor the 22 grammars' C sources from `parser-sources-1.14.3.tar.zst` (not from upstream at the manifest commit), plus `ctype_shim.h` and utf8proc from tag v1.14.3. Compile them with `cc` in our own build script and keep a sha256 manifest. Option (a) also works: I built and verified it with no network. It is the fallback, for the reasons in the Recommendation section.

## 1. Where the revisions come from

- Manifest: `sources/language_definitions.json` at tag v1.14.3. The same bytes (sha256 prefix `f9096c97e312c2b8`) are in `crates/ts-pack-core/language_definitions.json` at the tag, in the 1.14.3 crate, and in `sources/` inside `parser-sources-1.14.3.tar.zst`. Byte-compared with `cmp`.
  - https://raw.githubusercontent.com/xberg-io/tree-sitter-language-pack/v1.14.3/sources/language_definitions.json
- Tag v1.14.3 points to `df3bcc39862da6972032d7537d49b782a50a25bb` ("chore(swift): update Package.swift with checksum"). The release was built from its parent, `24efeb33eb6e82243e950a2ca7395efc3e3d47e3` ("chore(release): 1.14.3"). The 1.14.3 crate's `.cargo_vcs_info.json` names that parent. The two commits differ only in `Package.swift`.
  - `gh api repos/xberg-io/tree-sitter-language-pack/git/ref/tags/v1.14.3`
  - `gh api repos/xberg-io/tree-sitter-language-pack/compare/24efeb33...df3bcc39` gives ahead=1, files=Package.swift
- How a revision becomes `parser.c`. Source: https://raw.githubusercontent.com/xberg-io/tree-sitter-language-pack/v1.14.3/scripts/clone_vendors.py
  - `clone_repository` clones `branch` if one is given, then runs `git checkout <rev>`. `branch` therefore does not decide the revision.
  - If `generate: true`, the script runs `tree-sitter generate --abi <abi_version or 14>`. It skips this step when the committed `parser.c` is larger than 24 MiB (`ABI_EXEMPT_PARSER_BYTES`) and ships the committed file instead.
  - For `tsx`, `typescript` and `php`, `move_src_folder` rewrites `#include "../../common/scanner.h"` to `"../common/scanner.h"`. I diffed all three scanners: this one line is the only change.
- CI generator version: `npm install -g tree-sitter-cli@0.26.11` (publish.yaml, job `clone-vendors-shard`). The jobs `build-parser-sources` and `build-parser-binaries` both download the same `language-parsers` artifact.
  - https://raw.githubusercontent.com/xberg-io/tree-sitter-language-pack/v1.14.3/.github/workflows/publish.yaml
  - Caveat: CI restores `parsers/` from `actions/cache`, and the per-language cache key leaves out the CLI version. The workflow alone therefore does not prove which CLI generated each file. I settled this by measurement (next bullet).
- Regeneration check, measured: I ran tree-sitter CLI 0.26.11 (official release binary `tree-sitter-linux-x64.gz`) as `generate --abi 14 src/grammar.json` on each upstream `src/grammar.json` at the manifest rev. The result is byte-identical (git blob) to the tarball's `parser.c` for bash, c, go, javascript, python, rust, hcl, php, yaml and swift. sql was not tested: upstream commits no `src/grammar.json`, so regeneration needs node to evaluate `grammar.js`.

## 2. Is the bundle the same as parser-sources-1.14.3?

- The cached bundle libs are the release bundle. I extracted `parsers-linux-x86_64.tar.zst` (sha256 `935c0990...a77`, which matches `parsers.json` and the cached `manifest.json`). All 22 `libtree_sitter_<lang>.so` match the cache at `~/.cache/tree-sitter-language-pack/v1.14.3/libs` by sha256.
- Fingerprints: the ABI, node-kind count, field count and state count of each bundle `.so` equal the `#define`s in the tarball's `parser.c` for all 22 languages.
- Differential parse, Python. I compiled the tarball's sources the way `compile_parser_dynamic` in build.rs does (shim and utf8proc in scanner translation units; -O0 instead of -O2). I parsed 7,152 inputs (fixtures, repository sources, and seeded random mutations that add non-ASCII text and syntax errors). Trees match the bundle in 7,152 of 7,152 (kind, named, byte range, field, missing, error, has_error at every node). Script: `scripts/diff_parse.py`.
- Without the shim, trees differ: bash 2/318, javascript 1/312, hcl 23/306, kotlin 1/306, scala 8/306, swift 7/306 (measured inside Python, which sets LC_CTYPE from the environment; a Rust binary runs in the C locale, so expect at least as many differences). 17 of the 22 scanners call `isw*`/`tow*`.
- Upstream committed `parser.c` at the manifest rev (what a crates.io grammar crate ships) against the bundle, with the shim: go 66/336, javascript 45/312, python 79/660 and php 15/306 inputs differ. bash, c, rust, hcl and yaml did not differ on this corpus. All differing inputs contain errors. Clean files parse the same. Example: python `class if:\n  pass\n` gives `(class_definition name: (identifier) ...)` in the bundle (ABI 14, no reserved-word sets) and `(module (ERROR (pass_statement)))` from upstream's ABI 15 `parser.c`.

## 3. Per-language table

Bundle ABI comes from `grammars.get_language(n).abi_version` (command in the Commands section). "Source class": **committed** means the bundle uses upstream's committed parser.c (git blob equal at the rev). **regen** means CI regenerated parser.c at `--abi 14`, so it differs from upstream. **exempt** means `generate: true`, but the file is over 24 MiB, so the committed file is used.

| language | upstream repo | manifest revision (subdir) | source class | bundle ABI | exact-match crate and version | license (at rev) | evidence |
|---|---|---|---|---|---|---|---|
| bash | tree-sitter/tree-sitter-bash | a06c2e4415e9bc0346c6b86d401879ffb44058f7 (= tag v0.25.1) | regen | 14 | none. tree-sitter-bash 0.25.1 is built from this commit (vcs sha1 matches), but its parser.c (blob 4eeb1372, ABI 15) differs from the bundle source (221c402b). Its scanner also needs the shim. | MIT | [commit](https://github.com/tree-sitter/tree-sitter-bash/commit/a06c2e4415e9bc0346c6b86d401879ffb44058f7), [crate](https://static.crates.io/crates/tree-sitter-bash/tree-sitter-bash-0.25.1.crate) |
| c | tree-sitter/tree-sitter-c | b780e47fc780ddc8da13afa35a3f4ed5c157823d (= tag v0.24.2) | regen | 14 | none. tree-sitter-c 0.24.2 has a matching vcs sha1, but its parser.c (5032e3d0, ABI 15) is not the bundle source (cb89d2e7). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-c/commit/b780e47fc780ddc8da13afa35a3f4ed5c157823d), [crate](https://static.crates.io/crates/tree-sitter-c/tree-sitter-c-0.24.2.crate) |
| cpp | tree-sitter/tree-sitter-cpp | 8b5b49eb196bec7040441bee33b2c9a4838d6967 | exempt (25.9 MB) | 15 | none. Latest is 0.23.4 (2024-11-11), published before the rev date (2026-02-25). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-cpp/commit/8b5b49eb196bec7040441bee33b2c9a4838d6967), [versions](https://crates.io/api/v1/crates/tree-sitter-cpp/versions) |
| go | tree-sitter/tree-sitter-go | 2346a3ab1bb3857b48b29d779a1ef9799a248cd7 | regen | 14 | none. Latest is 0.25.0 (2025-08-29), before the rev date (2025-09-15). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-go/commit/2346a3ab1bb3857b48b29d779a1ef9799a248cd7), [versions](https://crates.io/api/v1/crates/tree-sitter-go/versions) |
| java | tree-sitter/tree-sitter-java | e10607b45ff745f5f876bfa3e94fbcc6b44bdc11 | committed | 14 | none. Latest is 0.23.5 (2024-12-21), before the rev date (2025-09-15). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-java/commit/e10607b45ff745f5f876bfa3e94fbcc6b44bdc11), [versions](https://crates.io/api/v1/crates/tree-sitter-java/versions) |
| javascript | tree-sitter/tree-sitter-javascript | 58404d8cf191d69f2674a8fd507bd5776f46cb11 | regen | 14 | none. Latest is 0.25.0 (2025-09-01), before the rev date (2025-09-15). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-javascript/commit/58404d8cf191d69f2674a8fd507bd5776f46cb11), [versions](https://crates.io/api/v1/crates/tree-sitter-javascript/versions) |
| lua | MunifTanjim/tree-sitter-lua (a GitHub fork of tree-sitter-grammars/tree-sitter-lua) | 4fbec840c34149b7d5fe10097c93a320ee4af053 | committed | 14 | none. Crate tree-sitter-lua is published from tree-sitter-grammars/tree-sitter-lua. Its versions after the rev date have vcs 816840c5... (0.4.1) and 10fe0054... (0.5.0). | MIT (LICENSE.md) | [commit](https://github.com/MunifTanjim/tree-sitter-lua/commit/4fbec840c34149b7d5fe10097c93a320ee4af053), [versions](https://crates.io/api/v1/crates/tree-sitter-lua/versions) |
| python | tree-sitter/tree-sitter-python | 26855eabccb19c6abf499fbc5b8dc7cc9ab8bc64 | regen | 14 | none. Latest is 0.25.0 (2025-09-11), before the rev date (2025-09-15). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-python/commit/26855eabccb19c6abf499fbc5b8dc7cc9ab8bc64), [versions](https://crates.io/api/v1/crates/tree-sitter-python/versions) |
| ruby | tree-sitter/tree-sitter-ruby | ad907a69da0c8a4f7a943a7fe012712208da6dee | committed | 14 | none. Latest is 0.23.1 (2024-11), before the rev date (2026-03-10). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-ruby/commit/ad907a69da0c8a4f7a943a7fe012712208da6dee), [versions](https://crates.io/api/v1/crates/tree-sitter-ruby/versions) |
| rust | tree-sitter/tree-sitter-rust | 77a3747266f4d621d0757825e6b11edcbf991ca5 (= tag v0.24.2) | regen | 14 | none. tree-sitter-rust 0.24.2 has vcs sha1 e2bee853694a1d3e0f6ef308fe3674542fec95d7, which GitHub does not find in the repo (HTTP 422). Its parser.c equals upstream's committed file at the rev (266a09ea), not the bundle source (f4f94197). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-rust/commit/77a3747266f4d621d0757825e6b11edcbf991ca5), [crate](https://static.crates.io/crates/tree-sitter-rust/tree-sitter-rust-0.24.2.crate) |
| tsx | tree-sitter/tree-sitter-typescript | 75b3874edb2dc714fb1fd77a32013d0f8699989f (`tsx`) | committed (scanner include path rewritten) | 14 | none. tree-sitter-typescript latest is 0.23.2 (2024-11-11), before the rev date (2025-01-30). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-typescript/commit/75b3874edb2dc714fb1fd77a32013d0f8699989f), [versions](https://crates.io/api/v1/crates/tree-sitter-typescript/versions) |
| typescript | tree-sitter/tree-sitter-typescript | 75b3874edb2dc714fb1fd77a32013d0f8699989f (`typescript`) | committed (scanner include path rewritten) | 14 | none (same crate as tsx) | MIT | same as tsx |
| csharp (file c_sharp) | tree-sitter/tree-sitter-c-sharp | 9150f7d56bb47f1a809fa23623f1ba1413e93fa9 | exempt (32.0 MB) | 15 | none. Latest is 0.23.5 (2026-04-14), before the rev date (2026-07-15). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-c-sharp/commit/9150f7d56bb47f1a809fa23623f1ba1413e93fa9), [versions](https://crates.io/api/v1/crates/tree-sitter-c-sharp/versions) |
| hcl | MichaHoffmann/tree-sitter-hcl (redirects to tree-sitter-grammars/tree-sitter-hcl) | 64ad62785d442eb4d45df3a1764962dafd5bc98b | regen | 14 | none. tree-sitter-hcl has one version, 1.1.0 (2025-05-16), before the rev date (2026-01-17). | Apache-2.0 | [commit](https://github.com/tree-sitter-grammars/tree-sitter-hcl/commit/64ad62785d442eb4d45df3a1764962dafd5bc98b), [versions](https://crates.io/api/v1/crates/tree-sitter-hcl/versions) |
| json | tree-sitter/tree-sitter-json | 001c28d7a29832b06b0e831ec77845553c89b56d | committed | 14 | none. Latest is 0.24.8 (2024-11), before the rev date (2025-09-15). | MIT | [commit](https://github.com/tree-sitter/tree-sitter-json/commit/001c28d7a29832b06b0e831ec77845553c89b56d), [versions](https://crates.io/api/v1/crates/tree-sitter-json/versions) |
| kotlin | fwcd/tree-sitter-kotlin | c8ac3d2627240160b999a2c100de3babbdb8f419 | committed | 14 | none. tree-sitter-kotlin (fwcd) latest is 0.3.8 (2024-08). tree-sitter-kotlin-ng (tree-sitter-grammars) and tree-sitter-kotlin-sg (ast-grep) come from other repositories and have no version after the rev date (2026-06-02). | MIT | [commit](https://github.com/fwcd/tree-sitter-kotlin/commit/c8ac3d2627240160b999a2c100de3babbdb8f419), [versions](https://crates.io/api/v1/crates/tree-sitter-kotlin/versions) |
| php | tree-sitter/tree-sitter-php | 3fda2fb9577166c6399834917f9844f30370beea (`php`) | regen (scanner include path rewritten) | 14 | none. tree-sitter-php 0.25.1 (2026-10-06) has vcs 58d5643086ae60e93a3746cca648363ee97c8761. | MIT | [commit](https://github.com/tree-sitter/tree-sitter-php/commit/3fda2fb9577166c6399834917f9844f30370beea), [crate](https://static.crates.io/crates/tree-sitter-php/tree-sitter-php-0.25.1.crate) |
| scala | tree-sitter/tree-sitter-scala | b4464dce74d84cd43ff1813cd8d85226812c968b | exempt (31.5 MB) | 15 | none. tree-sitter-scala 0.26.2 (2026-08-08) has vcs b931fcc338390925eb893d70ad070033f5856ccf. | MIT | [commit](https://github.com/tree-sitter/tree-sitter-scala/commit/b4464dce74d84cd43ff1813cd8d85226812c968b), [crate](https://static.crates.io/crates/tree-sitter-scala/tree-sitter-scala-0.26.2.crate) |
| sql | DerekStride/tree-sitter-sql | c2e1e08db1ea20dc23bdb8d228a81a8756e9c450 | regen (no committed parser.c upstream) | 14 | none. The crate for this repo is tree-sitter-sequel; its latest is 0.3.11 (2025-10-01), before the rev date (2026-03-02). Crate tree-sitter-sql is a different repo (m-novikov). | MIT | [commit](https://github.com/DerekStride/tree-sitter-sql/commit/c2e1e08db1ea20dc23bdb8d228a81a8756e9c450), [versions](https://crates.io/api/v1/crates/tree-sitter-sequel/versions) |
| swift | alex-pinkus/tree-sitter-swift | 28fe3a8a85586aa297524fe6164140b9521dcaff | regen (no committed parser.c upstream) | 14 | none. tree-sitter-swift 0.7.4 (2026-10-04) has vcs 171fa3bc343233fe07f5e17205aaada33d778825. | MIT | [commit](https://github.com/alex-pinkus/tree-sitter-swift/commit/28fe3a8a85586aa297524fe6164140b9521dcaff), [crate](https://static.crates.io/crates/tree-sitter-swift/tree-sitter-swift-0.7.4.crate) |
| toml | tree-sitter-grammars/tree-sitter-toml | 64b56832c2cffe41758f28e05c756a3a98d16f41 (= tag v0.7.0) | committed | 14 | **tree-sitter-toml-ng 0.7.0: exact.** vcs sha1 = rev. parser.c (1cfc62ba) and scanner.c (7fa3c23d) equal the bundle source. build.rs compiles `src/parser.c` and `src/scanner.c` with `cc` and has no network code. The scanner makes no `isw*` calls, so the shim does not apply. Crate tree-sitter-toml (Mathspy) is a different repo. | MIT | [commit](https://github.com/tree-sitter-grammars/tree-sitter-toml/commit/64b56832c2cffe41758f28e05c756a3a98d16f41), [crate](https://static.crates.io/crates/tree-sitter-toml-ng/tree-sitter-toml-ng-0.7.0.crate) |
| yaml | tree-sitter-grammars/tree-sitter-yaml | a1c4812a73ec5e089de8e441fdea3a921e8d5079 | regen | 14 | none. Latest is 0.7.2 (2025-10-07), before the rev date (2026-05-22). | MIT | [commit](https://github.com/tree-sitter-grammars/tree-sitter-yaml/commit/a1c4812a73ec5e089de8e441fdea3a921e8d5079), [versions](https://crates.io/api/v1/crates/tree-sitter-yaml/versions) |

Method notes for the table:

- Upstream-versus-bundle blob comparison: `gh api repos/<repo>/contents/<subdir/>src?ref=<rev>` blob SHAs against `git hash-object` on the tarball files (`scripts/upstream_cmp.py`).
- Crate check: for each candidate crate, every version with `created_at` on or after the rev's committer date was downloaded from `https://static.crates.io/crates/<c>/<c>-<v>.crate`. For each one I read `.cargo_vcs_info.json` and git-blob-hashed the packaged `parser.c` (`scripts/crate_vcs.py`). A version published before the rev's commit date cannot contain that commit.
- Licenses: `gh api repos/<repo>/license?ref=<rev>` (SPDX at the rev). This agrees with `sources/license_cache.json` at the tag in all 22 rows.
- utf8proc (needed by the shim) is MIT ("expat"), with the Unicode data notice in its LICENSE.md. Source: https://raw.githubusercontent.com/xberg-io/tree-sitter-language-pack/v1.14.3/crates/ts-pack-core/third_party/utf8proc/LICENSE.md

## 4. The tree-sitter-language-pack crate

- Versions near 1.14.3 on crates.io (https://crates.io/api/v1/crates/tree-sitter-language-pack/versions): 1.14.0 (2026-08-01), 1.14.1 (08-04), 1.14.2 (08-05), **1.14.3 (2026-08-05, MIT, 253,595 bytes, not yanked)**, 1.15.0 (08-13), then up to 1.21.3 (2026-10-10). Every version has features `config, default, download, dynamic-loading, serde, test-internals`, and none of them has a static-link feature.
- The 1.14.3 crate (downloaded from https://static.crates.io/crates/tree-sitter-language-pack/tree-sitter-language-pack-1.14.3.crate; its sha256 `1c8d90e1...875f` matches the index checksum) ships no grammar sources. It contains only `language_definitions.json`, `src/ctype_shim.h` and `third_party/utf8proc`. Its `build.rs` is byte-identical to the tag's.
- Static linking is selected by environment variables, not by features (`build.rs` `main`):
  - `TSLP_LINK_MODE=static` (the default is `dynamic`).
  - `TSLP_LANGUAGES=<list>`. When this is not set, the crate compiles nothing and relies on a runtime download.
  - Sources, in order: `PROJECT_ROOT/parsers` if it is populated; else `TSLP_OFFLINE=1` stops the build with "Parser sources not found" and a panic; else `scripts/clone_vendors.py`; else download `parser-sources-<ver>.tar.zst` with ureq, or read `TSLP_SOURCE_BUNDLE_URL` (which accepts `file://`).
- Unconditional build-dependencies: `cc, serde, serde_json, sha2, tar, ureq (platform-verifier), zstd`. They compile in every build, even when nothing is downloaded.
- **Verified offline static build:** scratch crate `rustcheck/` with `tree-sitter-language-pack = { version = "=1.14.3", default-features = false }` and `tree-sitter = "=0.26.9"`. `.cargo/config.toml` `[env]` sets `TSLP_LINK_MODE=static`, `TSLP_LANGUAGES=<22>`, `TSLP_OFFLINE=1`, and `PROJECT_ROOT = { value = "pack_root", relative = true }`, where `pack_root/parsers` holds the extracted tarball.
  - Build: `unshare -rn cargo build --offline` (no network namespace). It finished in 21 s (debug) with no warnings. The generated registry lists 22 static languages and 0 dynamic ones. The binary links only libc and libgcc_s.
  - Result: on 7,558 inputs the Rust trees equal the bundle's trees through py-tree-sitter, 7,558 of 7,558.
- Caveats for (a), observed in build.rs 1.14.3:
  - `apply_msvc_compat_patches` rewrites `crystal` and `sml` scanner.c files inside `PROJECT_ROOT/parsers` on every platform. This happened in my scratch tree. It does nothing if only our 22 grammars are vendored.
  - If `PROJECT_ROOT/sources/language_definitions.json` exists, build.rs copies it over the crate's own file in the cargo registry (`let _ = fs::copy(...)`). Avoid it by not vendoring `sources/`.
  - With `TSLP_OFFLINE` unset, a missing source tree falls through to a network download instead of failing.
  - The crate's `tree-sitter = "0.26"` resolves to 0.26.13 unless the workspace pins `=0.26.9`.
- Why docs.rs fails: docs.rs builds without network, and build.rs downloads the parser sources when they are absent. That matches the observed failures.

## 5. Recommendation: (b), vendor parser-sources-1.14.3 plus the shim

Vendor these files into the Rust workspace: `parsers/<lang>/src/{parser.c,scanner.c,tree_sitter/*.h}`, yaml's `schema.*.c`, and `parsers/{tsx,typescript,php}/common/` for the 22 languages, taken byte-for-byte from `parser-sources-1.14.3.tar.zst`. Add `ctype_shim.h` and `third_party/utf8proc/` from tag v1.14.3. Keep a sha256 manifest of each file. A CI test should recompute the manifest. The manifest should also record the tarball's sha256 and each language's repo and rev from the table.

Build script (cc), mirroring the pack:

- parser.c: compile as is (`-std=c11 -fno-strict-aliasing`).
- scanner.c: compile with `-DUTF8PROC_STATIC -include ctype_shim.h` and the utf8proc include dir.
- utf8proc.c: compile once.
- c_sharp symbol: `tree_sitter_c_sharp`.
- Collisions: some scanners export unprefixed globals: ruby `NON_IDENTIFIER_CHARS`, scala `token_name`, swift `DIRECTIVES`, `OPERATORS` and others. None of these collide among the 22 (measured with `nm`). Keep a link test, or apply the pack's prefix defines.

Pin `tree-sitter = "=0.26.9"`. It is the core that py-tree-sitter 0.26.0 bundles: submodule `tree_sitter/core` at v0.26.0 is 7f534862c3ec939c3a6ee147f7600ef5c1bf900f, which is tag v0.26.9, and the Rust crate 0.26.9's `.cargo_vcs_info.json` names the same commit. `uv.lock` pins py-tree-sitter 0.26.0.

Size: 112 files, 259,379,586 bytes uncompressed. A tar of them compresses to 12.8 MB with gzip -9 and 4.7 MB with zstd -19 (measured). Store them compressed and unpack them in build.rs if repository size matters.

Reasons for (b) over (a):

1. (a) needs the same vendored sources. The crate packages none, so (a) saves no vendoring.
2. (a) moves the pin into environment variables read by a third-party build script. If one is unset, the build changes mode silently: dynamic linking, or a ureq download when `TSLP_OFFLINE` is missing. (b) fails at compile time on a missing file.
3. (a) compiles ureq, rustls/ring, tar, zstd and sha2 into every offline build. All of them must be vendored for an offline build, and none of them is used.
4. (a)'s build.rs writes into the vendored tree and can write into the cargo registry (section 4).
5. (a) pulls in the pack's whole API (process, intel, chunking, registry). The port needs only 22 `Language` pointers.

Why not (c): all 22 revisions are reproducible. The tarball reproduces the bundle exactly (measured), and 10 of the 11 regenerated grammars also regenerate byte-for-byte with CLI 0.26.11.

Per-language path:

- All 22: (b) from the tarball.
- toml is the only language where a crates.io crate (tree-sitter-toml-ng 0.7.0) is an exact equivalent. Using it would mix two mechanisms for no gain, so vendor it too.
- No other language has a usable crate:
  - bash and c: crates built from the right commit, but they ship upstream's ABI 15 `parser.c`.
  - rust: the crate ships upstream's committed `parser.c`, not the regenerated one.
  - The other 18: no crate version was published from the manifest commit.
- Even with a matching `parser.c`, a grammar crate compiles its scanner without the shim. 17 of the 22 scanners call `isw*`/`tow*`.

To move the pin later, move the Python pin and the vendored tarball together. Regenerating from upstream is reproducible with tree-sitter CLI `<pack's pinned version> generate --abi 14 src/grammar.json` for every grammar that commits `src/grammar.json`. sql does not, and was not tested.

## Commands (reproduction)

- Bundle ABI and fingerprints:

  ```bash
  cd /home/dallasmarlow/Documents/Development/python/agentless-mcp && uv run python -c "from agentless_mcp.core import grammars; [print(n, grammars.get_language(n).abi_version) for n in ['bash','c','cpp','go','java','javascript','lua','python','ruby','rust','tsx','typescript','csharp','hcl','json','kotlin','php','scala','sql','swift','toml','yaml']]"
  ```

  Result: ABI 15 for cpp (semver 0.23.4), csharp (0.23.5) and scala (0.26.0); ABI 14 for the other 19.
- Release assets: `gh release download v1.14.3 -R xberg-io/tree-sitter-language-pack -p 'parser-sources-1.14.3.tar.zst*' -p parsers.json`
- Differential parse, Python: `.venv/bin/python -I scratchpad/scripts/diff_parse.py <bundle libs> <built shim> <built noshim> <corpus roots...>`
- Rust static build: `cd scratchpad/rustcheck && unshare -rn cargo build --offline`, then `target/debug/rustcheck rc_in rc_rs && diff -rq rc_py rc_rs` (0 differing files with tree-sitter =0.26.9; 357 with 0.26.13).
- Regeneration: `tree-sitter-linux-x64` v0.26.11, `generate --abi 14 src/grammar.json`, then `git hash-object src/parser.c`.

## Not confirmed

- sql regeneration from upstream: upstream commits no `src/grammar.json`, so it needs node and `grammar.js`. I did not run it. This does not affect (b): the tarball's sql `parser.c` reproduces the bundle.
- Platforms other than linux-x86_64: all parse comparisons ran against the linux-x86_64 bundle. The other platform bundles come from the same source artifact (publish.yaml), but I did not test them.
