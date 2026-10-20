# Genesis files

`umis-cli genesis` writes a devnet genesis (and the validator and faucet keys):

```sh
cargo run --release -p umis-cli -- genesis --validators 4 --start-in 12 \
  --keys-dir .devnet/keys --out genesis/devnet.json
```

`scripts/devnet.sh` does this for you into `.devnet/genesis.json`, with a
fresh random seed and a genesis time a few seconds in the future. The devnet
genesis holds the plan's defaults: 4 water zones at (0,0,0), (1,0,0), (0,1,0),
(1,1,0) with slot lengths 4, 8, 4, 16 ticks on an 8×8×2 lattice, 250 ms ticks,
16-tick beacon slots, D = K_f = 2, R = R_b = 2, and Plonky3 STARK proofs
with proof lag L = 1 (`--proof-backend mock` for signed statements and
L = 0; `--n-loc 8` for lighter zones; `--stark-config koalabear-test` for
fast, insecure proofs).
