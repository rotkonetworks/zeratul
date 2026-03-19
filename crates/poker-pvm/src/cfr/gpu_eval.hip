// GPU-accelerated Monte Carlo hand strength evaluation
// compiled with hipcc for AMD ROCm (gfx1102 / RX 7600)
//
// each thread:
//   1. generate random opponent cards + remaining board
//   2. evaluate both 7-card hands
//   3. write win/tie/loss to output buffer
//
// launched with N threads per hand strength query.
// CPU reads back win count → equity percentile.

#include <hip/hip_runtime.h>
#include <stdint.h>

// ============================================================================
// hand evaluation (ported from Rust eval_5)
// ============================================================================

__device__ uint32_t eval_5(uint8_t hand[5]) {
    uint8_t ranks[5], suits[5];
    for (int i = 0; i < 5; i++) {
        ranks[i] = hand[i] % 13;
        suits[i] = hand[i] / 13;
    }
    // sort ranks descending (insertion sort, 5 elements)
    for (int i = 1; i < 5; i++) {
        uint8_t key = ranks[i];
        int j = i - 1;
        while (j >= 0 && ranks[j] < key) { ranks[j+1] = ranks[j]; j--; }
        ranks[j+1] = key;
    }

    bool is_flush = suits[0]==suits[1] && suits[1]==suits[2] && suits[2]==suits[3] && suits[3]==suits[4];

    uint8_t diff = ranks[0] - ranks[4]; // safe: sorted descending
    bool all_diff = ranks[0]!=ranks[1] && ranks[1]!=ranks[2] && ranks[2]!=ranks[3] && ranks[3]!=ranks[4];
    bool is_straight = (diff == 4) && all_diff;
    bool is_wheel = ranks[0]==12 && ranks[1]==3 && ranks[2]==2 && ranks[3]==1 && ranks[4]==0;

    // straight flush
    if (is_flush && (is_straight || is_wheel)) {
        return (8u << 20) | (is_wheel ? 3u : (uint32_t)ranks[0]);
    }

    // count frequencies
    uint8_t freq[13] = {0};
    for (int i = 0; i < 5; i++) freq[ranks[i]]++;

    uint8_t quads_r = 0, trips_r = 0, pairs[2] = {0}, pair_count = 0;
    uint8_t kickers[5] = {0}; int kick_idx = 0;

    for (int r = 12; r >= 0; r--) {
        switch(freq[r]) {
            case 4: quads_r = r+1; break;
            case 3: trips_r = r+1; break;
            case 2: if (pair_count < 2) pairs[pair_count++] = r+1; break;
            case 1: if (kick_idx < 5) kickers[kick_idx++] = r; break;
        }
    }

    if (quads_r) return (7u<<20) | ((uint32_t)(quads_r-1)<<4) | kickers[0];
    if (trips_r && pair_count) return (6u<<20) | ((uint32_t)(trips_r-1)<<4) | (pairs[0]-1);
    if (is_flush) return (5u<<20) | ((uint32_t)ranks[0]<<16) | ((uint32_t)ranks[1]<<12) | ((uint32_t)ranks[2]<<8) | ((uint32_t)ranks[3]<<4) | ranks[4];
    if (is_straight) return (4u<<20) | ranks[0];
    if (is_wheel) return (4u<<20) | 3u;
    if (trips_r) return (3u<<20) | ((uint32_t)(trips_r-1)<<8) | ((uint32_t)kickers[0]<<4) | kickers[1];
    if (pair_count >= 2) return (2u<<20) | ((uint32_t)(pairs[0]-1)<<8) | ((uint32_t)(pairs[1]-1)<<4) | kickers[0];
    if (pair_count == 1) return (1u<<20) | ((uint32_t)(pairs[0]-1)<<12) | ((uint32_t)kickers[0]<<8) | ((uint32_t)kickers[1]<<4) | kickers[2];
    return ((uint32_t)ranks[0]<<16) | ((uint32_t)ranks[1]<<12) | ((uint32_t)ranks[2]<<8) | ((uint32_t)ranks[3]<<4) | ranks[4];
}

// best 5 from 7 cards
__device__ uint32_t best_hand_7(uint8_t cards[7]) {
    uint32_t best = 0;
    for (int i = 0; i < 7; i++) {
        for (int j = i+1; j < 7; j++) {
            uint8_t hand[5]; int idx = 0;
            for (int k = 0; k < 7; k++) {
                if (k != i && k != j) hand[idx++] = cards[k];
            }
            uint32_t score = eval_5(hand);
            if (score > best) best = score;
        }
    }
    return best;
}

// ============================================================================
// xorshift RNG (per-thread)
// ============================================================================

__device__ uint32_t xorshift(uint32_t* state) {
    uint32_t x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    return x;
}

// ============================================================================
// Monte Carlo rollout kernel
// ============================================================================

// input: hole cards (2), known board cards (0-5), num_board
// output: results[thread_id] = 0 (loss), 1 (tie), 2 (win)
__global__ void mc_rollout_kernel(
    uint8_t hole0, uint8_t hole1,
    uint8_t* board, int num_board,
    uint8_t* results,
    uint32_t seed,
    int num_threads
) {
    int tid = blockIdx.x * blockDim.x + threadIdx.x;
    if (tid >= num_threads) return;

    // per-thread RNG
    uint32_t rng = seed ^ (tid * 2654435761u);
    rng = rng == 0 ? 1 : rng;

    // mark used cards
    bool used[52] = {false};
    used[hole0] = true;
    used[hole1] = true;
    for (int i = 0; i < num_board; i++) used[board[i]] = true;

    // collect available cards
    uint8_t avail[52]; int navail = 0;
    for (int i = 0; i < 52; i++) {
        if (!used[i]) avail[navail++] = i;
    }

    // pick 2 opponent cards + remaining board cards
    int remaining = 5 - num_board;
    int needed = 2 + remaining;
    uint8_t picks[7]; int npicks = 0;
    bool picked[52] = {false};

    while (npicks < needed) {
        int idx = xorshift(&rng) % navail;
        uint8_t card = avail[idx];
        if (!picked[card]) {
            picked[card] = true;
            picks[npicks++] = card;
        }
    }

    // build full hands
    uint8_t my_cards[7] = {hole0, hole1, 0,0,0,0,0};
    uint8_t opp_cards[7] = {picks[0], picks[1], 0,0,0,0,0};
    for (int i = 0; i < num_board; i++) {
        my_cards[2+i] = board[i];
        opp_cards[2+i] = board[i];
    }
    for (int i = 0; i < remaining; i++) {
        my_cards[2+num_board+i] = picks[2+i];
        opp_cards[2+num_board+i] = picks[2+i];
    }

    uint32_t my_score = best_hand_7(my_cards);
    uint32_t opp_score = best_hand_7(opp_cards);

    results[tid] = (my_score > opp_score) ? 2 : (my_score == opp_score) ? 1 : 0;
}

// ============================================================================
// host API
// ============================================================================

extern "C" {

// compute hand equity: returns equity * 10000 (integer for precision)
// num_rollouts should be a multiple of 256
int gpu_hand_equity(
    uint8_t hole0, uint8_t hole1,
    uint8_t* board, int num_board,
    int num_rollouts
) {
    uint8_t* d_board;
    uint8_t* d_results;
    uint8_t* h_results = (uint8_t*)malloc(num_rollouts);

    hipMalloc(&d_board, 5);
    hipMalloc(&d_results, num_rollouts);
    hipMemcpy(d_board, board, num_board, hipMemcpyHostToDevice);

    int threads_per_block = 256;
    int blocks = (num_rollouts + threads_per_block - 1) / threads_per_block;

    // seed from clock
    uint32_t seed = (uint32_t)clock() ^ 0xCAFEBABE;

    mc_rollout_kernel<<<blocks, threads_per_block>>>(
        hole0, hole1, d_board, num_board, d_results, seed, num_rollouts
    );

    hipMemcpy(h_results, d_results, num_rollouts, hipMemcpyDeviceToHost);

    int wins = 0, ties = 0;
    for (int i = 0; i < num_rollouts; i++) {
        if (h_results[i] == 2) wins++;
        else if (h_results[i] == 1) ties++;
    }

    hipFree(d_board);
    hipFree(d_results);
    free(h_results);

    // equity in basis points (0-10000)
    return (int)((double)(wins * 2 + ties) / (double)(num_rollouts * 2) * 10000.0);
}

// batch: compute equity for N different hole card combos at once
int gpu_hand_equity_batch(
    uint8_t* holes,    // [N][2] hole cards
    uint8_t* board,    // [num_board] community cards
    int num_board,
    int* equities,     // [N] output equities (basis points)
    int num_hands,
    int rollouts_per_hand
) {
    for (int h = 0; h < num_hands; h++) {
        equities[h] = gpu_hand_equity(
            holes[h*2], holes[h*2+1],
            board, num_board,
            rollouts_per_hand
        );
    }
    return 0;
}

} // extern "C"
