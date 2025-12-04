import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

const RELAY_WS = 'ws://127.0.0.1:38659';
const PARA_ID = 2000;

async function main() {
    console.log('Connecting to relay chain...');
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });

    const keyring = new Keyring({ type: 'sr25519', ss58Format: api.registry.chainSS58 });
    const alice = keyring.addFromUri('//Alice');

    // Get current lease period
    const currentBlock = (await api.rpc.chain.getHeader()).number.toNumber();
    console.log('Current block:', currentBlock);

    // Assign lease using slots.forceLease
    // Parameters: para, leaser, amount, period_begin, period_count
    const currentLeasePeriod = Math.floor(currentBlock / 100); // Assuming 100 blocks per lease period for local
    console.log('Current lease period estimate:', currentLeasePeriod);

    // Force a lease from now for 100 periods
    const innerCall = api.tx.slots.forceLease(
        PARA_ID,
        alice.address,  // leaser
        0,              // amount (no deposit needed with sudo)
        currentLeasePeriod,      // period_begin
        currentLeasePeriod + 100 // period_count (end period)
    );

    const sudoCall = api.tx.sudo.sudo(innerCall);

    console.log('Assigning lease for para', PARA_ID);

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
                console.log('Lease assigned!');
                api.disconnect().then(resolve);
            }
        }).catch(reject);
    });
}

main().then(() => process.exit(0)).catch(err => {
    console.error(err.message);
    process.exit(1);
});
