import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

console.log('Connected to Paseo');

// Our main account
const mainAccount = '5GYjFw1tqoMkKjLnwB9BmQ7YvaYzyeYSFkmm1igBJVQKL9gU';

// Check next free para ID
const nextFreeParaId = await api.query.registrar.nextFreeParaId();
console.log('Next free ParaId:', nextFreeParaId.toString());

// Check pending swap for our paraIds
for (const paraId of [5081, 5082]) {
    const pending = await api.query.registrar.pendingSwap(paraId);
    console.log(`ParaId ${paraId} pendingSwap:`, pending.toString() || 'none');
    
    // Check paras info
    const paras = await api.query.registrar.paras(paraId);
    console.log(`ParaId ${paraId} paras info:`, paras.toString() || 'none');
}

// List all our reserved paraIds by checking the Deferred storage
// Actually let's check if we have any reserved by iterating pending paras
const entries = await api.query.registrar.paras.entries();
console.log('\nAll registered/pending paras:');
for (const [key, value] of entries.slice(-10)) {
    const paraId = key.args[0].toString();
    console.log(`  ParaId ${paraId}:`, value.toHuman());
}

await api.disconnect();
