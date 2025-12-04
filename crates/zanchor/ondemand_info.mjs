import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.dotters.network');
const api = await ApiPromise.create({ provider });

console.log('=== On-Demand Coretime ===\n');

// List on-demand extrinsics
console.log('Available extrinsics:');
const onDemand = api.tx.onDemand;
if (onDemand) {
    for (const method of Object.keys(onDemand)) {
        if (typeof onDemand[method] === 'function') {
            console.log('  onDemand.' + method);
        }
    }
}

// Check storage
console.log('\nStorage queries:');
const storage = api.query.onDemand;
if (storage) {
    for (const key of Object.keys(storage)) {
        if (typeof storage[key] === 'function') {
            console.log('  ' + key);
        }
    }
}

// Get current spot price
try {
    const revenue = await api.query.onDemand.revenue();
    console.log('\nRevenue info:', revenue.toHuman());
} catch (e) {}

try {
    const affinity = await api.query.onDemand.paraIdAffinity.entries();
    console.log('Affinity entries:', affinity.length);
} catch (e) {}

// Check OnDemandAssignment queue
try {
    const queue = await api.query.onDemandAssignment?.freeEntries();
    console.log('Free entries:', queue?.toString());
} catch (e) {}

await api.disconnect();
