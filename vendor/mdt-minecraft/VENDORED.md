# Vendored copy — local changes

Upstream: [Lavaver/Minecraft-Download-Toolkit](https://github.com/Lavaver/Minecraft-Download-Toolkit)
at `85f1b0d` (v0.7.0, 2026-11-05). License: MIT (see `LICENSE`), which is
compatible with the GPL-3.0-or-later this project ships under.

This copy is **metadata only**: it resolves a version and a loader profile and
writes a download list. It never transfers a game file, never launches the game
and never authenticates. Auth lives in `../minecraft-auth`; the bytes are moved
by aria2 (see the launcher's `aria2` module) or by whatever the front-end uses.

## Why an in-tree copy

Every other Rust Minecraft library hardcodes the Mojang endpoint constants. MDT
is the only one with declarative mirror providers (`providers.compile.toml`) plus
a runtime override file, and both are inside code that no upstream version
exposes as configuration. Vendoring beats wrapping here.

## The plan-only change set

Upstream downloads everything itself. This copy does not:

| Removed | Why |
|---|---|
| `downloader::execute_downloads*`, `DownloadBatch`, segmented Range downloads | aria2 does this, with more connections and per-file hashes |
| `asset::AssetRegistry` (`sync`, `fetch_and_sync`, registry seeding, `missing_locally`) | assets are entries in the list now |
| `virtual_dir` (hardlink / symlink / copy mounts) | it existed to feed the download cache |
| `registry` (home dir, registry root, mount/index helpers) | nothing is mounted or seeded; the plan writes lists only |
| `MdtClient::install_vanilla*`, `install_loader*`, `install_modloader`, `full_install`, `uninstall_modloader` | replaced by `plan_vanilla` / `plan_loader` |
| `installer::mcr` (post-uninstall manifest scan), `installer::uninstall` | uninstall needs the downloader to restore files |
| `jar` (JAR re-packing), `cli`, `tui`, `blocking`, `macros` | download-side conveniences; this crate has one consumer |
| `GeneratedVersionJson` / `GeneratedLibrary*`, aggregated manifest helpers | no caller builds a profile struct or fetches every manifest |
| `DownloadTask::{label, by}`, `MdtClientBuilder::{concurrency, asset_concurrency}` | no transfer happens here; aria2 owns progress and concurrency |
| `MdtError::{PartialDownloadFailed, UninstallFailed}` | nothing can produce them now |

Added:

- `plan::DownloadPlan` / `PlanEntry` — `<list_dir>/<id>.txt` (`列表/` by
  default, `MdtClientBuilder::list_dir` to move it) in the `aria2c --input-file`
  format (`dir=`, `out=`; one URL per file, no checksum).
- `DownloadPlan::verify` — the single verification step: missing, empty, wrong
  size or wrong SHA-1. The list stays a download list, not a proof.
- `DownloadPlan::create_dirs` — called by `write`. aria2 does not create `dir=`,
  and one missing directory fails the whole entry.
- `plan::fill_missing_hashes` — Fabric's profile names its own artifacts without
  a digest, so `<artifact>.sha1` is fetched (metadata) and the line becomes
  verifiable.
- `asset::plan_objects` — fetches the asset index (metadata, cached in
  `assets/indexes/<id>.json`, SHA-1 checked) and lists every object.
- `examples/plan.rs` — `cargo run --example plan -- <root> <mc> [loader]`.

## Local changes kept from earlier

- **HTTP/1.1 by default** (`http2` is opt-in). Measured on Minecraft assets:
  h2 put all streams on one TCP connection and lost to multiple h1.1 streams.
- **Asset index caching** in `$HOME/.mdt/registry/index/…`: the index is the same
  file every install of that version reads, so it is fetched once and re-verified
  by SHA-1 before use.
- **LWJGL is excluded from the plan.** The launcher ships its own Tessera
  Android runtime pack (LWJGL jars and natives, OpenAL, SDL, JNA), so Mojang's desktop
  LWJGL jars must not be downloaded. `build_tasks_from_version_json*` and
  `plan::entries_from_libraries` skip every `org.lwjgl*` coordinate.
- **Native classifier and extraction handling is gone.** There is no `natives`
  field, classifier selection, `extract_to`, `Extraction` or
  `DownloadPlan::extractions`; the plan only lists files and `verify` judges them.
- **Mirror-aware Maven and asset URLs** (`urls::ManifestUrls`), including
  `maven_exclude` host lists, so BMCLAPI clients never get linked to
  `libraries.minecraft.net`.
- **Security checks on remote metadata**: `security::is_safe_http_url`,
  `is_safe_path_component`, `is_sha1_hex` — a version JSON is remote input, and a
  task is admitted only when its hash, URL and Maven-relative path are all sane.
- **Version JSON written before any list**: the launcher reads it back to build a
  command line, and a plan that points at a missing profile is useless.

## Verifying

```sh
cd vendor/mdt-minecraft
cargo test                       # 17 unit tests + doc tests
cargo test --lib plan_ -- --ignored   # live: writes a 1.20.1 (and Fabric) list
cargo run --example plan -- /tmp/mdt 26.3 0.20.1
```

The two `--ignored` tests assert that a list is written, that every line carries
a hash, and that `libraries/` and `assets/objects/` contain **no files** — only
the directories aria2 needs.

## Updating

Re-apply the plan-only change set on top of the new upstream tag. The public
surface this project depends on is small:

```text
MdtClient::{builder, plan_vanilla, plan_loader, root_dir, mirror, downloader}
DownloadPlan::{entries, hashed, total_bytes, path, write, to_aria2}
Mirror, ModLoader, VersionType, MdtError, ProgressReporter, manifest::*
```

## Mobile front-end local changes

- `MdtClientBuilder::build` skips building its default reqwest client when a
  custom `Downloader` has been injected. The Android front-end injects
  `HttpDownloader::new(http::client())`; reqwest's default TLS path goes through
  `rustls-platform-verifier`, which panics on Android without a JNI context.
- MDT itself is plan-only, so it has no JAR-rewrite step. The mobile install
  ports the desktop `jar::normalize` to `src/jar.rs` and runs it after
  `DownloadPlan::verify` on every `.jar` in the plan (client, libraries, mods).
