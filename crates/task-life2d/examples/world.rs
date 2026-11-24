//! Writes the reference world of a life2d genesis: every generation `0..=G` of the
//! whole board (the genesis zones' boards stepped as one Game of Life, cells outside
//! the zones dead), bit-packed, for the demo recorder.
//!
//! `cargo run --release -p task-life2d --example world -- genesis.json G out.bin`
//! Output: u32 LE width, height, frames; then per frame `ceil(w*h/8)` bytes, cell
//! `y*w+x` at bit `i % 8` of byte `i / 8`.
use std::io::Write;
use task_life2d::board::Board;
use task_life2d::{Life2dParams, genesis_soup};
use umis_core::Hash;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let g: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&a[0])?)?;
    let frames: usize = a[1].parse()?;
    let seed = Hash::from_hex(g["seed"].as_str().unwrap_or_default()).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let zones: Vec<Life2dParams> = g["zones"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("zones"))?
        .iter()
        .filter(|z| z["task_id"].as_str().is_some_and(|t| t.starts_with("life2d")))
        .map(|z| serde_json::from_value(z["params"].clone()))
        .collect::<Result<_, _>>()?;
    let n = zones[0].n();
    let bx = zones.iter().map(|z| z.point[0]).max().unwrap_or(0) as usize + 1;
    let by = zones.iter().map(|z| z.point[1]).max().unwrap_or(0) as usize + 1;
    let (w, h) = (bx * n, by * n);
    let mut b = Board::dead(w, h);
    for z in &zones {
        b.paste(&genesis_soup(&seed, z, z.point), z.point[0] as usize * n, z.point[1] as usize * n);
    }
    let mut out = std::io::BufWriter::new(std::fs::File::create(&a[2])?);
    for v in [w as u32, h as u32, frames as u32 + 1] {
        out.write_all(&v.to_le_bytes())?;
    }
    for f in 0..=frames {
        let mut bytes = vec![0u8; (w * h).div_ceil(8)];
        for (i, &c) in b.cells.iter().enumerate() {
            bytes[i / 8] |= c << (i % 8);
        }
        out.write_all(&bytes)?;
        if f < frames {
            b = b.step();
        }
    }
    Ok(())
}
