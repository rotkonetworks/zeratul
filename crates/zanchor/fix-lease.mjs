import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

const RELAY_WS = 'ws://127.0.0.1:38659';
const PARA_ID = 2000;

async function main() {
    console.log('Connecting to relay chain...');
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });

    const keyring = new Keyring({ type: 'sr25519', ss58Format: api.registry.chainSS58 });
    const alice = keyring.addFromUri('//Alice');

    const currentBlock = (await api.rpc.chain.getHeader()).number.toNumber();
    console.log('Current block:', currentBlock);

    // Force a lease from period 0 for many periods
    const innerCall = api.tx.slots.forceLease(
        PARA_ID,
        alice.address,  // leaser
        0,              // amount (no deposit needed with sudo)
        0,              // period_begin - start from period 0!
        100             // period_count (end period)
    );

    const sudoCall = api.tx.sudo.sudo(innerCall);

    console.log('Assigning lease for para', PARA_ID, 'from period 0');

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
                console.log('Lease fixed!');
                api.disconnect().then(resolve);
            }
        }).catch(reject);
    });
}

main().then(() => process.exit(0)).catch(err => {
    console.error(err.message);
    process.exit(1);
});
