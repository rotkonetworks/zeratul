import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

async function main() {
    const provider = new WsProvider('ws://127.0.0.1:38659');
    const api = await ApiPromise.create({ provider });
    
    const keyring = new Keyring({ type: 'sr25519' });
    const alice = keyring.addFromUri('//Alice');
    
    const { data: balance } = await api.query.system.account(alice.address);
    console.log('Alice address:', alice.address);
    console.log('Free balance:', balance.free.toHuman());
    console.log('Reserved:', balance.reserved.toHuman());
    
    // Check sudo key
    const sudoKey = await api.query.sudo.key();
    console.log('Sudo key:', sudoKey.toString());
    
    await api.disconnect();
}
main().catch(console.error);
