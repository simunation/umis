# Vendored crates

- `libp2p-webrtc-websys` 0.5.0: unchanged except `src/stream/poll_data_channel.rs`,
  where the per-data-channel read buffer limit is raised from 16 KiB
  (`MAX_MSG_LEN`) to 8 MiB. Upstream resets the stream as soon as two messages
  arrive while the page is busy, which breaks gossipsub for multi-chunk
  messages (beacon blocks, zone blocks with attachments). Wired in through
  `[patch.crates-io]` in the workspace `Cargo.toml`.
