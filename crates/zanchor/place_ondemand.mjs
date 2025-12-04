import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.dotters.network');
const api = await ApiPromise.create({ provider });

const keyring = new Keyring({ type: 'sr25519' });
const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');

console.log('=== Place On-Demand Order ===');
console.log('Account:', signer.address);
console.log('ParaId: 5082');

// Check lifecycle first
const lifecycle = await api.query.paras.paraLifecycles(5082);
console.log('Lifecycle:', lifecycle.toString());

if (lifecycle.toString() === 'Onboarding') {
    console.log('\nERROR: ParaId still onboarding. Wait for PVF validation to complete.');
    await api.disconnect();
    process.exit(1);
}

// Get current queue status
const queueStatus = await api.query.onDemand.queueStatus();
console.log('\nQueue status:', queueStatus.toHuman());

// Place on-demand order
// maxAmount is the max price we're willing to pay (in planck)
const maxAmount = 100_000_000_000n; // 10 PAS max per order
const paraId = 5082;

console.log('\nPlacing on-demand order...');
console.log('Max amount:', (Number(maxAmount) / 1e10).toFixed(4), 'PAS');

const tx = api.tx.onDemand.placeOrderKeepAlive(maxAmount, paraId);

await new Promise((resolve, reject) => {
    tx.signAndSend(signer, ({ status, events, dispatchError }) => {
        console.log('Status:', status.type);
        
        if (dispatchError) {
            if (dispatchError.isModule) {
                const decoded = api.registry.findMetaError(dispatchError.asModule);
                console.log('ERROR:', decoded.section + '.' + decoded.method, '-', decoded.docs.join(' '));
            } else {
                console.log('ERROR:', dispatchError.toString());
            }
            reject(new Error('Failed'));
            return;
        }
        
        if (status.isFinalized) {
            console.log('Finalized!');
            events.forEach(({ event }) => {
                if (event.section === 'onDemand') {
                    console.log('Event:', event.method, event.data.toHuman());
                }
            });
            resolve();
        }
    }).catch(reject);
});

await api.disconnect();
