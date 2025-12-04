import { Keyring } from '@polkadot/api';
import { cryptoWaitReady } from '@polkadot/util-crypto';
import { hexToU8a } from '@polkadot/util';

const MANAGER = '15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT';

async function main() {
    await cryptoWaitReady();

    const keyring = new Keyring({ type: 'sr25519' });

    // Try the KSM seed
    const ksmSeed = '0x646686d51a8f344c66668b62c2b956d1e620fef6cac5a9303b3f9bb134acbb7f';

    console.log('=== Testing seeds and derivations ===\n');
    console.log('Target manager:', MANAGER);

    try {
        const fromSeed = keyring.addFromSeed(hexToU8a(ksmSeed));
        console.log('\nKSM seed raw:', fromSeed.address);
        if (fromSeed.address === MANAGER) console.log('  ^^^ MATCH!');
    } catch (e) {
        console.log('Error with raw seed:', e.message);
    }

    // Try as uri
    try {
        const fromUri = keyring.addFromUri(ksmSeed);
        console.log('KSM seed as URI:', fromUri.address);
        if (fromUri.address === MANAGER) console.log('  ^^^ MATCH!');
    } catch (e) {
        console.log('Error with URI:', e.message);
    }

    // Try some common derivations
    const paths = ['//', '//0', '//1', '//para', '//paseo', '//manager', '//5082'];
    for (const path of paths) {
        try {
            const derived = keyring.addFromUri(ksmSeed + path);
            console.log(`KSM + ${path}:`, derived.address);
            if (derived.address === MANAGER) console.log('  ^^^ MATCH!');
        } catch (e) {}
    }

    // Also try the known mnemonic with different derivations
    const mnemonic = 'move defense manage burden pudding core elite aware tenant payment assault federal';
    console.log('\n=== Mnemonic derivations ===');
    const baseMnemonic = keyring.addFromMnemonic(mnemonic);
    console.log('Base:', baseMnemonic.address);

    for (const path of ['//Alice', '//Bob', '//0', '//1', '//para', '//paseo', '//5082', '//manager', '//sudo']) {
        try {
            const derived = keyring.addFromUri(mnemonic + path);
            console.log(`${path}:`, derived.address);
            if (derived.address === MANAGER) console.log('  ^^^ MATCH!');
        } catch (e) {}
    }

    // Try checking subscan for the original registration transaction
    console.log('\n=== Manager account must be from registration tx ===');
    console.log('Check: https://paseo.subscan.io/account/' + MANAGER);
}

main().catch(console.error);
