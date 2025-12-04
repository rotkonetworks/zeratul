import { Keyring } from '@polkadot/api';
import { cryptoWaitReady } from '@polkadot/util-crypto';

const MANAGER = '15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT';

async function main() {
    await cryptoWaitReady();

    const keyring = new Keyring({ type: 'sr25519' });

    console.log('Target manager:', MANAGER);
    console.log('\n=== Testing standard dev mnemonics ===\n');

    const mnemonics = [
        // Standard Substrate dev mnemonic (bottom drive...)
        'bottom drive obey lake curtain smoke basket hold race lonely fit walk',
        // Known test mnemonic from coretime_loop.mjs
        'move defense manage burden pudding core elite aware tenant payment assault federal',
        // Polkadot-js UI default
        'lens hood victory skin loop weird insane brain skill credit trial quiz',
        // Common test seed phrase
        'raw security lady smoke fit video flat miracle change hurdle potato apple',
    ];

    for (const mnemonic of mnemonics) {
        console.log(`\nMnemonic: "${mnemonic.slice(0, 30)}..."`);
        const base = keyring.addFromMnemonic(mnemonic);
        console.log('  Base:', base.address);
        if (base.address === MANAGER) console.log('  ^^^ MATCH!');

        // Try with sr25519 paths
        for (const path of ['//Alice', '//paseo', '//para', '//5082', '/0', '/1', '//test']) {
            try {
                const derived = keyring.addFromUri(mnemonic + path);
                if (derived.address === MANAGER) {
                    console.log(`  MATCH with path ${path}!`, derived.address);
                }
            } catch (e) {}
        }
    }

    // Also try //Alice derivation from some other formats
    console.log('\n=== Testing //Alice type keys ===');
    const alice = keyring.addFromUri('//Alice');
    console.log('//Alice:', alice.address);
    if (alice.address === MANAGER) console.log('MATCH!');

    // Test with different ss58 formats - maybe the manager is from a different network prefix
    console.log('\n=== Maybe manager is same key with different ss58? ===');
    const formats = [0, 2, 42]; // 0=Polkadot, 2=Kusama, 42=Generic
    for (const ss58 of formats) {
        const kr = new Keyring({ type: 'sr25519', ss58Format: ss58 });
        const signer = kr.addFromMnemonic('move defense manage burden pudding core elite aware tenant payment assault federal');
        console.log(`ss58=${ss58}: ${signer.address}`);
    }

    console.log('\n=== Check Subscan/Explorer for registration history ===');
    console.log('Para 5082 on Paseo was registered by someone with address:', MANAGER);
    console.log('\nPossible actions:');
    console.log('1. Check if you have access to this account in Polkadot.js extension');
    console.log('2. If not, register a NEW paraId with correct genesis');
    console.log('3. Or contact Paseo operators for sudo help');
}

main().catch(console.error);
