// M1 spike: N SpikeNodes per tab. ?n=<count>&boot=<url of bootstrap.json>
import init, { SpikeNode } from './pkg/umis_web.js';

const params = new URLSearchParams(location.search);
const n = Number(params.get('n') || 2);
const bootUrl = params.get('boot') || '/bootstrap.json';

// Exposed for the e2e driver.
window.spike = { ready: false, nodes: [] };

await init();
const boot = await (await fetch(bootUrl)).json();
// Browsers can dial webrtc-direct and ws; the node filters by preference itself.
const addrs = boot.addrs;

for (let i = 0; i < n; i++) {
  const node = new SpikeNode();
  const state = { peerId: node.peerId, received: {}, connections: {}, reservation: false, registered: false, log: [] };
  const el = document.createElement('div');
  el.className = 'node';
  el.innerHTML = `<h3>${node.peerId}</h3><div class="log"></div>`;
  document.getElementById('nodes').appendChild(el);
  const logEl = el.querySelector('.log');
  const log = (s) => { state.log.push(s); logEl.textContent = state.log.slice(-40).join('\n'); };
  node.onEvent((ev) => {
    switch (ev.kind) {
      case 'Message': state.received[ev.from] = (state.received[ev.from] || 0) + 1; break;
      case 'Connected': state.connections[ev.peer] = { addr: ev.addr, relayed: ev.relayed }; log(`+ ${ev.peer.slice(-8)} ${ev.addr}`); break;
      case 'Disconnected': delete state.connections[ev.peer]; log(`- ${ev.peer.slice(-8)}`); break;
      case 'Reservation': state.reservation = true; log(`reservation at ${ev.relay.slice(-8)}`); break;
      case 'Registered': state.registered = true; log('registered'); break;
      case 'Listening': log(`listening ${ev.addr}`); break;
      case 'Error': log(`ERROR ${ev.message}`); break;
      default: break;
    }
  });
  node.start(addrs);
  setInterval(() => node.publish(`hello from ${node.peerId} at ${Date.now()}`), 1000);
  window.spike.nodes.push(state);
}
window.spike.ready = true;
