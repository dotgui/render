# Versioning

How the renderer is versioned, how that relates to the `.gui` spec, and what to
change when either one moves. Read this before bumping a version, implementing
a new spec version, or cutting a release.

## Two version numbers

| | Where it lives | Form | Example |
|---|---|---|---|
| **Spec version** | `<gui version="…">` in every document; decided in [dotgui/core](https://github.com/dotgui/core/blob/main/rfcs/README.md) | `MAJOR.MINOR` | `0.3` |
| **Renderer version** | `version` in the workspace [`Cargo.toml`](Cargo.toml) | `MAJOR.MINOR.PATCH` | `0.3.2` |

The spec is the contract. Its version tells a consumer what a document may
contain. The renderer is one consumer of that contract, and its version says
which contract it implements.

## The rule

> **The renderer's `MAJOR.MINOR` is the newest spec version it reads.
> Its `PATCH` changes only the renderer.**

- Renderer `0.3.0`, `0.3.1` and `0.3.2` all read documents up to `version="0.3"`.
  The later ones are better renderers of the same spec: fixes, speed, closer
  equivalence with kit, tooling.
- Renderer `0.4.0` is the first release that reads `version="0.4"` documents.
- The renderer never gets a `MINOR` bump on its own, and never gets a `PATCH`
  bump for implementing new spec vocabulary.

A patch in a document's version is ignored: `version="0.3.1"` reads as `0.3`.
The spec does not version documents that finely.

## Compatibility

Support is **cumulative**. A renderer reads every spec version up to its own,
and refuses anything newer.

| Renderer | Document declares | Result |
|---|---|---|
| `0.3.x` | `0.1`, `0.2`, or no version (read as `0.2`) | Renders exactly as it always did |
| `0.3.x` | `0.3` | Renders, with 0.3 rules enforced |
| `0.3.x` | `0.4`, `1.0`, … | **Refused**: "a newer renderer is needed" |
| `0.2.x` | `0.3` | Refused (from 0.3.0 onward; see below) |

**Why refuse instead of trying.** An older renderer has never heard of the newer
spec's features: slots, `library.guix`, whatever comes next. Drawing the file
anyway would silently render it wrong. Refusing, with both versions named, tells
the user exactly what to do: update the renderer. That is the version number's
whole job ([core RFC README](https://github.com/dotgui/core/blob/main/rfcs/README.md#format-versions),
[RFC-0042](https://github.com/dotgui/core/blob/spec-0.3-multipage-slots/rfcs/0042-multi-document-packages.md)).

**Old renderers cannot be fixed after the fact.** The version check exists from
renderer `0.3.0`. Builds before that never checked, so they will try to draw any
file they are handed.

**A document declaring a version the spec never had is refused too.** Several
producers have written `version="1.0"`, which no spec has been. Such a file is
mislabelled: the fix belongs in the tool that wrote it, not in the renderer.

## New rules apply only to new documents

When a spec version turns something that used to be tolerated into an error, the
error applies **only to documents declaring that version or higher**. A document
written earlier renders exactly as before, and the finding is reported on
`GuiDocument::warnings` instead.

0.3 examples: an unresolved `$token`, a `data:` URI, or breaking a slot rule is
an error in a `version="0.3"` document and a warning in a `version="0.2"` one.

In code, `read_parts` in [`parser.rs`](crates/renderer/src/parser.rs) computes
whether a document is held to 0.3 rules, and [`Issues`](crates/renderer/src/issues.rs)
routes each finding:

- `violation`: an error under the new rules, a warning otherwise
- `error`: always an error, for rules that only exist in new structure
- `advise`: always a warning

`Issues` holds a single on/off flag today. When 0.4 introduces rules of its own,
replace the flag with the document's parsed version, so each rule can check the
version that introduced it.

## Where the versions show

| | Value today |
|---|---|
| `RENDERER_VERSION` (Rust) | `0.3.0`, read from `Cargo.toml` at build time |
| `SUPPORTED_VERSION` (Rust) | `"0.3"`, in `parser.rs` |
| `render_png --version` | `dotgui renderer 0.3.0 (reads .gui up to version 0.3)` |
| WASM `renderer_version()` / `supported_spec_version()` | `"0.3.0"` / `"0.3"` |
| Error for a newer document | `the document declares version 0.4, but this renderer (0.3.0) reads documents up to version 0.3; a newer renderer is needed` |

The test `the_renderer_version_names_the_spec_it_implements` in `parser.rs`
fails if `Cargo.toml`'s `MAJOR.MINOR` and `SUPPORTED_VERSION` disagree. Treat a
failure as a question, not an obstacle: either the spec was implemented without
bumping the renderer, or the renderer was bumped without implementing the spec.

## Checklist: implementing a new spec version

Say core defines spec `0.4`.

1. **Find what 0.4 contains.** Every RFC in `core/rfcs/` with `targets: 0.4`,
   plus the spec changes in `core/spec/`. A renderer that claims a version
   implements all of it, not a subset (RFC-0042, *The version is the contract*).
2. **Refresh the vendored spec**, then regenerate coverage so new attributes
   show up as rows:
   ```bash
   cargo run -p dotgui-renderer --example refresh_spec
   ```
   ```bash
   UPDATE_COVERAGE=1 cargo test -p dotgui-renderer --test spec_coverage
   ```
3. **Do the work on a branch**, named after core's branch when there is one
   (this one was `spec-0.3-multipage-slots`). Leave the version alone while the
   work is partial.
4. **Gate new errors on the version** (see *New rules apply only to new
   documents*).
5. **Add fixtures that declare the new version.** Never edit the `version` of
   an existing fixture or example: the old ones are the proof that old files
   still render.
6. **Check that nothing old moved.** Existing layout snapshots and goldens
   should not change; if one does, that is a regression unless the spec says
   otherwise.
   ```bash
   cargo test --workspace
   ```
   ```bash
   cargo test -p dotgui-renderer --test golden_tests -- --ignored
   ```
7. **Bump both numbers in one commit**, once the work is complete:
   - `Cargo.toml`: `version = "0.4.0"`
   - `parser.rs`: `SUPPORTED_VERSION = "0.4"` and `SUPPORTED = (0, 4)`
8. **Update the docs.** The version table above, the README's feature list and
   Versions section, and `COVERAGE.md`.
9. **Release after core ships the version.** Core's RFC README marks each
   version `in progress` or `shipped`. The renderer can carry `0.4.0` on a
   branch while core is still in progress, but don't tag the release until 0.4
   is shipped there, because the contract can still change.

## Checklist: a renderer-only release

For fixes, performance, kit-equivalence work, tooling and docs that don't change
which spec is read:

1. Bump `PATCH` in `Cargo.toml`: `0.3.0` → `0.3.1`.
2. Leave `SUPPORTED_VERSION` alone.
3. Run the workspace tests and clippy with CI's toolchain:
   ```bash
   cargo +1.98 clippy --workspace --all-targets -- -D warnings
   ```

## After 1.0

Core applies semver to the spec from `1.0`: `MAJOR` = breaking, `MINOR` =
additive, `PATCH` = clarification only. The renderer's rule doesn't change: its
`MAJOR.MINOR` still names the newest spec version it reads. A spec `PATCH` is a
clarification, so it needs at most a renderer `PATCH`.

## Tags

Tag releases `vMAJOR.MINOR.PATCH` from `main`: `v0.3.0`, `v0.3.1`. The tag and
`Cargo.toml` must agree.
