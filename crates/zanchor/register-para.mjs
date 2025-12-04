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
    
    // Register parachain via sudo
    const call = api.tx.parasSudoWrapper.sudoScheduleParaInitialize(
        PARA_ID,
        {
            genesisHead: genesisHead,
            validationCode: genesisWasm,
            paraKind: true  // true = parachain (not parathread)
        }
    );
    
    console.log('Submitting parachain registration...');
    const sudoCall = api.tx.sudo.sudo(call);
    
    const hash = await sudoCall.signAndSend(alice);
    console.log('Transaction hash:', hash.toHex());
    
    // Wait a bit for inclusion
    await new Promise(r => setTimeout(r, 6000));
    
    // Check if registered
    const paraInfo = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('Para lifecycle:', paraInfo.toString());
    
    await api.disconnect();
}

main().catch(console.error);
