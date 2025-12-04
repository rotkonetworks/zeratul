import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const PARA_ID = 5082;

async function main() {
    const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
    const api = await ApiPromise.create({ provider });

    console.log('=== Schedule Code Upgrade for Para 5082 ===\n');

    const keyring = new Keyring({ type: 'sr25519' });
    const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');

    console.log('Signer:', signer.address);

    // Get current nonce
    const { nonce } = await api.query.system.account(signer.address);
    console.log('Current nonce:', nonce.toNumber());

    // Read new WASM
    const genesisWasm = fs.readFileSync('/tmp/zanchor-genesis-wasm.hex', 'utf8').trim();
    console.log('WASM length:', (genesisWasm.length - 2) / 2, 'bytes');

    // Check current code hash
    const currentCodeHash = await api.query.paras.currentCodeHash(PARA_ID);
    console.log('Current code hash:', currentCodeHash.toHuman());

    // Check if para is locked
    const paraInfo = await api.query.registrar.paras(PARA_ID);
    const info = paraInfo.toJSON();
    console.log('Para locked?', info?.locked);

    if (info?.locked) {
        console.log('ERROR: Para is locked!');
        await api.disconnect();
        return;
    }

    // Schedule code upgrade
    console.log('\nSubmitting code upgrade...');
    const tx = api.tx.registrar.scheduleCodeUpgrade(PARA_ID, genesisWasm);

    await new Promise((resolve, reject) => {
        tx.signAndSend(signer, ({ status, events, dispatchError }) => {
            console.log('Status:', status.type);

            if (dispatchError) {
                if (dispatchError.isModule) {
                    const decoded = api.registry.findMetaError(dispatchError.asModule);
                    console.log(`ERROR: ${decoded.section}.${decoded.method}: ${decoded.docs.join(' ')}`);
                } else {
                    console.log(`ERROR: ${dispatchError.toString()}`);
                }
            }

            if (status.isInBlock) {
                console.log('In block:', status.asInBlock.toHex());
                events.forEach(({ event }) => {
                    console.log(`  Event: ${event.section}.${event.method}`);
                    if (event.method === 'CodeUpgradeScheduled') {
                        console.log('  Data:', event.data.toHuman());
                    }
                });
            }

            if (status.isFinalized) {
                console.log('Finalized!');
                resolve();
            }
        }).catch(e => {
            console.log('ERROR:', e.message);
            resolve();
        });
    });

    // Check for pending upgrade
    const futureCodeHash = await api.query.paras.futureCodeHash(PARA_ID);
    console.log('\nFuture code hash:', futureCodeHash.toHuman());

    const futureCodeUpgrades = await api.query.paras.futureCodeUpgrades(PARA_ID);
    console.log('Future code upgrade at block:', futureCodeUpgrades.toHuman());

    await api.disconnect();
}

main().catch(console.error);
