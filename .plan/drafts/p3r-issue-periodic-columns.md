# Draft upstream issue (Plonky3-recursion) — NOT POSTED, owner must approve before posting

**Title:** `RecursiveAir::declares_interactions` builds its symbolic layout without periodic columns

**Version:** rev `3a69ba61f713c8400b14eb3d8f0e52caa13512c7` (`recursion/src/traits/air.rs:236-246`).

**What happens.** The blanket `declares_interactions` builds

```rust
let layout = AirLayout {
    preprocessed_width,
    main_width: self.width(),
    num_public_values: self.num_public_values(),
    ..Default::default()   // num_periodic_columns = 0
};
let mut builder = InteractionSymbolicBuilder::<F, EF>::new(layout);
self.eval(&mut builder);
```

so `builder.periodic_values()` is empty during this probe. An AIR that indexes its periodic values in `eval`
(any AIR with `num_periodic_columns() > 0`) panics with an index out of bounds while a trusted or prepared owner
is being built, although native proving and verification work.

**Repro.** Any uni-STARK AIR with one periodic column whose `eval` reads `builder.periodic_values()[0]`, passed
to `TrustedPreparedLayer::new(TrustedPreparedSource::UniStark { .. })`.

**Suggested fix.** Add `num_periodic_columns: BaseAir::<F>::num_periodic_columns(self)` to the layout (the same
way `transcript.rs:208` does).

**Workaround we use.** In `eval`, substitute zeros when `periodic_values()` is empty (only the constraint shape
matters in this probe).
