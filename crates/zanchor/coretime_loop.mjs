import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

const PARA_ID = 5082;
const MAX_AMOUNT = 100_000_000_000n; // 10 PAS max per order
const INTERVAL_MS = 12000; // Every 12 seconds (2 relay blocks)

async function main() {
    const provider = new WsProvider('wss://paseo.dotters.network');
    const api = await ApiPromise.create({ provider });

    const keyring = new Keyring({ type: 'sr25519' });
    const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');

    console.log('=== Continuous On-Demand Coretime ===');
    console.log('Account:', signer.address);
    console.log('ParaId:', PARA_ID);
    console.log('Interval:', INTERVAL_MS, 'ms');
    console.log('Press Ctrl+C to stop\n');

    // Check balance
    const { data: balance } = await api.query.system.account(signer.address);
    console.log('Balance:', (Number(balance.free) / 1e10).toFixed(4), 'PAS\n');

    let orderCount = 0;
    let lastBlockHeight = 0;

    async function placeOrder() {
        try {
            // Check queue status
            const queueStatus = await api.query.onDemand.queueStatus();
            const queue = queueStatus.toHuman();

            // Get current para head to see if block was produced
            const paraHead = await api.query.paras.heads(PARA_ID);
            const headerHash = paraHead.toHex().slice(0, 20);

            orderCount++;
            console.log(`[${new Date().toISOString()}] Order #${orderCount}`);
            console.log(`  Queue: next=${queue.nextIndex}, smallest=${queue.smallestIndex}`);
            console.log(`  Para head: ${headerHash}...`);

            const tx = api.tx.onDemand.placeOrderKeepAlive(MAX_AMOUNT, PARA_ID);

            await new Promise((resolve, reject) => {
                tx.signAndSend(signer, { nonce: -1 }, ({ status, events, dispatchError }) => {
                    if (dispatchError) {
                        if (dispatchError.isModule) {
                            const decoded = api.registry.findMetaError(dispatchError.asModule);
                            console.log(`  ERROR: ${decoded.section}.${decoded.method}`);
                        } else {
                            console.log(`  ERROR: ${dispatchError.toString()}`);
                        }
                        resolve();
                        return;
                    }

                    if (status.isInBlock) {
                        events.forEach(({ event }) => {
                            if (event.section === 'onDemand' && event.method === 'OnDemandOrderPlaced') {
                                const data = event.data.toHuman();
                                console.log(`  ✓ Order placed! Price: ${data.spotPrice}`);
                            }
                        });
                        resolve();
                    }
                }).catch(e => {
                    console.log(`  ERROR: ${e.message}`);
                    resolve();
                });
            });
        } catch (e) {
            console.log(`  ERROR: ${e.message}`);
        }
    }

    // Initial order
    await placeOrder();

    // Continuous loop
    setInterval(placeOrder, INTERVAL_MS);

    // Keep running
    await new Promise(() => {});
}

main().catch(console.error);
