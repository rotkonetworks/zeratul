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

    console.log('Current block:', (await api.rpc.chain.getHeader()).number.toNumber());

    // First check lifecycle
    const lifecycle = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('Current lifecycle:', lifecycle.toString());

    // Check actions queue
    const actionsQueue = await api.query.paras.actionsQueue.entries();
    for (const [key, value] of actionsQueue) {
        console.log('Actions queue at session', key.args[0].toNumber(), ':', value.toHuman());
    }

    // Check scheduled paras
    const upcomingUpgrades = await api.query.paras.upcomingUpgrades();
    console.log('Upcoming upgrades:', upcomingUpgrades.toHuman());

    // Check if there's a way to force upgrade to parachain
    // Use paras.forceSetCurrentCode and paras.forceSetCurrentHead might work
    // Or we can try registrar.deregister then paraSudoWrapper.sudoScheduleParaInitialize

    // Let's try using registrar.deregister first
    console.log('\nDeregistering para...');
    const deregCall = api.tx.registrar.deregister(PARA_ID);
    const sudoDeregCall = api.tx.sudo.sudo(deregCall);

    await new Promise((resolve, reject) => {
        sudoDeregCall.signAndSend(alice, { nonce: -1 }, ({ status, events, dispatchError }) => {
            console.log('Dereg Status:', status.type);

            if (status.isInBlock) {
                if (dispatchError) {
                    if (dispatchError.isModule) {
                        const decoded = api.registry.findMetaError(dispatchError.asModule);
                        console.error('Dereg Error:', decoded.section, decoded.name);
                    }
                }
                events.forEach(({ event }) => {
                    if (event.section !== 'system' && event.section !== 'balances' && event.section !== 'transactionPayment') {
                        console.log(`Dereg Event: ${event.section}.${event.method}`);
                    }
                });
            }

            if (status.isFinalized) {
                console.log('Deregistration finalized');
                resolve();
            }
        }).catch(reject);
    });

    // Wait a bit
    await new Promise(r => setTimeout(r, 3000));

    // Check lifecycle again
    const newLifecycle = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('\nNew lifecycle after dereg:', newLifecycle.toString() || 'Not found');

    // Now register as parachain with sudoScheduleParaInitialize
    console.log('\nScheduling para initialization as PARACHAIN...');
    const initCall = api.tx.paraSudoWrapper.sudoScheduleParaInitialize(
        PARA_ID,
        {
            genesisHead: genesisHead,
            validationCode: genesisWasm,
            paraKind: true  // true = Parachain
        }
    );

    const sudoInitCall = api.tx.sudo.sudo(initCall);

    await new Promise((resolve, reject) => {
        sudoInitCall.signAndSend(alice, { nonce: -1 }, ({ status, events, dispatchError }) => {
            console.log('Init Status:', status.type);

            if (status.isInBlock) {
                if (dispatchError) {
                    if (dispatchError.isModule) {
                        const decoded = api.registry.findMetaError(dispatchError.asModule);
                        console.error('Init Error:', decoded.section, decoded.name, decoded.docs);
                    }
                }
                events.forEach(({ event }) => {
                    console.log(`Init Event: ${event.section}.${event.method}`);
                });
            }

            if (status.isFinalized) {
                console.log('Para init finalized');
                resolve();
            }
        }).catch(reject);
    });

    // Final check
    await new Promise(r => setTimeout(r, 2000));
    const finalLifecycle = await api.query.paras.paraLifecycles(PARA_ID);
    console.log('\nFinal lifecycle:', finalLifecycle.toString() || 'Not found');

    await api.disconnect();
}

main().then(() => process.exit(0)).catch(err => {
    console.error(err.message);
    process.exit(1);
});
