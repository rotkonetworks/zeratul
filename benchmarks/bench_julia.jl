# standardized benchmark for ligerito.jl
using BinaryFields, Ligerito, Random

println("=== ligerito.jl standardized benchmark ===")
println("threads: $(Threads.nthreads())")

# standard parameters
Random.seed!(1234)
k = 20
poly = [BinaryElem32(UInt32(i % 0xFFFFFFFF)) for i in 0:(2^k-1)]

config = Ligerito.hardcoded_config_20(BinaryElem32, BinaryElem128)
verifier_cfg = Ligerito.hardcoded_config_20_verifier()

# warmup run to trigger JIT compilation
println("warming up...")
warmup_proof = prover(config, poly)
verifier(verifier_cfg, warmup_proof)

# benchmark proving (after warmup, excludes compilation time)
prove_time = @elapsed begin
    proof = prover(config, poly)
end

# benchmark verification
verify_time = @elapsed begin
    result = verifier(verifier_cfg, proof)
end

println("proving: $(round(prove_time * 1000, digits=2))ms")
println("verification: $(round(verify_time * 1000, digits=2))ms")
println("verified: $result")
