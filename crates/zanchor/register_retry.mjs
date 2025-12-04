import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

const keyring = new Keyring({ type: 'sr25519' });
const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');

// Check balance first
const info = await api.query.system.account(signer.address);
const freePAS = Number(info.data.free) / 10000000000;
console.log('Current free balance:', freePAS.toFixed(4), 'PAS');

// Read and prepare data
const genesisHeadHex = fs.readFileSync('/home/alice/rotko/zeratul/crates/zanchor/zanchor-genesis-head.hex', 'utf8').trim();
const validationCodeHex = fs.readFileSync('/home/alice/rotko/zeratul/crates/zanchor/zanchor-genesis-wasm.hex', 'utf8').trim();
const genesisHead = genesisHeadHex.startsWith('0x') ? genesisHeadHex : '0x' + genesisHeadHex;
const validationCode = validationCodeHex.startsWith('0x') ? validationCodeHex : '0x' + validationCodeHex;

const paraId = 5082;

const tx = api.tx.registrar.register(paraId, genesisHead, validationCode);
const paymentInfo = await tx.paymentInfo(signer);
const feePAS = Number(paymentInfo.partialFee) / 10000000000;

const depositPAS = 1341.288;
const totalNeeded = depositPAS + feePAS;

console.log('Estimated fee:', feePAS.toFixed(4), 'PAS');
console.log('Total needed (deposit + fee):', totalNeeded.toFixed(4), 'PAS');
console.log('Remaining after:', (freePAS - totalNeeded).toFixed(4), 'PAS');

if (freePAS < totalNeeded) {
    console.log('ERROR: Insufficient balance! Need', (totalNeeded - freePAS).toFixed(4), 'more PAS');
    await api.disconnect();
    process.exit(1);
}

console.log('\nSubmitting registration...');
await new Promise((resolve, reject) => {
    tx.signAndSend(signer, ({ status, events, dispatchError }) => {
        if (status.isReady) console.log('Status: Ready');
        if (status.isBroadcast) console.log('Status: Broadcast');
        
        if (status.isInBlock) {
            console.log('Status: InBlock', status.asInBlock.toHex());
            
            if (dispatchError) {
                if (dispatchError.isModule) {
                    const decoded = api.registry.findMetaError(dispatchError.asModule);
                    console.log('DISPATCH ERROR:', decoded.section + '.' + decoded.method);
                    console.log('Docs:', decoded.docs.join(' '));
                } else {
                    console.log('DISPATCH ERROR:', dispatchError.toString());
                }
            }
            
            // Show all events
            console.log('\nEvents:');
            events.forEach(({ event }) => {
                console.log(' ', event.section + '.' + event.method);
                if (event.method === 'ExtrinsicFailed') {
                    console.log('   Data:', event.data.toString());
                }
            });
        }
        
        if (status.isFinalized) {
            console.log('Status: Finalized');
            resolve();
        }
    }).catch(reject);
});

await api.disconnect();
