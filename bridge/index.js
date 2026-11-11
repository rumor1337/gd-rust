// TikTok gift -> JSONL on stdout. Spawned and read by the Rust orchestrator.
// Reconnects forever; one line per event: {"type":"gift"|"status",...}
const { TikTokLiveConnection, WebcastEvent } = require('tiktok-live-connector');

const username = process.env.TT_USERNAME;
if (!username) { console.error('TT_USERNAME required'); process.exit(1); }

function emit(obj) { process.stdout.write(JSON.stringify(obj) + '\n'); }

async function run() {
  for (;;) {
    const conn = new TikTokLiveConnection(username, { enableExtendedGiftInfo: false });
    conn.on(WebcastEvent.GIFT, (e) => {
      if (!e.gift) return;
      // streakable gifts fire repeatedly; count only when the streak ends
      if (e.gift.type === 1 && e.repeatEnd !== 1) return;
      emit({ type: 'gift', user: e.user?.displayId ?? '?', name: e.gift.name ?? '?', count: e.repeatCount ?? 1 });
    });
    try {
      const state = await conn.connect();
      emit({ type: 'status', state: 'connected', roomId: state.roomId });
      await new Promise((res) => conn.on(WebcastEvent.DISCONNECTED, res));
      emit({ type: 'status', state: 'disconnected' });
    } catch (err) {
      emit({ type: 'status', state: 'error', msg: String(err?.message ?? err).slice(0, 200) });
    }
    await new Promise((r) => setTimeout(r, 5000)); // reconnect backoff
  }
}
run();
