/**
 * cobi.one - Trustless Escrow for Everything
 * P2P bets, trades, services, and private transfers
 * Powered by Verifiable Secret Sharing (VSS)
 */

// Configuration
const CONFIG = {
  chainRpc: 'https://eth-passet-hub-paseo.dotters.network',
  chainId: 420420421,
  // Updated contract address (deployed Dec 15, 2025)
  escrowContract: '0xe40dC8485142A4fb32356b958E05fE9a213A375E',
  // IPFS gateway for metadata
  ipfsGateway: 'https://ipfs.io/ipfs/',
};

// Escrow Types - extensible for any kind of escrow
const ESCROW_TYPES = {
  FIAT_CRYPTO: {
    id: 'fiat_crypto',
    name: 'Fiat to Crypto',
    description: 'Trade fiat (USD, EUR, etc.) for cryptocurrency',
    icon: '💱',
  },
  CRYPTO_CRYPTO: {
    id: 'crypto_crypto',
    name: 'Crypto to Crypto',
    description: 'Swap one cryptocurrency for another',
    icon: '🔄',
  },
  NFT_SALE: {
    id: 'nft_sale',
    name: 'NFT Sale',
    description: 'Buy or sell NFTs with escrow protection',
    icon: '🖼️',
  },
  SERVICE: {
    id: 'service',
    name: 'Service Escrow',
    description: 'Freelance work, consulting, any service',
    icon: '🛠️',
  },
  GOODS: {
    id: 'goods',
    name: 'Physical Goods',
    description: 'Buy/sell physical items with delivery escrow',
    icon: '📦',
  },
};

// Supported cryptocurrencies
const CRYPTOCURRENCIES = {
  UM: { symbol: 'UM', name: 'Penumbra', icon: '🌙', decimals: 6 },
  ZEC: { symbol: 'ZEC', name: 'Zcash', icon: '⚡', decimals: 8 },
  ETH: { symbol: 'ETH', name: 'Ethereum', icon: '⟠', decimals: 18 },
  BTC: { symbol: 'BTC', name: 'Bitcoin', icon: '₿', decimals: 8 },
  USDC: { symbol: 'USDC', name: 'USD Coin', icon: '💵', decimals: 6 },
  DOT: { symbol: 'DOT', name: 'Polkadot', icon: '●', decimals: 10 },
};

// Payment methods
const PAYMENT_METHODS = {
  bank_transfer: { id: 'bank_transfer', name: 'Bank Transfer', icon: '🏦' },
  paypal: { id: 'paypal', name: 'PayPal', icon: '💳' },
  wise: { id: 'wise', name: 'Wise', icon: '🌐' },
  revolut: { id: 'revolut', name: 'Revolut', icon: '📱' },
  cash_deposit: { id: 'cash_deposit', name: 'Cash Deposit', icon: '💵' },
  cash_in_person: { id: 'cash_in_person', name: 'Cash (In Person)', icon: '🤝' },
  crypto: { id: 'crypto', name: 'Cryptocurrency', icon: '🪙' },
  zelle: { id: 'zelle', name: 'Zelle', icon: '⚡' },
  venmo: { id: 'venmo', name: 'Venmo', icon: '📲' },
};

// Escrow states
const ESCROW_STATE = {
  CREATED: 0,
  BUYER_CONFIRMED: 1,
  PAYMENT_SENT: 2,
  COMPLETED: 3,
  DISPUTED: 4,
  RESOLVED_BUYER: 5,
  RESOLVED_SELLER: 6,
};

const STATE_LABELS = {
  [ESCROW_STATE.CREATED]: 'Waiting for Buyer',
  [ESCROW_STATE.BUYER_CONFIRMED]: 'Buyer Joined',
  [ESCROW_STATE.PAYMENT_SENT]: 'Payment Sent',
  [ESCROW_STATE.COMPLETED]: 'Completed',
  [ESCROW_STATE.DISPUTED]: 'Disputed',
  [ESCROW_STATE.RESOLVED_BUYER]: 'Resolved (Buyer Won)',
  [ESCROW_STATE.RESOLVED_SELLER]: 'Resolved (Seller Won)',
};

// ============================================================================
// Binary Field GF(2^32) - Matches contract implementation
// ============================================================================

class BF32 {
  static POLY = 0x8D; // x^7 + x^3 + x^2 + 1

  constructor(value) {
    this.value = value >>> 0;
  }

  static zero() { return new BF32(0); }
  static one() { return new BF32(1); }

  add(other) {
    return new BF32(this.value ^ other.value);
  }

  mul(other) {
    let a = this.value;
    let b = other.value;
    let result = 0;

    for (let i = 0; i < 32; i++) {
      if (b & 1) result ^= a;
      const highBit = a & 0x80000000;
      a = (a << 1) >>> 0;
      if (highBit) a ^= BF32.POLY;
      b >>>= 1;
    }

    return new BF32(result >>> 0);
  }
}

// ============================================================================
// VSS (Verifiable Secret Sharing)
// ============================================================================

class VSS {
  // Generate random 32-byte secret
  static generateSecret() {
    const secret = new Uint8Array(32);
    crypto.getRandomValues(secret);
    return secret;
  }

  // Convert secret to 8 field elements
  static secretToElements(secret) {
    const elements = [];
    const view = new DataView(secret.buffer, secret.byteOffset, secret.byteLength);
    for (let i = 0; i < 8; i++) {
      elements.push(new BF32(view.getUint32(i * 4, true)));
    }
    return elements;
  }

  // Evaluate polynomial using Horner's method
  static evalPoly(coeffs, x) {
    let result = BF32.zero();
    for (let i = coeffs.length - 1; i >= 0; i--) {
      result = result.mul(x).add(coeffs[i]);
    }
    return result;
  }

  // Keccak256 hash (using Web Crypto)
  static async keccak256(data) {
    // For browser, we'd use a library like ethers or viem
    // Simplified placeholder - in production use proper keccak
    const hashBuffer = await crypto.subtle.digest('SHA-256', data);
    return new Uint8Array(hashBuffer);
  }

  // Generate 2-of-3 shares
  static async generateShares(secret) {
    const elements = this.secretToElements(secret);

    // Random coefficients for degree-1 polynomials
    const randomCoeffs = elements.map(() => {
      const coeff = new Uint8Array(4);
      crypto.getRandomValues(coeff);
      const view = new DataView(coeff.buffer);
      return new BF32(view.getUint32(0, true));
    });

    // Evaluation points: 1, 2, 3
    const evalPoints = [new BF32(1), new BF32(2), new BF32(3)];

    // Generate shares
    const shares = evalPoints.map((x, idx) => {
      const values = elements.map((secret, i) => {
        const coeffs = [secret, randomCoeffs[i]];
        return this.evalPoly(coeffs, x).value;
      });
      return { index: idx, values };
    });

    // Compute commitment (Merkle root of share hashes)
    const leafHashes = [];
    for (const share of shares) {
      const data = new Uint8Array(32);
      const view = new DataView(data.buffer);
      share.values.forEach((v, i) => view.setUint32(i * 4, v, true));
      leafHashes.push(await this.keccak256(data));
    }

    // Pad to 4 leaves
    while (leafHashes.length < 4) {
      leafHashes.push(new Uint8Array(32));
    }

    // Build Merkle tree
    let level = leafHashes;
    while (level.length > 1) {
      const nextLevel = [];
      for (let i = 0; i < level.length; i += 2) {
        const combined = new Uint8Array(64);
        combined.set(level[i], 0);
        combined.set(level[i + 1] || level[i], 32);
        nextLevel.push(await this.keccak256(combined));
      }
      level = nextLevel;
    }

    const commitment = this.toHex(level[0]);

    // Escrow pubkey = hash of secret
    const escrowPubkey = this.toHex(await this.keccak256(secret));

    // ShareC as bytes32
    const shareCData = new Uint8Array(32);
    const shareCView = new DataView(shareCData.buffer);
    shares[2].values.forEach((v, i) => shareCView.setUint32(i * 4, v, true));
    const shareC = this.toHex(shareCData);

    return {
      shares,
      commitment,
      escrowPubkey,
      shareC,
      secret,
    };
  }

  static toHex(bytes) {
    return '0x' + Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join('');
  }

  static fromHex(hex) {
    const bytes = new Uint8Array((hex.length - 2) / 2);
    for (let i = 0; i < bytes.length; i++) {
      bytes[i] = parseInt(hex.slice(2 + i * 2, 4 + i * 2), 16);
    }
    return bytes;
  }
}

// ============================================================================
// Contract Interaction
// ============================================================================

class EscrowContract {
  constructor(rpcUrl, contractAddress) {
    this.rpcUrl = rpcUrl;
    this.contractAddress = contractAddress;
    this.provider = null;
    this.signer = null;
  }

  async connect() {
    if (typeof window.ethereum === 'undefined') {
      throw new Error('No Ethereum wallet found');
    }

    const accounts = await window.ethereum.request({
      method: 'eth_requestAccounts',
    });

    // Switch to correct chain
    try {
      await window.ethereum.request({
        method: 'wallet_switchEthereumChain',
        params: [{ chainId: '0x' + CONFIG.chainId.toString(16) }],
      });
    } catch (e) {
      if (e.code === 4902) {
        await window.ethereum.request({
          method: 'wallet_addEthereumChain',
          params: [{
            chainId: '0x' + CONFIG.chainId.toString(16),
            chainName: 'Paseo Asset Hub',
            rpcUrls: [CONFIG.chainRpc],
            nativeCurrency: { name: 'PAS', symbol: 'PAS', decimals: 18 },
          }],
        });
      }
    }

    this.signer = accounts[0];
    return this.signer;
  }

  // Compute function selector
  selector(signature) {
    // In production, use proper keccak256
    // This is a simplified version
    const selectors = {
      'createEscrow(bytes32,bytes32,bytes32,bytes32)': '0x12345678',
      'confirmEscrow(bytes32)': '0x23456789',
      'markPaymentSent(bytes32)': '0x34567890',
      'confirmPayment(bytes32)': '0x45678901',
      'dispute(bytes32)': '0x56789012',
      'resolveDispute(bytes32,bool)': '0x67890123',
      'getEscrow(bytes32)': '0x78901234',
    };
    return selectors[signature] || '0x00000000';
  }

  // Generate escrow ID from seller + nonce
  generateEscrowId(seller, nonce) {
    // In production, use proper keccak256(abi.encode(seller, nonce))
    const data = seller + nonce.toString(16).padStart(64, '0');
    return '0x' + data.slice(2, 66);
  }

  async createEscrow(escrowId, commitment, escrowPubkey, shareC) {
    if (!this.signer) throw new Error('Not connected');

    const data = this.selector('createEscrow(bytes32,bytes32,bytes32,bytes32)') +
      escrowId.slice(2) +
      commitment.slice(2) +
      escrowPubkey.slice(2) +
      shareC.slice(2);

    const txHash = await window.ethereum.request({
      method: 'eth_sendTransaction',
      params: [{
        from: this.signer,
        to: this.contractAddress,
        data,
        gas: '0x100000',
      }],
    });

    return txHash;
  }

  async confirmEscrow(escrowId) {
    if (!this.signer) throw new Error('Not connected');

    const data = this.selector('confirmEscrow(bytes32)') + escrowId.slice(2);

    return window.ethereum.request({
      method: 'eth_sendTransaction',
      params: [{
        from: this.signer,
        to: this.contractAddress,
        data,
        gas: '0x80000',
      }],
    });
  }

  async markPaymentSent(escrowId) {
    if (!this.signer) throw new Error('Not connected');

    const data = this.selector('markPaymentSent(bytes32)') + escrowId.slice(2);

    return window.ethereum.request({
      method: 'eth_sendTransaction',
      params: [{
        from: this.signer,
        to: this.contractAddress,
        data,
        gas: '0x80000',
      }],
    });
  }

  async confirmPayment(escrowId) {
    if (!this.signer) throw new Error('Not connected');

    const data = this.selector('confirmPayment(bytes32)') + escrowId.slice(2);

    return window.ethereum.request({
      method: 'eth_sendTransaction',
      params: [{
        from: this.signer,
        to: this.contractAddress,
        data,
        gas: '0x80000',
      }],
    });
  }

  async dispute(escrowId) {
    if (!this.signer) throw new Error('Not connected');

    const data = this.selector('dispute(bytes32)') + escrowId.slice(2);

    return window.ethereum.request({
      method: 'eth_sendTransaction',
      params: [{
        from: this.signer,
        to: this.contractAddress,
        data,
        gas: '0x80000',
      }],
    });
  }
}

// ============================================================================
// Local Storage for Offers and Trades
// ============================================================================

class OfferStore {
  static KEY = 'cobi_offers';

  static getAll() {
    const data = localStorage.getItem(this.KEY);
    return data ? JSON.parse(data) : [];
  }

  static save(offers) {
    localStorage.setItem(this.KEY, JSON.stringify(offers));
  }

  static add(offer) {
    const offers = this.getAll();
    offers.push({ ...offer, id: Date.now(), createdAt: new Date().toISOString() });
    this.save(offers);
    return offers[offers.length - 1];
  }

  static remove(id) {
    const offers = this.getAll().filter(o => o.id !== id);
    this.save(offers);
  }
}

class TradeStore {
  static KEY = 'cobi_trades';

  static getAll() {
    const data = localStorage.getItem(this.KEY);
    return data ? JSON.parse(data) : [];
  }

  static save(trades) {
    localStorage.setItem(this.KEY, JSON.stringify(trades));
  }

  static add(trade) {
    const trades = this.getAll();
    trades.push({
      ...trade,
      id: Date.now(),
      tradeId: Math.random().toString(36).substring(2, 10).toUpperCase(),
      createdAt: new Date().toISOString(),
      state: ESCROW_STATE.CREATED,
    });
    this.save(trades);
    return trades[trades.length - 1];
  }

  static update(id, updates) {
    const trades = this.getAll();
    const idx = trades.findIndex(t => t.id === id);
    if (idx >= 0) {
      trades[idx] = { ...trades[idx], ...updates };
      this.save(trades);
    }
  }

  static get(id) {
    return this.getAll().find(t => t.id === id);
  }
}

// ============================================================================
// Export for use in HTML
// ============================================================================

window.Cobi = {
  CONFIG,
  ESCROW_TYPES,
  CRYPTOCURRENCIES,
  PAYMENT_METHODS,
  ESCROW_STATE,
  STATE_LABELS,
  VSS,
  EscrowContract,
  OfferStore,
  TradeStore,
  BF32,
};

console.log('🔐 Cobi P2P loaded. Access via window.Cobi');
