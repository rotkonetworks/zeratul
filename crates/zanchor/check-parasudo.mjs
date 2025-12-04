import { ApiPromise, WsProvider } from '@polkadot/api';

async function main() {
    const provider = new WsProvider('ws://127.0.0.1:38659');
    const api = await ApiPromise.create({ provider });
    
    console.log('paraSudoWrapper calls:', Object.keys(api.tx.paraSudoWrapper));
    await api.disconnect();
}
main().catch(console.error);
