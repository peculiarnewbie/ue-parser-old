# UTrace Comparison Contract

The comparison workbench treats two captures as independent observations unless
the user explicitly selects one local range in each capture. It never assumes
that capture cycles, elapsed time, frame number, or frame ordinal share a clock
or workload identity.

## Primary outcome

Completed Game or Rendering frame summaries provide the primary comparison.
Every retained frame participates in the distribution; chart point budgets do
not change the statistics. The initial verdict uses p99 effect thresholds of
0.5 ms and 5%. This is a materiality rule, not a statistical significance test.

Budget misses are calculated against the user-selected frame budget. The UI
always reports observation counts and normalizes attribution when capture
lengths differ.

## CPU attribution

Whole-capture CPU rows use exhaustive inclusive scope totals and are normalized
by completed Game frames. Inclusive totals can overlap because parent and child
scopes both contribute time.

Explicit range rows come from the retained monotonic CPU index. The query:

- enumerates every interval overlapping the local half-open range;
- clips each interval to that range before accumulating inclusive cycles;
- reports overlap count separately from begin-in-range count;
- aggregates before sorting and bounding response rows;
- rejects a query above 100,000 distinct timer identities rather than returning
  a silently survivor-biased result.

Range impact is normalized as inclusive milliseconds per independently selected
second. A response may return at most 10,000 sorted timer identities. Returned
raw timer-identity values remain exact when only the response is bounded. The UI
groups metadata variants by local specification before matching source timers;
these grouped totals may be lower bounds when a variant was omitted. Change
classification is disabled if either response or source index is truncated.
Empty ranges are rejected and intervals touching only a range boundary are
excluded.

Cross-capture timer matching never uses capture-local specification IDs. It uses
name plus normalized source location when available, then a unique-name fallback.
Ambiguous names stay explicitly ambiguous. Unique-name fallback requires a
unique name in each entire capture, including already matched source timers.
Missing CPU providers cannot establish new or removed timers, and incomplete
frame populations cannot establish a capture-wide distribution verdict.

## Timeline evidence

Selecting an attribution row queries both retained browser sessions on demand.
The resulting timelines use local axes and local zoom state. Side-by-side layout
does not imply temporal pairing. A search can include rendered-name variants and
the interval response is visibly marked when bounded.

## Session ownership

One WASM Worker owns at most two retained progressive sessions. Captures parse
sequentially. Releasing or cancelling one session frees only that session;
global Worker termination remains reserved for the single-capture analyzer's
legacy reset path or fatal recovery.

The comparison route releases both sessions on teardown. Because releasing Rust
objects does not shrink WebAssembly linear memory, a future admission policy may
still cold-store one capture and recycle the Worker after large comparisons.

## Evidence vocabulary

Comparison results use these grades:

- `exact`
- `exact_capture_wide`
- `bounded_top_n`
- `prefix_sampled`
- `truncated_lower_bound`
- `incompatible`
- `unavailable`

Missing provider data is unavailable, never zero. Absence from sampled, bounded,
or truncated data cannot establish that a timer or provider was removed.
