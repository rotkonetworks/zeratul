#!/bin/bash
cd "$(dirname "$0")/Ligerito.jl"
julia --threads=auto --project=. -e '
using Pkg; Pkg.activate("."); Pkg.instantiate()
using BinaryFields, Ligerito
config = Ligerito.hardcoded_config_20(BinaryElem32, BinaryElem128)
poly = [BinaryElem32(i % UInt32(0xFFFFFFFF)) for i in 0:(2^20-1)]
verifier_cfg = Ligerito.hardcoded_config_20_verifier()
# multiple warmup runs
for _ in 1:5
    proof = prover(config, poly)
    result = verifier(verifier_cfg, proof)
end
# timed runs
p1 = @elapsed proof = prover(config, poly); v1 = @elapsed result = verifier(verifier_cfg, proof)
p2 = @elapsed proof = prover(config, poly); v2 = @elapsed result = verifier(verifier_cfg, proof)
p3 = @elapsed proof = prover(config, poly); v3 = @elapsed result = verifier(verifier_cfg, proof)
println("proving time: $(round(min(p1,p2,p3) * 1000, digits=2))ms")
println("verification time: $(round(min(v1,v2,v3) * 1000, digits=2))ms")
' 2>&1 | grep -E "proving time:|verification time:"
