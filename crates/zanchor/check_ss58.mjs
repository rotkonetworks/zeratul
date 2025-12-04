import { Keyring } from '@polkadot/api';
import { cryptoWaitReady, encodeAddress, decodeAddress } from '@polkadot/util-crypto';
import { u8aToHex } from '@polkadot/util';

const MANAGER = '15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT';
const SIGNER = '5GYjFw1tqoMkKjLnwB9BmQ7YvaYzyeYSFkmm1igBJVQKL9gU';

async function main() {
    await cryptoWaitReady();

    console.log('=== SS58 Address Analysis ===\n');

    // Decode both addresses to public key
    const managerPubkey = decodeAddress(MANAGER);
    const signerPubkey = decodeAddress(SIGNER);

    console.log('Manager pubkey:', u8aToHex(managerPubkey));
    console.log('Signer pubkey:', u8aToHex(signerPubkey));

    // Check if they're the same
    const same = u8aToHex(managerPubkey) === u8aToHex(signerPubkey);
    console.log('\nSame public key?', same);

    if (!same) {
        // Convert manager to different SS58 formats
        console.log('\n=== Manager address in different formats ===');
        for (const ss58 of [0, 2, 5, 42]) {
            console.log(`ss58=${ss58}: ${encodeAddress(managerPubkey, ss58)}`);
        }

        // Convert signer to different SS58 formats
        console.log('\n=== Signer address in different formats ===');
        for (const ss58 of [0, 2, 5, 42]) {
            console.log(`ss58=${ss58}: ${encodeAddress(signerPubkey, ss58)}`);
        }
    }

    console.log('\n=== Conclusion ===');
    if (same) {
        console.log('SUCCESS: Manager and signer are the same account!');
        console.log('We can use the signer key to manage the para.');
    } else {
        console.log('Manager and signer are DIFFERENT accounts.');
        console.log('Manager pubkey starts with:', u8aToHex(managerPubkey).slice(0, 20));
        console.log('Signer pubkey starts with:', u8aToHex(signerPubkey).slice(0, 20));
        console.log('\nOptions:');
        console.log('1. Find the manager private key');
        console.log('2. Register a NEW para with our signer as manager');
        console.log('3. Contact Paseo operators for sudo assistance');
    }
}

main().catch(console.error);
