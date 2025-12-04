import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

const keyring = new Keyring({ type: 'sr25519' });
const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');
console.log('Signer:', signer.address);

// Read genesis files
const genesisHeadHex = fs.readFileSync('/home/alice/rotko/zeratul/crates/zanchor/zanchor-genesis-head.hex', 'utf8').trim();
const validationCodeHex = fs.readFileSync('/home/alice/rotko/zeratul/crates/zanchor/zanchor-genesis-wasm.hex', 'utf8').trim();

const genesisHead = genesisHeadHex.startsWith('0x') ? genesisHeadHex : '0x' + genesisHeadHex;
const validationCode = validationCodeHex.startsWith('0x') ? validationCodeHex : '0x' + validationCodeHex;

const paraId = 5082;
console.log('Registering ParaId:', paraId);
console.log('Genesis head:', (genesisHead.length - 2) / 2, 'bytes');
console.log('Validation code:', (validationCode.length - 2) / 2, 'bytes');

const tx = api.tx.registrar.register(paraId, genesisHead, validationCode);

// Submit and watch
console.log('Submitting...');
await new Promise((resolve, reject) => {
    tx.signAndSend(signer, ({ status, events, dispatchError }) => {
        console.log('Status:', status.type);
        
        if (dispatchError) {
            if (dispatchError.isModule) {
                const decoded = api.registry.findMetaError(dispatchError.asModule);
                console.log('Error:', decoded.section + '.' + decoded.method);
                console.log('Docs:', decoded.docs.join(' '));
            } else {
                console.log('Error:', dispatchError.toString());
            }
            reject(new Error('Dispatch error'));
            return;
        }
        
        if (status.isInBlock) {
            console.log('In block:', status.asInBlock.toHex());
            events.forEach(({ event }) => {
                console.log('Event:', event.section + '.' + event.method, event.data.toString());
            });
        }
        
        if (status.isFinalized) {
            console.log('Finalized:', status.asFinalized.toHex());
            resolve();
        }
    }).catch(reject);
});

await api.disconnect();
