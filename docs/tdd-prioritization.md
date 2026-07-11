# TDD prioritization for a UE translation parser

How we should grow tests going forward, and why. Concrete gaps live in
[`test-opportunity-catalog.md`](test-opportunity-catalog.md) (rewritten from a
module-by-module impl↔test audit, not a structural skim). Class/event scoreboards
remain [`asset-coverage.md`](asset-coverage.md) and `memory/utrace-coverage-matrix.md`.

---

## Why this project wants far more tests than “normal” app code

This codebase is not inventing a format. It is **re-implementing contracts that
already exist** in Unreal Engine (and, for values, in Electroswag fixtures):

| Oracle | What it answers |
|--------|-----------------|
| Local UE 5.7 tree (`memory/ue-source-reference.md`) | Wire layout, version gates, Serialize order |
| TraceServices / TraceAnalysis / harvest script | Event fields and provider semantics |
| Electroswag `contract.ts` → `electroswag-v15.json` | Authored decoded values |

When the oracle is external and stable, **tests are the product**. Implementation
is the translation layer. A ~0.5× test:prod line ratio is fine for a CRUD app; it
is thin for a parser whose job is “match the engine.” A gut target around **~4×
test LOC** is a north star for *contract surface coverage*, not a vanity metric —
prefer many small unit/property tests over one giant ignored E2E.

We already lean this way in places (crafted blobs citing `USkeleton::Serialize`,
Insights `ProcessBufferV2`, etc.). The shift is to make that the **default
workflow**, not the exception after a catalog failure.

---

## What “TDD” means here

Not “red-green-refactor every helper.” It means:

1. **Name the UE (or contract) behavior** you are about to claim.
2. **Encode it as a failing test** (crafted bytes, table of cases, or property).
3. **Implement until that test passes** without widening scope.
4. **Keep the test in the default `cargo test` path** whenever possible.

Fixture E2E and `examples/catalog` measure *breadth on real content*. They do not
replace step 2 for wire-format work — they confirm the translation still holds
on studio assets.

---

## Priority order (always)

### 1. Portable unit tests first

Default CI must exercise the contract without Perforce.

- Crafted packages / property payloads / trace packets in `#[cfg(test)]`.
- Prefer extending `src/test_support.rs` over copy-pasting byte builders.
- If a bug was found on a real asset, **minimize** it into a unit test; keep the
  ignored fixture as optional confirmation.

### 2. Property / generative tests for bounds and combinatorics

Use when the space is large but the invariant is small:

- Counts and sizes never allocate before limit checks.
- Nesting depth caps.
- Fixed-size container elements don’t over-read.
- Dashboard aggregates stay non-negative / parent ≥ child where claimed.
- Random *valid-enough* type trees or event registries don’t panic.

No property-test crate is wired yet; when we add one, start on `archive` +
`property` + `codec` fixed sizes — highest AGENTS.md alignment.

### 3. Committed tiny fixtures

Hex/binary under `tests/fixtures/tiny/` (and small synthetic `.utrace` blobs)
for cases that are awkward as inline arrays but must stay in CI.

### 4. Ignored studio E2E second

Electroswag + real `.utrace` captures prove domain values and provider traffic
we cannot fully synthesize. They stay `#[ignore]` / env-gated so clones stay
green — but **new behavior should not be proven only there**.

### 5. Fuzz as a backstop

Existing targets are panic-only. Keep fuzzing; gradually add “rejects or Raw”
assertions where false “success” would be dangerous. Fuzz does not replace
oracle-backed unit tests.

---

## How to prioritize *what* to test next

Use this filter when choosing work (including “just add tests” passes):

```text
1. Has this bug class already shipped or nearly shipped?     → do it now
2. Is this on the shared path (archive / property / codec / package)?
                                                            → before class-specific work
3. Are we about to implement or promote a decoder/provider? → red tests first
4. Does EVENT_COVERAGE / asset-coverage claim Decoded/✅?  → must have a unit test
5. Is it bulk pixels/geometry or full Insights UI parity?   → defer
```

**Shared path > leaf decoder.** A SoftObject-in-Map unit test protects thousands
of assets; an AnimSequence tail test protects one class — both matter, but the
shared path unblocks everything.

**Claimed status > aspirational status.** If `EVENT_COVERAGE` says Decoded or
`asset-coverage` says ✅, a missing unit test is a documentation bug. Fix the
test or downgrade the claim. The audit found this already: 3 of 5 Decoded
utrace rows (`EventSpec`, `BeginFrame`, `EndFrame`) lack payload unit tests.

**TDD the backlog item you’re picking up.** Don’t pre-write hundreds of Anim*
tests before that priority is active — but when you start AnimSequence, the
first commit should be red tests from `UAnimSequence::Serialize`, not a partial
decoder.

---

## Mapping work types to test styles

| Work | Lead with | Then |
|------|-----------|------|
| New property type / tag shape | Unit crafted payload | Property over sizes; fixture cell if authored |
| Package version/flag gate | Table-driven unit | Tiny hex corpus |
| New asset class tail | Red unit from UE Serialize | Catalog sample; optional fixture |
| UTrace event Partial → Decoded | Red crafted packet from TraceServices | Inventory/coverage assert; optional real capture |
| CLI JSON field / schema bump | Unit on output mappers | `tests/cli.rs` / fixture schema assert |
| Limit / DoS defense | Unit + property | Fuzz |

---

## Discipline rules (keep us honest)

1. **No silent skips.** Fixture absence is skip or ignore with a clear message;
   `REQUIRE_FIXTURE=1` must mean fail. Don’t leave README and harness disagreeing.
2. **No status without a test.** Updating `EVENT_COVERAGE` or `asset-coverage.md`
   to a stronger status requires a default-path unit test in the same change.
3. **Prefer invariants over goldens** for large utrace dashboards (counts,
   pairing, bounds, monotonicity). Goldens for tiny crafted packets are fine.
4. **One oracle per assert.** Wire layout ↔ UE source comments/paths; cell
   values ↔ contract mirror — don’t invent expected numbers.
5. **Don’t bypass Reader limits in tests** that pretend to be production paths;
   tests should use the same APIs (AGENTS.md).
6. **Grow `test_support`, don’t fork builders** for every module.

---

## What success looks like

- Default `cargo test --all-targets --features utrace` carries most of the
  contract weight; ignored fixtures are confirmation, not the only proof.
- Shared layers (`archive`, `property`, `codec`, `package`) are obviously
  “test-heavy”; leaf decoders arrive with red tests already in tree.
- Property tests cover combinatorial bounds; fuzz remains the chaos net.
- Line-count ratio drifts toward “tests dominate” because **each UE behavior we
  claim is encoded**, not because we duplicated E2E three ways.

The catalog is the backlog of *where*; this doc is the *policy*. When they
conflict, follow the priority filter above and update the catalog.
