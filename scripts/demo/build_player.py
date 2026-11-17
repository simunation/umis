#!/usr/bin/env python3
"""Builds the offline demo player (one self-contained HTML file) from a recording.

    python3 scripts/demo/build_player.py .demo/small/recording.json out.html [--title "..."]

Steps the reference world from the genesis (`target/release/examples/world`), checks
every live board sampled from the network against it, and embeds the boards (bit-packed,
deflated) with who simulated, committed and proved each zone slot.
"""
import argparse, base64, json, os, struct, subprocess, sys, zlib

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
WORLD = os.path.join(ROOT, "target/release/examples/world")
TEMPLATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "player.html")

# Categorical node colours, distinct on the dark board (validators first).
PALETTE = ["#f2c14e", "#f78154", "#4d9de0", "#e15554", "#7bd389", "#c879ff", "#3bceac", "#ff8fab",
           "#a0c4ff", "#ffd6a5", "#9ef01a", "#ff70a6", "#70d6ff", "#e9ff70", "#b8b8ff", "#ff9770"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("rec")
    ap.add_argument("out")
    ap.add_argument("--title", default="Distributed Life")
    a = ap.parse_args()
    r = json.load(open(a.rec))
    g = r["genesis"]
    zones = [z for z in g["zones"] if z["task_id"].startswith("life2d")]
    zp = zones[0]["params"]
    n = zp["n"]
    rx, ry = zp["genesis_region"]
    idx = {nd["id"]: i for i, nd in enumerate(r["nodes"]) if nd["id"]}

    # zone generations over time (and the frames they need)
    zgen = {int(z): [[round(t, 2), gen] for t, gen, _pop in h] for z, h in r["gens"].items()}
    max_gen = max(gen for h in zgen.values() for _, gen in h)

    # reference world
    tmp = os.path.join(os.path.dirname(os.path.abspath(a.rec)), "world.bin")
    subprocess.run([WORLD, os.path.join(os.path.dirname(os.path.abspath(a.rec)), "genesis.json"), str(max_gen), tmp],
                   check=True)
    raw = open(tmp, "rb").read()
    w, h, frames = struct.unpack("<III", raw[:12])
    fb = (w * h + 7) // 8
    body = raw[12:]

    def cell(f, x, y):
        i = y * w + x
        return (body[f * fb + i // 8] >> (i % 8)) & 1

    # exactness: every live board sampled from the network equals the reference crop
    checked = matched = 0
    bad = []
    for s in r["samples"]:
        if s["gen"] >= frames:
            continue
        bits = bytes.fromhex(s["bits"])
        ox, oy, _ = s["origin"]
        sn = s["n"]
        ok = all(((bits[(yy * sn + xx) // 8] >> ((yy * sn + xx) % 8)) & 1) == cell(s["gen"], ox + xx, oy + yy)
                 for yy in range(sn) for xx in range(sn))
        checked += 1
        matched += ok
        if not ok:
            bad.append((s["zone"], s["gen"]))
    print(f"exactness: {matched}/{checked} live boards equal the global Game of Life", file=sys.stderr)
    if bad:
        print("mismatches:", bad[:10], file=sys.stderr)

    # Display timeline on a 0.25 s grid, clamped to lockstep: on the canonical chain
    # neighbouring zones (8-neighbourhood) are never more than one generation apart, so
    # a view further ahead comes from a node still on an abandoned fork after a rollback.
    pts0 = {z["id"]: tuple(z["point"][:2]) for z in zones}
    nb = {a_: [b_ for b_ in pts0 if b_ != a_ and max(abs(pts0[a_][0] - pts0[b_][0]), abs(pts0[a_][1] - pts0[b_][1])) == 1]
          for a_ in pts0}
    def raw_at(z, t):
        g_ = 0
        for tt, gg in zgen.get(z, []):
            if tt <= t:
                g_ = gg
            else:
                break
        return g_
    tmax = max(tt for h in zgen.values() for tt, _ in h)
    clamped = 0
    out = {z: [] for z in pts0}
    t = 0.0
    while t <= tmax + 0.25:
        gs = {z: raw_at(z, t) for z in pts0}
        changed = True
        while changed:
            changed = False
            for z in pts0:
                lim = min(gs[b_] for b_ in nb[z]) + 1 if nb[z] else gs[z]
                if gs[z] > lim:
                    gs[z] = lim
                    changed = True
        for z in pts0:
            if gs[z] != raw_at(z, t):
                clamped += 1
            if not out[z] or out[z][-1][1] != gs[z]:
                out[z].append([round(t, 2), gs[z]])
        t += 0.25
    print(f"display: {clamped} zone-samples clamped to lockstep (stale fork views during recovery)", file=sys.stderr)
    zgen = out

    # lockstep: neighbouring zones (8-neighbourhood) never differ by more than one generation
    pts = {z["id"]: tuple(z["point"][:2]) for z in zones}
    def gen_at(z, t):
        g = 0
        for tt, gg in zgen.get(z, []):
            if tt <= t:
                g = gg
            else:
                break
        return g
    times = sorted({tt for h in zgen.values() for tt, _ in h})
    viol = 0
    for t in times[:: max(1, len(times) // 400)]:
        gs = {z: gen_at(z, t) for z in pts}
        for a_ in pts:
            for b_ in pts:
                if a_ < b_ and max(abs(pts[a_][0] - pts[b_][0]), abs(pts[a_][1] - pts[b_][1])) == 1 and abs(gs[a_] - gs[b_]) > 1:
                    viol += 1
    print(f"lockstep: {viol} neighbour pairs more than one generation apart (sampled times)", file=sys.stderr)

    # attribution from the beacon history
    lat = r["lattice"]
    beacon = []
    blocks = {z["id"]: {} for z in zones}
    commits = {z["id"]: {} for z in zones}
    for b in lat["blocks"]:
        beacon.append([b["slot"], idx.get(b["proposer"], -1)])
        for c in b["commits"]:
            for slot, m in c["minters"]:
                blocks[c["zone"]][slot] = idx.get(m, -1)
            sl = [s for s, _ in c["minters"]]
            commits[c["zone"]][c["e"]] = {"e": c["e"], "inc": b["slot"], "s0": min(sl) if sl else None,
                                          "s1": max(sl) if sl else None, "by": idx.get(c["signer"], -1),
                                          "pv": -1, "pat": None}
        for p in b["proofs"]:
            cm = commits.get(p["zone"], {}).get(p["e"])
            if cm is not None:
                cm["pv"] = idx.get(p["signer"], -1)
                cm["pat"] = b["slot"]
    bslot = r["beacon_slot"]
    rollbacks = [{"t": rb["e_record"] * bslot, "zone": rb["zone"], "reason": rb["reason"], "jr": rb["j_r"]}
                 for rb in lat["rollbacks"]]

    # node status, per second
    names = [nd["name"] for nd in r["nodes"]]
    status = []
    for row in r["statuses"]:
        status.append([round(row["t"], 1)] + [
            [1 if row["nodes"].get(nm, {}).get("alive") else 0, row["nodes"].get(nm, {}).get("minted") or 0,
             row["nodes"].get(nm, {}).get("proved") or 0, row["nodes"].get(nm, {}).get("zone")]
            for nm in names])
    # when each node first appears (late joiners)
    first_seen = {}
    for row in r["statuses"]:
        for nm, v in row["nodes"].items():
            if v.get("alive") and nm not in first_seen:
                first_seen[nm] = row["t"]

    data = {
        "title": a.title,
        "n": n, "rx": rx, "ry": ry, "w": w, "h": h, "frames": frames,
        "tick": r["tick"], "bslot": bslot, "slotTicks": zones[0]["slot_ticks"], "duration": r["duration"],
        "pattern": zp.get("pattern", 0), "layout": [zp["strips"], zp["rows_per_trace_row"]],
        "zones": [{"id": z["id"], "p": z["point"][:2]} for z in zones],
        "nodes": [{"name": nd["name"], "role": nd["role"], "prover": nd["prover"],
                   "color": PALETTE[i % len(PALETTE)], "from": round(first_seen.get(nd["name"], 0), 1)}
                  for i, nd in enumerate(r["nodes"])],
        "zgen": {str(k): v for k, v in zgen.items()},
        "blocks": {str(z): sorted([[s, m] for s, m in v.items()]) for z, v in blocks.items()},
        "commits": {str(z): sorted(v.values(), key=lambda c: c["e"]) for z, v in commits.items()},
        "beacon": beacon,
        "rollbacks": rollbacks,
        "events": [{"t": round(e["t"], 1), "kind": e["kind"], "node": e["node"], "text": e["text"]}
                   for e in r["events"]],
        "status": status,
        "verify": {"checked": checked, "matched": matched, "lockstep_violations": viol, "clamped": clamped},
        "board": base64.b64encode(zlib.compress(body, 9)).decode(),
    }
    html = open(TEMPLATE).read().replace("/*__DATA__*/null", json.dumps(data, separators=(",", ":")))
    html = html.replace("__TITLE__", a.title)
    open(a.out, "w").write(html)
    print(f"{a.out}: {len(html) / 1e6:.2f} MB, {frames} generations of a {w}x{h} world", file=sys.stderr)


if __name__ == "__main__":
    main()
