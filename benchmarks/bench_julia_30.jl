# standardized benchmark for ligerito.jl - 2^30
using BinaryFields, Ligerito, Random

println("=== ligerito.jl standardized benchmark (2^30) ===")
println("threads: $(Threads.nthreads())")

# standard parameters
Random.seed!(1234)
k = 30
poly = [BinaryElem32(UInt32(i % 0xFFFFFFFF)) for i in 0:(2^k-1)]

config = Ligerito.hardcoded_config_30(BinaryElem32, BinaryElem128)
verifier_cfg = Ligerito.hardcoded_config_30_verifier()

# warmup runs to trigger JIT compilation (2 iterations for very large size)
println("warming up...")
for _ in 1:2
    warmup_proof = prover(config, poly)
    verifier(verifier_cfg, warmup_proof)
end

# benchmark proving (best of 3 after warmup)
p1 = @elapsed proof1 = prover(config, poly)
p2 = @elapsed proof2 = prover(config, poly)
p3 = @elapsed proof = prover(config, poly)
prove_time = min(p1, p2, p3)

# benchmark verification (best of 3)
v1 = @elapsed result1 = verifier(verifier_cfg, proof1)
v2 = @elapsed result2 = verifier(verifier_cfg, proof2)
v3 = @elapsed result = verifier(verifier_cfg, proof)
verify_time = min(v1, v2, v3)

println("proving: $(round(prove_time * 1000, digits=2))ms")
println("verification: $(round(verify_time * 1000, digits=2))ms")
println("verified: $result")
