import { ApiPromise, WsProvider } from '@polkadot/api';

const RELAY_WS = 'ws://127.0.0.1:38659';

async function main() {
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });

    const currentSession = await api.query.session.currentIndex();
    const currentBlock = (await api.rpc.chain.getHeader()).number.toNumber();

    console.log('Current block:', currentBlock);
    console.log('Current session:', currentSession.toNumber());
    console.log('Para 2000 activates at session 108');
    console.log('Sessions to wait:', 108 - currentSession.toNumber());

    await api.disconnect();
}

main().catch(console.error);
