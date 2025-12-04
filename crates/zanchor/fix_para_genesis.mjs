import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';
import fs from 'fs';

const PARA_ID = 5082;

async function main() {
    const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
    const api = await ApiPromise.create({ provider });

    console.log('=== Fix Para 5082 Genesis ===\n');

    const keyring = new Keyring({ type: 'sr25519' });
    const signer = keyring.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');

    console.log('Signer (ss58=42):', signer.address);

    // Check balance
    const { data: balance } = await api.query.system.account(signer.address);
    console.log('Balance:', (Number(balance.free) / 1e10).toFixed(4), 'PAS\n');

    // Read new genesis data
    const genesisState = fs.readFileSync('/tmp/zanchor-genesis-state.hex', 'utf8').trim();
    const genesisWasm = fs.readFileSync('/tmp/zanchor-genesis-wasm.hex', 'utf8').trim();

    console.log('New genesis state length:', (genesisState.length - 2) / 2, 'bytes');
    console.log('New WASM length:', (genesisWasm.length - 2) / 2, 'bytes');

    // Check current state
    const currentHead = await api.query.paras.heads(PARA_ID);
    const currentCodeHash = await api.query.paras.currentCodeHash(PARA_ID);
    console.log('\nCurrent head (first 60):', currentHead.toHex().slice(0, 60) + '...');
    console.log('New head (first 60):', genesisState.slice(0, 60) + '...');
    console.log('Current code hash:', currentCodeHash.toHuman());

    // Check if para is locked
    const paraInfo = await api.query.registrar.paras(PARA_ID);
    const info = paraInfo.toJSON();
    console.log('\nPara locked?', info?.locked);

    if (info?.locked) {
        console.log('ERROR: Para is locked! Cannot modify.');
        await api.disconnect();
        return;
    }

    // Step 1: Schedule code upgrade
    console.log('\n=== Step 1: Schedule Code Upgrade ===');
    const codeUpgradeTx = api.tx.registrar.scheduleCodeUpgrade(PARA_ID, genesisWasm);

    console.log('Submitting code upgrade...');
    await new Promise((resolve, reject) => {
        codeUpgradeTx.signAndSend(signer, { nonce: -1 }, ({ status, events, dispatchError }) => {
            if (dispatchError) {
                if (dispatchError.isModule) {
                    const decoded = api.registry.findMetaError(dispatchError.asModule);
                    console.log(`ERROR: ${decoded.section}.${decoded.method}: ${decoded.docs.join(' ')}`);
                } else {
                    console.log(`ERROR: ${dispatchError.toString()}`);
                }
                resolve();
                return;
            }

            if (status.isInBlock) {
                console.log('Code upgrade in block:', status.asInBlock.toHex());
                events.forEach(({ event }) => {
                    console.log(`  Event: ${event.section}.${event.method}`);
                });
                resolve();
            }
        }).catch(e => {
            console.log('ERROR:', e.message);
            resolve();
        });
    });

    // Step 2: Set current head
    console.log('\n=== Step 2: Set Current Head ===');
    const setHeadTx = api.tx.registrar.setCurrentHead(PARA_ID, genesisState);

    console.log('Submitting head update...');
    await new Promise((resolve, reject) => {
        setHeadTx.signAndSend(signer, { nonce: -1 }, ({ status, events, dispatchError }) => {
            if (dispatchError) {
                if (dispatchError.isModule) {
                    const decoded = api.registry.findMetaError(dispatchError.asModule);
                    console.log(`ERROR: ${decoded.section}.${decoded.method}: ${decoded.docs.join(' ')}`);
                } else {
                    console.log(`ERROR: ${dispatchError.toString()}`);
                }
                resolve();
                return;
            }

            if (status.isInBlock) {
                console.log('Head update in block:', status.asInBlock.toHex());
                events.forEach(({ event }) => {
                    console.log(`  Event: ${event.section}.${event.method}`);
                });
                resolve();
            }
        }).catch(e => {
            console.log('ERROR:', e.message);
            resolve();
        });
    });

    // Check new state
    console.log('\n=== Verifying Changes ===');
    const newHead = await api.query.paras.heads(PARA_ID);
    const newCodeHash = await api.query.paras.currentCodeHash(PARA_ID);
    console.log('New head on chain (first 60):', newHead.toHex().slice(0, 60) + '...');
    console.log('New code hash on chain:', newCodeHash.toHuman());

    console.log('\n=== Next Steps ===');
    console.log('1. Wait for the code upgrade to be enacted (may take a few blocks)');
    console.log('2. Deploy the new chain spec to the collator');
    console.log('3. Restart the collator with wiped data');
    console.log('4. Start buying on-demand coretime');

    await api.disconnect();
}

main().catch(console.error);
