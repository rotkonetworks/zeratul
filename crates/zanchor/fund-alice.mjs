import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

const RELAY_WS = 'ws://127.0.0.1:38659';

async function main() {
    console.log('Connecting to relay chain...');
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });

    const keyring = new Keyring({ type: 'sr25519', ss58Format: api.registry.chainSS58 });
    const alice = keyring.addFromUri('//Alice');

    // Check current balance
    const { data: balanceBefore } = await api.query.system.account(alice.address);
    console.log('Balance before:', balanceBefore.free.toString());

    // Use sudo to set Alice's balance to a very large amount (1 trillion units)
    const newBalance = BigInt('1000000000000000000000'); // 1 trillion with 12 decimals

    const innerCall = api.tx.balances.forceSetBalance(alice.address, newBalance);
    const sudoCall = api.tx.sudo.sudo(innerCall);

    console.log('Setting balance to:', newBalance.toString());

    return new Promise((resolve, reject) => {
        sudoCall.signAndSend(alice, { nonce: -1 }, async ({ status, events, dispatchError }) => {
            console.log('Status:', status.type);

            if (status.isInBlock) {
                console.log('In block:', status.asInBlock.toHex());

                if (dispatchError) {
                    if (dispatchError.isModule) {
                        const decoded = api.registry.findMetaError(dispatchError.asModule);
                        console.error('Error:', decoded.section, decoded.name, decoded.docs);
                        reject(new Error(decoded.name));
                        return;
                    }
                    console.error('Dispatch error:', dispatchError.toString());
                }

                // Check new balance
                const { data: balanceAfter } = await api.query.system.account(alice.address);
                console.log('Balance after:', balanceAfter.free.toString());
            }

            if (status.isFinalized) {
                console.log('Balance set successfully!');
                await api.disconnect();
                resolve();
            }
        }).catch(reject);
    });
}

main().then(() => process.exit(0)).catch(err => {
    console.error(err.message);
    process.exit(1);
});
