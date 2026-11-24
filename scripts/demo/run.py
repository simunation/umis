#!/usr/bin/env python3
"""Local distributed Game of Life demo: run, perturb, record.

Starts a bootstrap, genesis validators (beacon chain + full nodes) and joining zone
members on this machine, some proving on the GPU (the A6000 only), the rest on the
CPU; fires scenario events (crash a prover, restart it, late joiner); records who
simulated and who proved every zone slot, the zones' generations over time and live
board snapshots; writes `recording.json` for `build_player.py`.

    python3 scripts/demo/run.py --out .demo/small --n 64 --region 3,2 --duration 120

Needs `target/release/umis-cli` built with `--features cuda` and
`target/release/examples/world` (`cargo build --release -p task-life2d --example world`).
"""
import argparse, json, os, signal, subprocess, sys, time, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CLI = os.path.join(ROOT, "target/release/umis-cli")


def get(url, timeout=2.0):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read())


def log(*a):
    print(f"[demo {time.strftime('%H:%M:%S')}]", *a, flush=True)


class Node:
    def __init__(self, name, role, prover, key, data, env):
        self.name, self.role, self.prover, self.key, self.data, self.env = name, role, prover, key, data, env
        self.proc, self.url, self.id, self.logf = None, None, None, None

    def start(self, genesis, http, join):
        args = [CLI, "node", "--genesis", genesis, "--key", self.key, "--data-dir", self.data,
                "--bootstrap-http", f"http://{http}"]
        if join:
            args += ["--join", "member"]
        env = dict(os.environ)
        env.update(self.env)
        self.logf = open(self.data + ".log", "a")
        self.proc = subprocess.Popen(args, env=env, stdout=self.logf, stderr=subprocess.STDOUT)
        self.url = None

    def http(self):
        if self.url is None:
            p = os.path.join(self.data, "http.addr")
            if os.path.exists(p):
                self.url = open(p).read().strip()
        return self.url

    def alive(self):
        return self.proc is not None and self.proc.poll() is None

    def kill(self):
        if self.alive():
            self.proc.send_signal(signal.SIGKILL)
            self.proc.wait()
        if os.path.exists(os.path.join(self.data, "http.addr")):
            os.remove(os.path.join(self.data, "http.addr"))
        self.url = None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--n", type=int, default=64)
    ap.add_argument("--region", default="3,2")
    ap.add_argument("--slot-ticks", type=int, default=4)
    ap.add_argument("--validators", default="gpu,cpu,cpu,cpu", help="prover per validator")
    ap.add_argument("--members", default="gpu,cpu,gpu,cpu,cpu,cpu", help="prover per joining member")
    ap.add_argument("--late", default="cpu", help="provers of members joining late (comma list, may be empty)")
    ap.add_argument("--late-at", type=float, default=0.45, help="fraction of the run when late members join")
    ap.add_argument("--events", default="0.30:crash:m0,0.60:restart:m0",
                    help="frac:crash|restart:node, comma separated (fractions of the run)")
    ap.add_argument("--max-vram-mb", type=int, default=20000, help="stop if the A6000 uses more")
    ap.add_argument("--min-free-ram-gb", type=float, default=12)
    ap.add_argument("--duration", type=float, default=120, help="seconds after genesis")
    ap.add_argument("--pattern", default="showcase")
    ap.add_argument("--sample-every", type=float, default=4.0, help="seconds between live board samples")
    ap.add_argument("--http", default="127.0.0.1:18431")
    ap.add_argument("--cpu-threads", default="2")
    a = ap.parse_args()

    out = os.path.abspath(a.out)
    subprocess.run(["rm", "-rf", out], check=True)
    os.makedirs(out)
    keys = os.path.join(out, "keys")
    genesis = os.path.join(out, "genesis.json")
    vprov = [p for p in a.validators.split(",") if p]
    subprocess.run([CLI, "genesis", "--validators", str(len(vprov)), "--start-in", "15", "--keys-dir", keys,
                    "--out", genesis, "--task", "life", "--n-loc", str(a.n), "--gens", "1", "--region", a.region,
                    "--slot-ticks", str(a.slot_ticks), "--pattern", a.pattern, "--proof-backend", "plonky3"],
                   check=True, stdout=subprocess.DEVNULL)
    g = json.load(open(genesis))
    gt = g["genesis_time_ms"] / 1000.0
    p = g["params"]
    tick = p["tick_ms"] / 1000.0
    bslot = p["beacon_slot_ticks"] * tick

    def env_for(prover):
        if prover == "gpu":
            # The A6000 only (never the 3090 Ti): PCI order, device 0.
            return {"CUDA_DEVICE_ORDER": "PCI_BUS_ID", "CUDA_VISIBLE_DEVICES": "0", "UMIS_GPU_PROVE": "1",
                    "UMIS_GPU": "A6000", "UMIS_GPU_CPU_THREADS": a.cpu_threads, "RAYON_NUM_THREADS": a.cpu_threads}
        return {"CUDA_VISIBLE_DEVICES": "", "RAYON_NUM_THREADS": a.cpu_threads}

    boot_dir = os.path.join(out, "bootstrap")
    blog = open(os.path.join(out, "bootstrap.log"), "w")
    boot = subprocess.Popen([CLI, "bootstrap", "--genesis", genesis, "--data-dir", boot_dir, "--faucet-key",
                             os.path.join(keys, "faucet.key"), "--http", a.http], stdout=blog, stderr=subprocess.STDOUT)
    for _ in range(100):
        try:
            get(f"http://{a.http}/bootstrap.json")
            break
        except Exception:
            time.sleep(0.1)

    nodes = []
    for i, pr in enumerate(vprov):
        nodes.append(Node(f"v{i}", "validator", pr, os.path.join(keys, f"node-{i}.key"),
                          os.path.join(out, f"v{i}"), env_for(pr)))
    mprov = [p for p in a.members.split(",") if p]
    for i, pr in enumerate(mprov):
        nodes.append(Node(f"m{i}", "member", pr, os.path.join(keys, f"member-{i}.key"),
                          os.path.join(out, f"m{i}"), env_for(pr)))
    late = [Node(f"m{len(mprov) + i}", "member", pr, os.path.join(keys, f"member-{len(mprov) + i}.key"),
                 os.path.join(out, f"m{len(mprov) + i}"), env_for(pr)) for i, pr in enumerate(x for x in a.late.split(",") if x)]
    for nd in nodes:
        nd.start(genesis, a.http, nd.role == "member")
    log(f"{len(nodes)} nodes up; genesis in {gt - time.time():.1f}s")

    events, samples, gens, statuses = [], [], {}, []
    byname = {nd.name: nd for nd in nodes + late}
    plan = []
    for ev in [e for e in a.events.split(",") if e]:
        fr, what, name = ev.split(":")
        if name in byname:
            plan.append((float(fr), what, name))
    if late:
        plan += [(a.late_at, "join", nd.name) for nd in late]
    plan.sort()
    zones = [z["id"] for z in g["zones"]]
    obs = nodes[1] if len(nodes) > 1 else nodes[0]
    last_status = last_sample = last_res = last_lat = last_meta = 0.0
    zone_of = {}
    beacon = {}  # slot -> beacon block view (later fetches win: canonical as of then)
    lat_extra = {"rollbacks": [], "slashes": []}

    def fetch_lattice(frm, timeout):
        for nd in [obs] + [x for x in nodes if x.role == "validator" and x is not obs]:
            u = nd.http()
            if not (nd.alive() and u):
                continue
            try:
                lv = get(f"{u}/lattice?from={frm}", timeout)
            except Exception:
                continue
            for b in lv.get("blocks", []):
                beacon[b["slot"]] = b
            for k in ("rollbacks", "slashes"):
                for x in lv.get(k, []):
                    if x not in lat_extra[k]:
                        lat_extra[k].append(x)
            return True
        return False
    resources = []
    si = 0
    end = gt + a.duration
    try:
        while time.time() < end:
            now = time.time()
            t = now - gt
            while plan and t >= plan[0][0] * a.duration:
                _, what, name = plan.pop(0)
                nd = byname[name]
                if what == "crash":
                    nd.kill()
                    events.append({"t": t, "kind": "crash", "node": name, "text": f"{name} ({nd.prover.upper()}) crashed"})
                elif what == "restart":
                    nd.start(genesis, a.http, nd.role == "member")
                    events.append({"t": t, "kind": "restart", "node": name, "text": f"{name} restarted, catching up"})
                elif what == "join":
                    nd.start(genesis, a.http, True)
                    nodes.append(nd)
                    events.append({"t": t, "kind": "join", "node": name, "text": f"{name} ({nd.prover.upper()}) joins late"})
                log(events[-1]["text"])
            # Observe each zone through one of its own members (spreads the load; the
            # validators keep their event loops for the beacon chain), else a validator.
            def observer(z):
                for nd in nodes:
                    if nd.role == "member" and zone_of.get(nd.name) == z and nd.alive() and nd.http():
                        return nd.http()
                for nd in nodes:
                    if nd.role == "validator" and nd.alive() and nd.http():
                        return nd.http()
                return None
            if t > -2 and now - last_meta > 0.5:
                last_meta = now
                vals = [nd for nd in nodes if nd.role == "validator" and nd.alive() and nd.http()]
                for z in zones:
                    # the zone's member and one validator (rotating): a node that is still
                    # catching up shows an old generation, so keep the most advanced view
                    obs_urls = {observer(z)}
                    if vals:
                        obs_urls.add(vals[(z + int(now * 2)) % len(vals)].http())
                    best = None
                    for ob in obs_urls:
                        if not ob:
                            continue
                        try:
                            m = get(f"{ob}/zonemeta/{z}", 1.0)
                        except Exception:
                            continue
                        if isinstance(m, dict) and "gen" in m and (best is None or m["gen"] > best["gen"]):
                            best = m
                    m = best
                    if m is None:
                        continue
                    hist = gens.setdefault(z, [])
                    if not hist or hist[-1][1] != m["gen"]:
                        hist.append([round(t, 3), m["gen"], m.get("population", 0)])
            if t > 0 and now - last_sample > a.sample_every:
                # a live board, for the exactness check against the global reference
                z = zones[si % len(zones)]
                si += 1
                ob = observer(z)
                try:
                    r = get(f"{ob}/zone/{z}", 3.0)
                    cells = r["data"][0::2]
                    bits = bytearray((len(cells) + 7) // 8)
                    for i, c in enumerate(cells):
                        if c:
                            bits[i // 8] |= 1 << (i % 8)
                    samples.append({"t": round(t, 3), "zone": z, "gen": r["meta"]["gen"], "origin": r["origin"],
                                    "n": r["dims"][0], "bits": bits.hex()})
                except Exception:
                    pass
                last_sample = now
            if now - last_res > 10.0:
                last_res = now
                q = subprocess.run(["nvidia-smi", "--query-gpu=name,memory.used", "--format=csv,noheader,nounits"],
                                   capture_output=True, text=True).stdout
                vram = {l.split(",")[0].strip(): int(l.split(",")[1]) for l in q.strip().splitlines()}
                apps = subprocess.run(["nvidia-smi", "--query-compute-apps=gpu_name,pid", "--format=csv,noheader"],
                                      capture_output=True, text=True).stdout
                ours = {str(nd.proc.pid) for nd in nodes if nd.alive()}
                on3090 = [l for l in apps.splitlines() if "3090" in l and l.split(",")[1].strip() in ours]
                free = int(open("/proc/meminfo").read().split("MemAvailable:")[1].split()[0]) / 1e6
                resources.append({"t": round(t, 1), "vram": vram, "free_ram_gb": round(free, 1)})
                a6k = next((v for k, v in vram.items() if "A6000" in k), 0)
                if on3090:
                    log("a demo node is on the 3090 Ti: stopping", on3090)
                    break
                if a6k > a.max_vram_mb or free < a.min_free_ram_gb:
                    log(f"resource limit (A6000 {a6k} MB, free RAM {free:.1f} GB): stopping early")
                    break
            if now - last_lat > 8.0 and t > 4:
                last_lat = now
                fetch_lattice(max(0, (max(beacon) if beacon else 0) - 4), 5.0)
            if now - last_status > 1.0:
                row = {"t": round(t, 3), "nodes": {}}
                for nd in nodes:
                    u = nd.http()
                    st = None
                    if nd.alive() and u:
                        try:
                            st = get(f"{u}/status", 1.0)
                        except Exception:
                            st = None
                    if st:
                        nd.id = st.get("node_id") or nd.id
                        zone_of[nd.name] = st.get("zone")
                        row["nodes"][nd.name] = {"alive": True, "kind": st.get("kind"), "zone": st.get("zone"),
                                                 "minted": st.get("minted"), "proved": st.get("proved"),
                                                 "proposed": st.get("proposed"), "head": st.get("head_slot"),
                                                 "fin": st.get("finalized_slot"), "rollbacks": st.get("rollbacks")}
                    else:
                        row["nodes"][nd.name] = {"alive": nd.alive()}
                statuses.append(row)
                last_status = now
            time.sleep(0.1)
        a.duration = min(a.duration, time.time() - gt)
        # final: the whole beacon history with commitments, minters and provers
        for _ in range(3):
            if fetch_lattice(max(0, (max(beacon) if beacon else 0) - 6), 20.0):
                break
        lat = {"blocks": [beacon[k] for k in sorted(beacon)], **lat_extra}
    finally:
        for nd in nodes + late:
            nd.kill()
        boot.kill()
    rec = {
        "genesis": g, "genesis_time": gt, "tick": tick, "beacon_slot": bslot, "duration": a.duration,
        "nodes": [{"name": nd.name, "role": nd.role, "prover": nd.prover, "id": nd.id} for nd in nodes],
        "events": events, "resources": resources, "gens": gens, "samples": samples, "statuses": statuses, "lattice": lat,
    }
    json.dump(rec, open(os.path.join(out, "recording.json"), "w"))
    log(f"recorded {len(statuses)} status rows, {len(samples)} board samples, "
        f"{len(lat.get('blocks', []))} beacon blocks -> {out}/recording.json")


if __name__ == "__main__":
    main()
