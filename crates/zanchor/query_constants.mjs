import { ApiPromise, WsProvider } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

console.log('Connected to Paseo');

// List all pallets
const palletNames = Object.keys(api.consts);
console.log('Available pallets:', palletNames.filter(p => p.toLowerCase().includes('registrar') || p.toLowerCase().includes('para')).join(', '));

// Try different pallet names for registrar
for (const palletName of ['registrar', 'parasRegistrar', 'paras', 'Registrar']) {
    if (api.consts[palletName]) {
        console.log(`\nPallet: ${palletName}`);
        const constants = api.consts[palletName];
        for (const [key, value] of Object.entries(constants)) {
            console.log(`  ${key}: ${value.toString()}`);
        }
    }
}

await api.disconnect();
