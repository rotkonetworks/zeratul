import { ApiPromise, WsProvider } from '@polkadot/api';

const RELAY_WS = 'ws://127.0.0.1:38659';

async function main() {
    const provider = new WsProvider(RELAY_WS);
    const api = await ApiPromise.create({ provider });
    
    // List all tx modules
    const modules = Object.keys(api.tx);
    console.log('Available tx modules:', modules.join(', '));
    
    // Check for paras related
    const parasModules = modules.filter(m => m.toLowerCase().includes('para') || m.toLowerCase().includes('registrar'));
    console.log('\nParas-related modules:', parasModules);
    
    // Check sudo
    if (api.tx.sudo) {
        console.log('\nSudo calls:', Object.keys(api.tx.sudo));
    }
    
    // Check registrar
    if (api.tx.registrar) {
        console.log('\nRegistrar calls:', Object.keys(api.tx.registrar));
    }
    
    if (api.tx.paras) {
        console.log('\nParas calls:', Object.keys(api.tx.paras));
    }
    
    await api.disconnect();
}

main().catch(console.error);
