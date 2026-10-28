# Versioning

How the renderer is versioned, how that relates to the `.gui` spec, and what to
change when either one moves. Read this before bumping a version, implementing
a new spec version, or cutting a release.

## Two version numbers

| | Where it lives | Form | Example |
|---|---|---|---|
| **Spec version** | `<gui version="…">` in every document; decided in [dotgui/core](https://github.com/dotgui/core/blob/main/rfcs/README.md) | `MAJOR.MINOR.PATCH` | `0.3.4` |
| **Renderer version** | `version` in the workspace [`Cargo.toml`](Cargo.toml) | `MAJOR.MINOR.PATCH` | `0.3.7` |

The spec is the contract. Its version tells a consumer what a document may
contain. The renderer is one consumer of that contract, and its version says
which contract it implements.

**The spec moves in chapters.** Its `MINOR` is a *chapter*: a themed group of
RFCs (0.3 is components and packages, 0.4 is motion and interaction). Its
`PATCH` is *one RFC* inside that chapter: `0.3.2` is multi-document packages,
`0.3.4` is slots. A chapter closes when every RFC on its list has shipped, been
rejected, or moved to the next chapter; the next release is then `0.4.0`. The
current assignment is in core's [RFC index](https://github.com/dotgui/core/blob/main/rfcs/README.md#index).

## The rule

> **The renderer shares the spec's chapter, and counts its own patches.
> It declares exactly which spec point it reads.**

- **Same chapter.** The renderer's `MAJOR.MINOR` is the spec chapter it reads.
  Every renderer `0.3.x` reads 0.3 documents; renderer `0.4.0` is the first that
  reads 0.4.
- **Own patch.** The renderer's `PATCH` is its own counter. It goes up for
  anything it releases — a bug fix, speed, tooling, *or* implementing the next
  RFC in the chapter. So renderer `0.3.7` may read spec `0.3.4`; the numbers
  share a chapter, not a patch.
- **Declared support.** `SUPPORTED_VERSION` names the newest spec point the
  renderer reads in full, e.g. `"0.3.4"`. That, not the renderer's own patch,
  is what a document is checked against.
- The renderer never gets a `MINOR` bump on its own: it moves to `0.4.0` only
  when it reads spec 0.4.

Why not identical numbers: the renderer needs somewhere to put a fix. If
renderer `0.3.4` had to mean spec `0.3.4`, the next fix would need `0.3.5` —
which is already the next RFC. Four-part versions (`0.3.4.1`) are not valid on
crates.io or npm, so the patch belongs to the renderer and the spec point is
declared instead.

A document declaring only `MAJOR.MINOR` (`version="0.3"`) reads as the start of
that chapter (`0.3.0`).

## Compatibility

Support is **cumulative**. A renderer reads every spec version up to its own,
and refuses anything newer.

| Renderer (reads spec) | Document declares | Result |
|---|---|---|
| `0.3.x` (`0.3.11`) | `0.1`, `0.2`, or no version (read as `0.2`) | Renders exactly as it always did |
| `0.3.x` (`0.3.11`) | `0.3`, `0.3.0` … `0.3.11` | Renders, with 0.3 rules enforced |
| `0.3.x` (`0.3.11`) | `0.3.12` and up | **Refused**: "a newer renderer is needed" |
| `0.3.x` (`0.3.11`) | `0.4`, `1.0`, … | **Refused** |
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

## Vocabulary implemented ahead of the spec

Sometimes the renderer implements an RFC before the spec version that carries
it exists. The version number cannot say so — `MAJOR.MINOR` is the spec chapter
read and `SUPPORTED_VERSION` names a spec point that exists — so such work is
*carried*, not claimed.

What that requires:

- **It must not change how a conforming document renders.** A document that does
  not use the new vocabulary comes out exactly as before. Vocabulary that cannot
  be added that way waits for its spec version instead. Show it, rather than
  reasoning it: render every example with the commit before and the commit
  after and compare the bytes, and run the goldens.
  ```bash
  cargo test -p dotgui-renderer --test golden_tests -- --ignored
  ```
  Render both sets **from the same directory**, so they share one
  `.gui-render/cache`. Examples pull charts and icons over the network, and two
  runs from different working directories differ on cache state alone — which
  looks exactly like a rendering change and is not one.
- **It moves no version number.** Not `MAJOR.MINOR` or `SUPPORTED_VERSION`,
  which belong to the spec, and it is not the reason for a `PATCH`: a release
  that includes it is numbered for whatever else it ships.
- **It is listed below**, so the gap between what the renderer does and what its
  version claims is written down rather than inferred from the source.
- **It is revisited when its RFC moves.** An RFC in Draft or Proposed can change
  shape or be rejected, and what is implemented here follows it — including
  being taken out again.

### Carried today

| Vocabulary | RFC | Status in core | Chapter it belongs to |
|---|---|---|---|
| Token modes — `<mode>`/`<modes>`, per-mode token values, `mode-{axis}` pins | [0037](https://github.com/dotgui/core/blob/main/rfcs/0037-token-modes-theming.md) | Proposed | 1.0 |

Token modes shipped ahead of its chapter because kit already resolves them. A
document using them rendered in one renderer and failed to parse in the other,
which is a divergence about whole documents rather than about pixels. It meets
the additive rule: `modes::apply` returns before touching the tree unless the
document declares an axis *and* a token that varies along one. Checked rather
than assumed — all 27 examples render byte-identical across the commit that
added it, and the 18 goldens hold.

It cannot be versioned `0.3.x`, because it is not in the 0.3 chapter. It cannot
be `1.0` either — that chapter also holds composite token types (RFC-0038,
Draft), and a renderer claiming a version implements all of it, not a subset
(RFC-0042, *The version is the contract*). So the renderer stays `0.3.x` and
token modes sits outside the number until RFC-0037 is Accepted and lands in a
chapter of its own.

## Where the versions show

| | Value today |
|---|---|
| `RENDERER_VERSION` (Rust) | `0.3.1`, read from `Cargo.toml` at build time |
| `SUPPORTED_VERSION` (Rust) | `"0.3.11"`, in `parser.rs` |
| `render_png --version` | `dotgui renderer 0.3.1 (reads .gui up to version 0.3.11)` |
| WASM `renderer_version()` / `supported_spec_version()` | `"0.3.1"` / `"0.3.11"` |
| Error for a newer document | `the document declares version 0.3.12, but this renderer (0.3.1) reads documents up to version 0.3.11; a newer renderer is needed` |

The test `the_renderer_version_names_the_spec_it_implements` in `parser.rs`
fails if `Cargo.toml`'s `MAJOR.MINOR` and `SUPPORTED_VERSION`'s chapter disagree. Treat a
failure as a question, not an obstacle: either the spec was implemented without
bumping the renderer, or the renderer was bumped without implementing the spec.

## Checklist: implementing the next RFC in a chapter

Say core accepts slots as spec `0.3.4`, and the renderer reads `0.3.3`.

1. **Work on the RFC's branch**, named like core's: `rfc/0044-slots`.
2. **Gate new errors on the version** (see *New rules apply only to new documents*)
   and **add fixtures declaring `0.3.4`**. Never edit an existing fixture's version.
3. **Check nothing old moved** (the test commands below).
4. **Bump in one commit**, once complete: `SUPPORTED_VERSION = "0.3.4"`, and the
   renderer's own `PATCH` by one (`0.3.6` → `0.3.7`).
5. **Merge with core.** Core, parser, renderer and kit merge the RFC together;
   see core's [GOVERNANCE.md](https://github.com/dotgui/core/blob/main/GOVERNANCE.md#branches-and-merging).

## Checklist: opening a new chapter

Say core opens spec `0.4`.

1. **Find what 0.4 contains.** Every RFC in `core/rfcs/` with `targets: 0.4.x`,
   plus the spec changes in `core/spec/`. A renderer reads the chapter one RFC
   at a time, but never claims a spec point without implementing it in full
   (RFC-0042, *The version is the contract*).
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
   - `parser.rs`: `SUPPORTED_VERSION = "0.4.0"` and `SUPPORTED = (0, 4, 0)`
8. **Clear anything the new version now carries.** If the chapter includes an
   RFC listed under *Vocabulary implemented ahead of the spec*, the version now
   claims it: take its row out of that table, since it is no longer a gap
   between what the renderer does and what its number says.
9. **Update the docs.** The version table above, the README's feature list and
   Versions section, and `COVERAGE.md`.
10. **Release after core ships the version.** Core's RFC README marks each
   version `in progress` or `shipped`. The renderer can carry `0.4.0` on a
   branch while core is still in progress, but don't tag the release until 0.4
   is shipped there, because the contract can still change.

## Checklist: a renderer-only release

For fixes, performance, kit-equivalence work, tooling and docs that don't change
which spec is read:

1. Bump `PATCH` in `Cargo.toml`: `0.3.1` → `0.3.2`.
2. Leave `SUPPORTED_VERSION` alone — and leave it alone for carried vocabulary
   too. New vocabulary is not a patch, so a release that happens to include
   some does not get a bigger number for it; it is the fixes beside it that
   earn the `PATCH`.
3. Run the workspace tests and clippy with CI's toolchain:
   ```bash
   cargo +1.98 clippy --workspace --all-targets -- -D warnings
   ```

## After 1.0

Core applies semver to the spec from `1.0`: `MAJOR` = breaking, `MINOR` =
additive, `PATCH` = clarification only. Chapters end there — a new RFC becomes a
spec `MINOR`. The renderer's rule doesn't change: its `MAJOR.MINOR` names the
spec version it reads, its `PATCH` is its own, and `SUPPORTED_VERSION` declares
the exact spec point.

## Tags

Tag releases `vMAJOR.MINOR.PATCH` from `main`: `v0.3.0`, `v0.3.1`. The tag and
`Cargo.toml` must agree.
