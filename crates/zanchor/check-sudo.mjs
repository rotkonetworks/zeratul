import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

async function main() {
    const provider = new WsProvider('ws://127.0.0.1:38659');
    const api = await ApiPromise.create({ provider });
    
    const sudoKey = await api.query.sudo.key();
    console.log('Sudo key:', sudoKey.toString());
    
    // Check balance of sudo account
    const { data: balance } = await api.query.system.account(sudoKey.toString());
    console.log('Sudo balance:', balance.free.toHuman());
    
    // Check dev accounts
    const keyring = new Keyring({ type: 'sr25519' });
    
    // Try common dev seeds
    const seeds = ['//Alice', '//Bob', '//Charlie', '//Dave', '//Eve', '//Ferdie'];
    for (const seed of seeds) {
        const pair = keyring.addFromUri(seed);
        if (pair.address === sudoKey.toString()) {
            console.log('Sudo is:', seed);
        }
    }
    
    // Also check sr25519 vs ed25519
    const keyringEd = new Keyring({ type: 'ed25519' });
    for (const seed of seeds) {
        const pair = keyringEd.addFromUri(seed);
        if (pair.address === sudoKey.toString()) {
            console.log('Sudo is (ed25519):', seed);
        }
    }
    
    await api.disconnect();
}
main().catch(console.error);
