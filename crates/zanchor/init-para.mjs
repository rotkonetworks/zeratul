import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import { readFileSync } from 'fs';

const RELAY_WS = 'ws://127.0.0.1:38659';
const PARA_ID = 2000;

async function main() {
    console.log('Connecting to relay chain...');
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });

    const keyring = new Keyring({ type: 'sr25519', ss58Format: api.registry.chainSS58 });
    const alice = keyring.addFromUri('//Alice');

    // Read genesis data
    const genesisHead = readFileSync('genesis-head.hex', 'utf8').trim();
    const genesisWasm = readFileSync('genesis-wasm.hex', 'utf8').trim();

    console.log('Genesis head length:', genesisHead.length);
    console.log('Genesis wasm length:', genesisWasm.length);

    const currentBlock = (await api.rpc.chain.getHeader()).number.toNumber();
    console.log('Current block:', currentBlock);

    // Check available pallets
    console.log('Available pallets with sudo:', Object.keys(api.tx).filter(k => k.toLowerCase().includes('sudo') || k.toLowerCase().includes('para')));

    // Use paraSudoWrapper.sudoScheduleParaInitialize to force parachain mode
    // This takes: id, genesis (head, code, para_kind)
    const innerCall = api.tx.paraSudoWrapper.sudoScheduleParaInitialize(
        PARA_ID,
        {
            genesisHead: genesisHead,
            validationCode: genesisWasm,
            paraKind: true  // true = Parachain, false = Parathread
        }
    );

    const sudoCall = api.tx.sudo.sudo(innerCall);

    console.log('Scheduling para initialization for', PARA_ID);
    console.log('Call hex length:', sudoCall.toHex().length);

    return new Promise((resolve, reject) => {
        sudoCall.signAndSend(alice, { nonce: -1 }, ({ status, events, dispatchError }) => {
            console.log('Status:', status.type);

            if (status.isInBlock) {
                console.log('In block:', status.asInBlock.toHex());

                if (dispatchError) {
                    if (dispatchError.isModule) {
                        const decoded = api.registry.findMetaError(dispatchError.asModule);
                        console.error('Error:', decoded.section, decoded.name, decoded.docs);
                    } else {
                        console.error('Dispatch error:', dispatchError.toString());
                    }
                }

                events.forEach(({ event }) => {
                    console.log(`Event: ${event.section}.${event.method}`);
                });
            }

            if (status.isFinalized) {
                console.log('Para init scheduled!');
                api.disconnect().then(resolve);
            }
        }).catch(reject);
    });
}

main().then(() => process.exit(0)).catch(err => {
    console.error(err.message);
    process.exit(1);
});
