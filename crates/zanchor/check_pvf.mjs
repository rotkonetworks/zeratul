import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

console.log('=== ParaId 5082 Status ===\n');

// Paras lifecycle
const lifecycle = await api.query.paras.paraLifecycles(5082);
console.log('Lifecycle:', lifecycle.toString());

// Check the registrar
const parasInfo = await api.query.registrar.paras(5082);
console.log('Registrar info:', parasInfo.toHuman());

// Get current session
const currentSession = await api.query.session.currentIndex();
console.log('Current session:', currentSession.toString());

// Get pending paras (parachains scheduled for onboarding)
const pending = await api.query.registrar.pendingSwap.entries();
console.log('Pending swaps:', pending.length);

// Check upcoming paras
console.log('\n=== Next Steps ===');
console.log('1. PVF validation should complete within a few sessions (~30 mins)');
console.log('2. Then parachain transitions from Onboarding to Parathread');
console.log('3. Need to get coretime via https://hub.regionx.tech/?network=paseo');
console.log('4. Deploy collator to produce blocks');

await api.disconnect();
