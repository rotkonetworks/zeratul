#include <stdio.h>
#include <stdint.h>
#include <time.h>

extern int gpu_hand_equity(uint8_t hole0, uint8_t hole1, uint8_t* board, int num_board, int num_rollouts);

int main() {
    // AA preflop (no board)
    clock_t start = clock();
    int aa_equity = gpu_hand_equity(12, 25, NULL, 0, 100000); // 100K rollouts
    clock_t end = clock();
    double ms = (double)(end - start) / CLOCKS_PER_SEC * 1000.0;
    printf("AA preflop equity: %d bp (%.1f%%) [100K rollouts in %.1fms]\n", aa_equity, aa_equity/100.0, ms);

    // 72o preflop
    start = clock();
    int _72o = gpu_hand_equity(5, 13, NULL, 0, 100000);
    end = clock();
    ms = (double)(end - start) / CLOCKS_PER_SEC * 1000.0;
    printf("72o preflop equity: %d bp (%.1f%%) [100K rollouts in %.1fms]\n", _72o, _72o/100.0, ms);

    // AKs on flop As Kd 7c
    uint8_t board[3] = {12, 24, 5}; // As=12, Kd=24(11+13), 7c=5+39=44... wait
    // card encoding: rank + suit*13. A♠=12, K♦=11+2*13=37, 7♣=5+3*13=44
    uint8_t board2[3] = {12, 37, 44};
    start = clock();
    // AKs = A♠(12) K♠(11)
    int aks_flop = gpu_hand_equity(12, 11, board2, 3, 100000);
    end = clock();
    ms = (double)(end - start) / CLOCKS_PER_SEC * 1000.0;
    printf("AKs on AK7 flop equity: %d bp (%.1f%%) [100K rollouts in %.1fms]\n", aks_flop, aks_flop/100.0, ms);

    return 0;
}
