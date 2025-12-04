import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import { readFileSync } from 'fs';

const RELAY_WS = 'ws://127.0.0.1:38659';
const PARA_ID = 2000;

async function main() {
    console.log('Connecting to relay chain...');
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });
    
    console.log('Chain:', (await api.rpc.system.chain()).toString());
    
    // Read genesis data
    const genesisHead = readFileSync('genesis-head.hex', 'utf8').trim();
    const genesisWasm = readFileSync('genesis-wasm.hex', 'utf8').trim();
    
    console.log('Genesis head length:', genesisHead.length);
    console.log('Genesis wasm length:', genesisWasm.length);
    
    // Setup Alice as sudo
    const keyring = new Keyring({ type: 'sr25519' });
    const alice = keyring.addFromUri('//Alice');
    console.log('Using sudo account:', alice.address);
    
    // Register parachain via paraSudoWrapper (note: no extra 's')
    const call = api.tx.paraSudoWrapper.sudoScheduleParaInitialize(
        PARA_ID,
        {
            genesisHead: genesisHead,
            validationCode: genesisWasm,
            paraKind: true  // true = parachain
        }
    );
    
    console.log('Submitting parachain registration...');
    
    return new Promise((resolve, reject) => {
        call.signAndSend(alice, ({ status, events, dispatchError }) => {
            console.log('Status:', status.type);
            
            if (dispatchError) {
                if (dispatchError.isModule) {
                    const decoded = api.registry.findMetaError(dispatchError.asModule);
                    console.error('Error:', decoded.docs.join(' '));
                } else {
                    console.error('Error:', dispatchError.toString());
                }
            }
            
            if (status.isInBlock) {
                console.log('Included in block:', status.asInBlock.toHex());
            }
            
            if (status.isFinalized) {
                console.log('Finalized in block:', status.asFinalized.toHex());
                
                // Check events
                events.forEach(({ event }) => {
                    console.log('Event:', event.section, event.method);
                });
                
                api.disconnect().then(resolve);
            }
        }).catch(reject);
    });
}

main().then(() => {
    console.log('Done!');
    process.exit(0);
}).catch(err => {
    console.error(err);
    process.exit(1);
});
