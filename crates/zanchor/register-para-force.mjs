import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import { readFileSync } from 'fs';

const RELAY_WS = 'ws://127.0.0.1:38659';
const PARA_ID = 2000;

async function main() {
    console.log('Connecting to relay chain...');
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });
    
    console.log('Chain:', (await api.rpc.system.chain()).toString());
    console.log('SS58 format:', api.registry.chainSS58);
    
    // Read genesis data  
    const genesisHead = readFileSync('genesis-head.hex', 'utf8').trim();
    const genesisWasm = readFileSync('genesis-wasm.hex', 'utf8').trim();
    
    console.log('Genesis head length:', genesisHead.length);
    console.log('Genesis wasm length:', genesisWasm.length);
    
    // Setup Alice (sudo) - use the correct format
    const keyring = new Keyring({ type: 'sr25519', ss58Format: api.registry.chainSS58 });
    const alice = keyring.addFromUri('//Alice');
    console.log('Using account:', alice.address);
    
    // Check balance
    const { data: balance } = await api.query.system.account(alice.address);
    console.log('Balance:', balance.free.toString());
    
    // Use registrar.forceRegister wrapped in sudo
    const innerCall = api.tx.registrar.forceRegister(
        alice.address,  // who (manager)
        0,              // deposit
        PARA_ID,        // para_id
        genesisHead,    // genesis_head
        genesisWasm     // validation_code
    );
    
    const sudoCall = api.tx.sudo.sudo(innerCall);
    
    console.log('Call hex length:', sudoCall.toHex().length);
    console.log('Submitting...');
    
    return new Promise((resolve, reject) => {
        sudoCall.signAndSend(alice, { nonce: -1 }, ({ status, events, dispatchError }) => {
            console.log('Status:', status.type);
            
            if (status.isInBlock) {
                console.log('In block:', status.asInBlock.toHex());
                events.forEach(({ event }) => {
                    console.log(`Event: ${event.section}.${event.method}`);
                    if (event.section === 'system' && event.method === 'ExtrinsicFailed') {
                        if (dispatchError?.isModule) {
                            const decoded = api.registry.findMetaError(dispatchError.asModule);
                            console.error('Error:', decoded.section, decoded.name, decoded.docs);
                        }
                    }
                });
            }
            
            if (status.isFinalized) {
                console.log('Finalized!');
                api.disconnect().then(resolve);
            }
        }).catch(reject);
    });
}

main().then(() => process.exit(0)).catch(err => {
    console.error(err.message);
    process.exit(1);
});
