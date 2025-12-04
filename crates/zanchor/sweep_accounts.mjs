import { ApiPromise, WsProvider, Keyring } from '@polkadot/api';

const provider = new WsProvider('wss://paseo.rpc.amforc.com:443');
const api = await ApiPromise.create({ provider });

const keyring = new Keyring({ type: 'sr25519' });

const seeds = [
    'traffic where slam fan iron neglect damage rug list gift sudden shoe',
    'solution solve mimic carbon lesson bubble clown chest girl right response keep',
    'domain easy simple artwork cup wear rare genuine furnace pencil awful dizzy',
    'among olive bubble claim cat long horn total mistake certain tourist glory'
];

const mainAccount = '15V2QGGxhadDmGMJtpCBuYwhnCYefx6aLFWFB1fXraRqWcPT';
let totalSent = 0n;

for (const seed of seeds) {
    const account = keyring.addFromMnemonic(seed);
    const info = await api.query.system.account(account.address);
    const free = info.data.free.toBigInt();
    const freePAS = Number(free) / 1e10;
    
    console.log('Account:', account.address);
    console.log('  Free:', freePAS.toFixed(4), 'PAS');
    
    // If has funds, transfer (leave 1 PAS for ED + fees)
    if (free > 20000000000n) { // > 2 PAS
        const transferAmount = free - 15000000000n; // leave 1.5 PAS
        console.log('  Transferring:', (Number(transferAmount) / 1e10).toFixed(4), 'PAS');
        
        try {
            const hash = await api.tx.balances.transferKeepAlive(mainAccount, transferAmount)
                .signAndSend(account);
            console.log('  TX hash:', hash.toHex());
            totalSent += transferAmount;
        } catch (e) {
            console.log('  Transfer error:', e.message);
        }
    }
    console.log('');
}

console.log('Total sent:', (Number(totalSent) / 1e10).toFixed(4), 'PAS');

await new Promise(r => setTimeout(r, 6000)); // Wait for finalization

// Check main account balance
const mainInfo = await api.query.system.account(mainAccount);
console.log('\nMain account new balance:', (Number(mainInfo.data.free) / 1e10).toFixed(4), 'PAS');

await api.disconnect();
