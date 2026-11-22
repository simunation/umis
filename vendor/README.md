# Vendored crates

- `libp2p-webrtc-websys` 0.5.0: unchanged except `src/stream/poll_data_channel.rs`,
  where the per-data-channel read buffer limit is raised from 16 KiB
  (`MAX_MSG_LEN`) to 8 MiB. Upstream resets the stream as soon as two messages
  arrive while the page is busy, which breaks gossipsub for multi-chunk
  messages (beacon blocks, zone blocks with attachments). Wired in through
  `[patch.crates-io]` in the workspace `Cargo.toml`.

- `plonky3-recursion` (gitignored, built by `scripts/vendor-p3r.sh`): Plonky3-recursion at the pinned rev
  `3a69ba61f713c8400b14eb3d8f0e52caa13512c7` plus the committed patches `vendor/p3r-*.patch` (owner-approved
  2026-11-22). Wired in through `[patch."https://github.com/Plonky3/Plonky3-recursion"]` in the workspace
  `Cargo.toml`. `scripts/check.sh` runs the script first; run it by hand after a fresh clone.
  - `p3r-common-data-preprocessed.patch`: public accessor for the preprocessed-commitment targets of
    `CommonDataTargets` (pinning a native batch leaf's verifying key, `docs/zk/aggregation.md`).
  - `p3r-periodic-layout.patch`: declare periodic columns in two `AirLayout`s (upstream bug; batch AIRs with
    periodic columns panic in `replay_batch_stark_transcript`).
